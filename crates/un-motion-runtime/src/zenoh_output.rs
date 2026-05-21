//! Zenoh による UNMotionFrame 出力 worker。
//!
//! UNMotion から外部 (UN Avatar / UNVirtualAvatarConnect / 任意の Zenoh subscriber) へ
//! `UNMotionFrame` を publish する。
//!
//! - 設計は VMC output worker と同じスレッドモデル: `spawn_zenoh_output_worker` で
//!   バックグラウンドスレッドを起動し、`Sender<ZenohOutputCommand>` で
//!   post-process 済み `UNMotionFrame` を送り込む。
//! - Publisher backend は `un-motion-frame-zenoh::ZenohSessionBackend` を実セッションとして使う。
//!   `[features = ["zenoh-transport"]]` (default-on) を前提とする。

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use tracing::{debug, info, warn};
use un_motion_frame::{
	CoordinateSpace, HumanoidBone, LengthUnit, MotionSourceInfo, MotionSourceKind, TimestampBasis, TrackingState, UNMotionFrame,
};
use un_motion_frame_zenoh::{Publisher, TopicMode, ZenohSessionBackend, ZenohTopicStrategy};

use crate::modifier::{ModifierConfig, ModifierPipeline};
use crate::signal_enrich::enrich_frame_with_signal_derived_motion;

/// UNMotion → Zenoh 出力の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct ZenohOutputConfig {
	/// `un-motion/frame` のような末尾 `/` を含まないベース key。
	/// `ZenohTopicStrategy::base_key_expr` にそのまま渡される。
	pub base_key_expr: String,
	/// publish 時の key 構築モード (`Frame` / `ByPrimarySource` / `ByStreamId`)。
	pub topic_mode: TopicMode,
	/// `UNMotionFrame.header.stream_id` に詰める論理ストリーム識別子。
	/// `None` のときは `MotionHeader` の既定 (= 未設定) のまま。
	/// `TopicMode::ByStreamId` を使うときは設定しておくと subscriber 側で区別しやすい。
	pub stream_id: Option<String>,
	/// 1 フレームあたりの公称インターバル (nanoseconds)。
	/// `UNMotionFrame.header.expected_dt_ns` に詰める。`None` なら未設定。
	pub expected_dt_ns: Option<u64>,
	/// publisher の `producer` フィールドに詰める識別子。`None` なら未設定。
	pub producer: Option<String>,
	/// Capturer 出力段の Modifier 設定 (Engine 非依存の bone subset filter)。
	/// 既定 (`ModifierConfig::default()`) は pass-through (全カテゴリ ON) なので、
	/// 既存の Capturer は何も変更せずに従来通り動作する。
	pub modifier: ModifierConfig,
}

impl ZenohOutputConfig {
	pub fn new(base_key_expr: impl Into<String>) -> Self {
		Self {
			base_key_expr: base_key_expr.into(),
			topic_mode: TopicMode::Frame,
			stream_id: None,
			expected_dt_ns: None,
			producer: None,
			modifier: ModifierConfig::default(),
		}
	}

	pub fn with_topic_mode(mut self, mode: TopicMode) -> Self {
		self.topic_mode = mode;
		self
	}

	pub fn with_stream_id(mut self, stream_id: impl Into<String>) -> Self {
		self.stream_id = Some(stream_id.into());
		self
	}

	pub fn with_expected_dt_ns(mut self, expected_dt_ns: u64) -> Self {
		self.expected_dt_ns = Some(expected_dt_ns);
		self
	}

	pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
		self.producer = Some(producer.into());
		self
	}

	pub fn with_modifier(mut self, modifier: ModifierConfig) -> Self {
		self.modifier = modifier;
		self
	}

	pub fn topic_strategy(&self) -> ZenohTopicStrategy {
		ZenohTopicStrategy::new(self.base_key_expr.clone(), self.topic_mode)
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum ZenohOutputFrame {
	UnmotionFrame(UNMotionFrame),
}

impl From<UNMotionFrame> for ZenohOutputFrame {
	fn from(frame: UNMotionFrame) -> Self {
		Self::UnmotionFrame(frame)
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum ZenohOutputCommand {
	Send(ZenohOutputFrame),
	Shutdown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZenohOutputStats {
	pub sent_frames: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZenohOutputEvent {
	Sent { key_expr: String, frames: u64 },
	Error { message: String },
	Stopped { stats: ZenohOutputStats },
}

pub struct ZenohOutputWorker {
	config: ZenohOutputConfig,
	modifier_pipeline: ModifierPipeline,
	publisher: Publisher<ZenohSessionBackend>,
	next_sequence: u64,
	last_direct_frame_timestamp_ns: Option<u64>,
	stats: ZenohOutputStats,
}

impl ZenohOutputWorker {
	pub fn open(config: ZenohOutputConfig) -> anyhow::Result<Self> {
		info!(
			target: "un_motion_runtime::zenoh_output",
			base_key_expr = %config.base_key_expr,
			topic_mode = ?config.topic_mode,
			stream_id = ?config.stream_id,
			producer = ?config.producer,
			"opening Zenoh session for UNMotionFrame publisher (default features = zenoh-transport)",
		);
		let backend = ZenohSessionBackend::open_default()
			.map_err(|e| anyhow::anyhow!("zenoh session open failed: {e}"))
			.context("Zenoh session open")?;
		let publisher = Publisher::new(backend).with_strategy(config.topic_strategy());
		info!(
			target: "un_motion_runtime::zenoh_output",
			base_key_expr = %config.base_key_expr,
			"Zenoh session ready; subscribers should match this base key (UN Avatar default: 'un-motion/frame/v1')",
		);
		Ok(Self {
			modifier_pipeline: ModifierPipeline::from_config(&config.modifier),
			config,
			publisher,
			next_sequence: 0,
			last_direct_frame_timestamp_ns: None,
			stats: ZenohOutputStats::default(),
		})
	}

	pub fn with_backend(config: ZenohOutputConfig, backend: ZenohSessionBackend) -> Self {
		let publisher = Publisher::new(backend).with_strategy(config.topic_strategy());
		Self {
			modifier_pipeline: ModifierPipeline::from_config(&config.modifier),
			config,
			publisher,
			next_sequence: 0,
			last_direct_frame_timestamp_ns: None,
			stats: ZenohOutputStats::default(),
		}
	}

	pub fn stats(&self) -> &ZenohOutputStats {
		&self.stats
	}

	pub fn config(&self) -> &ZenohOutputConfig {
		&self.config
	}

	pub fn send_frame(&mut self, frame: ZenohOutputFrame) -> anyhow::Result<ZenohOutputEvent> {
		let mut unmotion_frame = match frame {
			ZenohOutputFrame::UnmotionFrame(frame) => self.finalize_frame(frame),
		};
		// MediaPipe Native などの signal-only frame は VMC 出力側では signal fallback で
		// Humanoid bone OSC に変換される。UNMF/Z でも同じ意味の body/face を載せてから
		// publish しないと、UNAvatar 側が直接適用できない。
		enrich_frame_with_signal_derived_motion(&mut unmotion_frame);
		// Engine 非依存の Modifier を出力直前に適用。
		// `is_pass_through()` の場合は内部で早期 return するので overhead は無い。
		self.modifier_pipeline.apply(&mut unmotion_frame);
		self.next_sequence = self.next_sequence.saturating_add(1);
		let key_expr = self.publisher.strategy().key_expr_for_frame(&unmotion_frame);
		self.publisher
			.send(&unmotion_frame)
			.map_err(|e| anyhow::anyhow!("zenoh publish failed on {key_expr}: {e}"))?;
		self.stats.sent_frames = self.stats.sent_frames.saturating_add(1);
		// 初回 1 frame と以降は 600 frame ごとに INFO で「実際に publish した key と
		// payload 概要」を出す。これにより subscribe 側 (UN Avatar) で同じ key を
		// listen しているか / 中身は空でないか をユーザー stderr で確認できる。
		if self.stats.sent_frames == 1 || self.stats.sent_frames % 600 == 0 {
			let bones = unmotion_frame
				.body
				.as_ref()
				.and_then(|b| b.humanoid.as_ref())
				.map(|h| h.bones.len())
				.unwrap_or(0);
			let blendshapes = unmotion_frame.face.as_ref().map(|f| f.expressions.len()).unwrap_or(0);
			let left_hand_fingers = unmotion_frame.left_hand.as_ref().map(|h| h.fingers.len()).unwrap_or(0);
			let right_hand_fingers = unmotion_frame.right_hand.as_ref().map(|h| h.fingers.len()).unwrap_or(0);
			let left_hand_joints = unmotion_frame
				.left_hand
				.as_ref()
				.map(|h| h.fingers.iter().map(|f| f.joints.len()).sum::<usize>())
				.unwrap_or(0);
			let right_hand_joints = unmotion_frame
				.right_hand
				.as_ref()
				.map(|h| h.fingers.iter().map(|f| f.joints.len()).sum::<usize>())
				.unwrap_or(0);
			let left_finger_probe = diagnostic_finger_quat_summary(unmotion_frame.left_hand.as_ref());
			let right_finger_probe = diagnostic_finger_quat_summary(unmotion_frame.right_hand.as_ref());
			info!(
				target: "un_motion_runtime::zenoh_output",
				sent_frames = self.stats.sent_frames,
				key_expr = %key_expr,
				sequence = unmotion_frame.header.sequence,
				bones,
				blendshapes,
				left_hand_fingers,
				right_hand_fingers,
				left_hand_joints,
				right_hand_joints,
				left_finger_probe = %left_finger_probe,
				right_finger_probe = %right_finger_probe,
				coordinate_space = ?unmotion_frame.header.coordinate_space,
				"UNMotionFrame/Zenoh publish (subscribers should listen on the same key)",
			);
		}
		Ok(ZenohOutputEvent::Sent { key_expr, frames: 1 })
	}

	pub fn stopped_event(&self) -> ZenohOutputEvent {
		ZenohOutputEvent::Stopped { stats: self.stats.clone() }
	}

	/// 既に組み立てられた UNMotionFrame に対して、worker 設定 (stream_id / expected_dt_ns /
	/// sequence) を後付けで適用し、subscriber が期待する header 既定値を補完する。
	///
	/// # Phase E retrograde fix (E-α-3/4/5 で発生した受信不具合の修正)
	///
	/// UNMotionFrame 単一ルート化後、Engine 側が `coordinate_space` / `timestamp_basis` /
	/// `sources` 等の header フィールドを未設定 (= `Default::default() == Unknown`) のまま
	/// publish するケースがあった。UN Avatar の retarget は `frame.header.coordinate_space`
	/// を見て VRM 座標への変換を行うので、これが `Unknown` だと bone rotation が
	/// 意味不明になり、結果として「subscriber は受信しているがアバターが動かない」
	/// 沈黙故障になっていた。
	///
	/// ここでは subscriber が必要とする header の暗黙既定値を publish 直前に保証する。
	/// 値が既に明示設定されている (Engine 側で適切に埋めている) 場合は触らない方針
	/// (`Unknown` のときだけ補完)。
	fn finalize_frame(&mut self, mut frame: UNMotionFrame) -> UNMotionFrame {
		frame.header.sequence = self.next_sequence;
		if let Some(stream_id) = &self.config.stream_id {
			frame.header.stream_id = Some(stream_id.clone());
		}
		if let Some(expected_dt_ns) = self
			.observed_direct_expected_dt_ns(frame.header.frame_timestamp_ns)
			.or(self.config.expected_dt_ns)
			.or(frame.header.expected_dt_ns)
		{
			frame.header.expected_dt_ns = Some(expected_dt_ns);
		}
		if let Some(producer) = &self.config.producer
			&& frame.metadata.producer.is_none()
		{
			frame.metadata.producer = Some(producer.clone());
		}
		// Phase E retrograde fix: coordinate_space / timestamp_basis の補完。
		// MediaPipe Native source は UNMotionFrame::new() の default 値 (= Unknown) のまま
		// 出してくるため、UN Avatar 側の retarget が機能しなくなっていた。
		if frame.header.coordinate_space == CoordinateSpace::Unknown {
			frame.header.coordinate_space = CoordinateSpace::UNMotion;
		}
		if frame.header.timestamp_basis == TimestampBasis::Unknown {
			frame.header.timestamp_basis = TimestampBasis::UnixEpoch;
		}
		if frame.header.length_unit == LengthUnit::default() {
			frame.header.length_unit = LengthUnit::Meter;
		}
		// sources が空ならフォールバック: subscriber が `sources.first().source_id` で
		// primary を識別する実装に備える。`stream_id` が事実上の Capturer 識別子。
		if frame.sources.is_empty()
			&& let Some(stream_id) = &self.config.stream_id
		{
			frame.sources.push(MotionSourceInfo {
				source_id: stream_id.clone(),
				source_kind: MotionSourceKind::WebcamPose,
				display_name: self.config.producer.clone(),
				confidence: 1.0,
				latency_ns: None,
				state: TrackingState::Valid,
			});
		}
		// capture / frame / processed timestamp が未設定 (= 0) なら現在時刻で埋める。
		// Engine 側で適切な時刻が入っているなら触らない。未設定 frame でも
		// subscriber 側の latency 計測等が想定通り動くようにする。
		let now_ns = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map(|d| d.as_nanos() as u64)
			.unwrap_or_default();
		if frame.header.capture_timestamp_ns == 0 {
			frame.header.capture_timestamp_ns = now_ns;
		}
		if frame.header.frame_timestamp_ns == 0 {
			frame.header.frame_timestamp_ns = now_ns;
		}
		if frame.header.processed_timestamp_ns == 0 {
			frame.header.processed_timestamp_ns = now_ns;
		}
		frame
	}

	fn observed_direct_expected_dt_ns(&mut self, frame_timestamp_ns: u64) -> Option<u64> {
		observe_frame_dt_ns(&mut self.last_direct_frame_timestamp_ns, frame_timestamp_ns)
	}
}

fn observe_frame_dt_ns(last_frame_timestamp_ns: &mut Option<u64>, frame_timestamp_ns: u64) -> Option<u64> {
	if frame_timestamp_ns == 0 {
		return None;
	}
	let observed = last_frame_timestamp_ns
		.and_then(|previous| frame_timestamp_ns.checked_sub(previous))
		.filter(|dt| *dt > 0);
	*last_frame_timestamp_ns = Some(frame_timestamp_ns);
	observed
}

fn diagnostic_finger_quat_summary(hand: Option<&un_motion_frame::HandMotion>) -> String {
	let Some(hand) = hand else {
		return "none".to_string();
	};
	hand.fingers
		.iter()
		.filter_map(|finger| {
			let quat = finger.joints.first()?.rotation?;
			Some(format!(
				"{:?}=({:.3},{:.3},{:.3},{:.3})",
				finger.finger, quat.x, quat.y, quat.z, quat.w
			))
		})
		.collect::<Vec<_>>()
		.join(";")
}

pub struct ZenohOutputWorkerHandle {
	pub base_key_expr: String,
	command_tx: Sender<ZenohOutputCommand>,
	join: Option<JoinHandle<()>>,
}

impl ZenohOutputWorkerHandle {
	pub fn send(&self, frame: impl Into<ZenohOutputFrame>) -> Result<(), std::sync::mpsc::SendError<ZenohOutputCommand>> {
		self.command_tx.send(ZenohOutputCommand::Send(frame.into()))
	}

	pub fn shutdown(&self) {
		let _ = self.command_tx.send(ZenohOutputCommand::Shutdown);
	}

	pub fn join(mut self) -> thread::Result<()> {
		self.shutdown();
		if let Some(join) = self.join.take() { join.join() } else { Ok(()) }
	}
}

impl Drop for ZenohOutputWorkerHandle {
	fn drop(&mut self) {
		let _ = self.command_tx.send(ZenohOutputCommand::Shutdown);
	}
}

pub fn spawn_zenoh_output_worker(config: ZenohOutputConfig, event_tx: Sender<ZenohOutputEvent>) -> anyhow::Result<ZenohOutputWorkerHandle> {
	let worker = ZenohOutputWorker::open(config)?;
	let base_key_expr = worker.config.base_key_expr.clone();
	let topic_mode = worker.config.topic_mode;
	let stream_id = worker.config.stream_id.clone();
	info!(
		target: "un_motion_runtime::zenoh_output",
		base_key_expr = %base_key_expr,
		?topic_mode,
		stream_id = stream_id.as_deref().unwrap_or("-"),
		"Zenoh output worker started",
	);
	let (command_tx, command_rx) = mpsc::channel();
	let join = thread::spawn(move || run_zenoh_output_worker(worker, command_rx, event_tx));
	Ok(ZenohOutputWorkerHandle {
		base_key_expr,
		command_tx,
		join: Some(join),
	})
}

fn run_zenoh_output_worker(mut worker: ZenohOutputWorker, command_rx: Receiver<ZenohOutputCommand>, event_tx: Sender<ZenohOutputEvent>) {
	// 1 秒に 1 回程度の頻度で送信状況を debug ログに出す。多すぎると log が溢れ、
	// 少なすぎると Phase E e2e Step B の切り分けに使えないので保守的に 30 frame 毎。
	const SEND_LOG_INTERVAL: u64 = 30;
	let base_key_expr = worker.config.base_key_expr.clone();
	for command in command_rx {
		match command {
			ZenohOutputCommand::Send(frame) => match worker.send_frame(frame) {
				Ok(event) => {
					let frames = worker.stats().sent_frames;
					if frames == 1 || frames % SEND_LOG_INTERVAL == 0 {
						if let ZenohOutputEvent::Sent { key_expr, .. } = &event {
							debug!(
								target: "un_motion_runtime::zenoh_output",
								base_key_expr = %base_key_expr,
								key_expr = %key_expr,
								sent_frames = frames,
								"Zenoh publish ok",
							);
						}
					}
					let _ = event_tx.send(event);
				}
				Err(error) => {
					warn!(
						target: "un_motion_runtime::zenoh_output",
						base_key_expr = %base_key_expr,
						error = %error,
						"Zenoh publish failed",
					);
					let _ = event_tx.send(ZenohOutputEvent::Error {
						message: error.to_string(),
					});
				}
			},
			ZenohOutputCommand::Shutdown => break,
		}
	}
	info!(
		target: "un_motion_runtime::zenoh_output",
		base_key_expr = %base_key_expr,
		sent_frames = worker.stats().sent_frames,
		"Zenoh output worker stopped",
	);
	let _ = event_tx.send(worker.stopped_event());
}

/// VMC / UNMotion ランタイムが扱うボーン名文字列を `HumanoidBone` enum に解決する。
///
/// 既知の VRM Humanoid bone 名のみ拾い、未知の名前 (custom bone, head accessory, etc.) は
/// `None` を返して呼び出し側で捨てさせる。
pub fn vmc_bone_name_to_humanoid_bone(name: &str) -> Option<HumanoidBone> {
	match name {
		"Hips" => Some(HumanoidBone::Hips),
		"Spine" => Some(HumanoidBone::Spine),
		"Chest" => Some(HumanoidBone::Chest),
		"UpperChest" => Some(HumanoidBone::UpperChest),
		"Neck" => Some(HumanoidBone::Neck),
		"Head" => Some(HumanoidBone::Head),
		"LeftShoulder" => Some(HumanoidBone::LeftShoulder),
		"LeftUpperArm" => Some(HumanoidBone::LeftUpperArm),
		"LeftLowerArm" => Some(HumanoidBone::LeftLowerArm),
		"LeftHand" => Some(HumanoidBone::LeftHand),
		"RightShoulder" => Some(HumanoidBone::RightShoulder),
		"RightUpperArm" => Some(HumanoidBone::RightUpperArm),
		"RightLowerArm" => Some(HumanoidBone::RightLowerArm),
		"RightHand" => Some(HumanoidBone::RightHand),
		"LeftUpperLeg" => Some(HumanoidBone::LeftUpperLeg),
		"LeftLowerLeg" => Some(HumanoidBone::LeftLowerLeg),
		"LeftFoot" => Some(HumanoidBone::LeftFoot),
		"LeftToes" => Some(HumanoidBone::LeftToes),
		"RightUpperLeg" => Some(HumanoidBone::RightUpperLeg),
		"RightLowerLeg" => Some(HumanoidBone::RightLowerLeg),
		"RightFoot" => Some(HumanoidBone::RightFoot),
		"RightToes" => Some(HumanoidBone::RightToes),
		"LeftEye" => Some(HumanoidBone::LeftEye),
		"RightEye" => Some(HumanoidBone::RightEye),
		"Jaw" => Some(HumanoidBone::Jaw),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_frame::{MotionSignal, MotionSignalValue, SampleState};

	#[test]
	fn signal_enrich_does_not_synthesize_vmc_body_for_zenoh() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(MotionSignal {
			name: "arm.left.elbow.x".to_string(),
			value: MotionSignalValue::Scalar(-0.2),
			confidence: 1.0,
			state: SampleState::Valid,
			source_index: None,
		});

		let inserted = enrich_frame_with_signal_derived_motion(&mut frame);

		assert!(!inserted);
		assert_eq!(frame.header.coordinate_space, CoordinateSpace::Unknown);
		assert!(frame.body.is_none());
	}

	#[test]
	fn direct_frame_expected_dt_prefers_observed_timestamp_delta() {
		let mut last = None;

		assert_eq!(observe_frame_dt_ns(&mut last, 1_000_000_000), None);
		assert_eq!(observe_frame_dt_ns(&mut last, 1_037_000_000), Some(37_000_000));
		assert_eq!(observe_frame_dt_ns(&mut last, 1_037_000_000), None);
		assert_eq!(observe_frame_dt_ns(&mut last, 1_070_000_000), Some(33_000_000));
	}
}
