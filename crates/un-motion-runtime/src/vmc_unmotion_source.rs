//! Phase E-α-6: VMC/UDP 受信エンジン (`MotionFrameSource` trait 実装)。
//!
//! VMC/UDP 経由で外部送信者 (Waidayo / VSeeFace / iPad VMC 送信器 / mocopi 等) から
//! 流入する OSC データを `un_motion_input_vmc::VmcInputSource` でデコードし、
//! `VmcInputFrame` → `UNMotionFrame` 変換を行って `MotionFrameStreamWorker` に渡す。
//!
//! これにより VMC 入力も Capturer 正式経路
//! `Input → Engine(decoder) → UNMotionFrame → Modifier → Output` に乗る。
//!
//! # データフロー
//!
//! ```text
//! VMC/UDP listen → VmcInputSource (decode OSC)
//!                    ↓ VmcInputFrame { root, bones, blendshapes }
//! vmc_input_frame_to_unmotion_frame()
//!                    ↓ UNMotionFrame { body.humanoid.{root, bones}, face.expressions }
//! VmcUnmotionSource::next_frame() → UNMotionFrame → Modifier → [UNMF/Z, VMC/UDP]
//! ```
//!
//! # マッピング
//!
//! - `VmcInputFrame.root` → `UNMotionFrame.body.humanoid.root` (translation + rotation)
//! - `VmcInputFrame.bones[*]` (name: String) → `UNMotionFrame.body.humanoid.bones[*]`
//!   `humanoid_bone_from_name()` で Unity Humanoid 命名規則 (Hips, LeftHand など) を
//!   `HumanoidBone` enum に解決する。未知の bone 名は捨てる (warn ログのみ)。
//! - `VmcInputFrame.blendshapes[*]` (name + value) → `UNMotionFrame.face.expressions[*]`
//!   `/VMC/Ext/Blend/Apply` を受け取った直後の frame でのみ face を埋める。
//!
//! # スコープ外
//!
//! - 速度 / 加速度 (`linear_velocity` / `angular_velocity`): VMC は持たないので `None`。
//! - 視線情報 (eyes / gaze): VMC コア仕様には含まれない (拡張 OSC アドレスは未対応)。
//! - hand finger landmark: VMC は手指 bone を root pose と同じ humanoid bones で
//!   表現するため、別途 hand-specific 経路には流さない。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use tracing::{debug, info, warn};
use un_motion_frame::{
	BodyMotion, BoneSample, CoordinateSpace, ExpressionSample, FaceMotion, Handedness, HumanoidBone, HumanoidPose, LengthUnit, Quatf,
	SampleState, TrackingState, TransformSample, UNMotionFrame, Vec3f,
};
use un_motion_input_vmc::{VmcBlendshapeSample, VmcBoneSample, VmcInputConfig, VmcInputFrame, VmcInputSource, VmcTransform};

use crate::{MotionFrameSource, SourceTelemetryHandle};

/// VMC/UDP 受信を `MotionFrameSource` として提供する。
///
/// # Aggregation model
///
/// Waidayo / VSeeFace / mocopi 等は VMC プロトコルの設計上 「1 論理フレーム」を
/// 複数の OSC bundle に分割して送出する:
///
/// * Bundle A: `/VMC/Ext/Bone/Pos` ×13 (= 全 humanoid bone)
/// * Bundle B: `/VMC/Ext/Blend/Val` ×13 (basic VRM blendshape)
/// * Bundle C: `/VMC/Ext/Blend/Val` ×70 (ARKit + 拡張 blendshape)
/// * Bundle D: `/VMC/Ext/Blend/Apply` (commit)
///
/// bundle 単位で UNMotionFrame を emit すると、Waidayo 1 frame あたり「ボーンだけ入った UNF」
/// 「blendshape だけ入った UNF」「空 UNF」が 4 本生成され、UN Avatar 側では
/// pose flicker (ボーン → 全 unset → blendshape → 全 unset → ...) が起こって
/// まともに描画できない。
///
/// 新実装は VmcInputFrame の root / bones / blendshapes を per-bundle に
/// accumulator (BTreeMap で per-bone / per-blendshape merge) へ取り込み、
/// `next_frame()` 1 回ごとに「最新スナップショット」を 1 つの UNMotionFrame
/// として emit する。
///
/// * accumulator は emit 後もクリアしない: 各 channel (root / bones[i] /
///   blendshape[name]) は「最後に届いた値」を保持する。Waidayo / VSeeFace は
///   毎フレーム全 channel を上書きしてくるので結果として常に最新スナップに
///   なる。
/// * dirty flag を持ち、新規 bundle が来ていない `next_frame()` 呼び出しでは
///   `Ok(None)` を返す (= renderer の idle tick で重複 frame を emit しない)。
pub struct VmcUnmotionSource {
	source: VmcInputSource,
	source_id: String,
	sequence: u64,
	/// per-bundle に届く root transform の最新値。
	pending_root: Option<VmcTransform>,
	/// bone 名 (VMC 規約上は HumanoidBone enum と 1:1 の "Hips", "Spine" 等) →
	/// 最新の VmcBoneSample。
	pending_bones: BTreeMap<String, VmcBoneSample>,
	/// blendshape 名 → 最新値。VMC `/VMC/Ext/Blend/Val` の累積。
	pending_blendshapes: BTreeMap<String, f32>,
	/// 最新の受信 unix timestamp ns。emit する UNMotionFrame の
	/// `capture_timestamp_ns` / `frame_timestamp_ns` に使う。
	pending_received_ts: u64,
	/// 前回 `next_frame()` 呼び出し以降に accumulator が更新されたか。
	/// false の間は emit せずに `Ok(None)` で帰る。
	pending_dirty: bool,
	/// 観測用累積カウンタ。Capturer stderr に「最初の datagram 到着」と
	/// 一定間隔の累計を流すために使う。bind は成功したが Waidayo / iPad などが
	/// 送信していない or Firewall で詰まる、というケースを判別する。
	///
	/// `SourceTelemetryHandle::atomics` (Arc<SourceStageAtomics>) に集約し、
	/// Capturer の runtime loop が `runtime_snapshot` 直前に `load(Relaxed)` で
	/// 読み出して `OutputTelemetry.sources` に積む。同じカウンタを periodic ログの
	/// 「跨ぎ判定」にも `load(Relaxed)` で使う (worker thread 単独 writer なので
	/// monotonic に増えると仮定して安全)。
	telemetry: SourceTelemetryHandle,
	announced_first_datagram: bool,
	announced_first_frame: bool,
	announced_first_non_vmc: bool,
}

impl VmcUnmotionSource {
	/// VMC/UDP listener を bind し、ソースを構築する。
	///
	/// `source_id` は `UNMotionFrame.metadata.source.id` に格納される識別子で、
	/// 受信側で複数の Capturer を識別する用途に使う。
	pub fn bind(source_id: impl Into<String>, listen_addr: SocketAddr) -> anyhow::Result<Self> {
		let source_id = source_id.into();
		let source = VmcInputSource::bind(VmcInputConfig::new(source_id.clone(), listen_addr))
			.with_context(|| format!("VMC receive engine bind failed: {listen_addr}"))?;
		let local = source.local_addr().ok();
		info!(
			target: "un_motion_runtime::vmc_unmotion_source",
			source_id = %source_id,
			requested = %listen_addr,
			bound = ?local,
			"VMC receive engine bound and listening for OSC datagrams (waiting for Waidayo / VSeeFace / iPad / mocopi to send)",
		);
		Ok(Self {
			source,
			source_id: source_id.clone(),
			sequence: 0,
			pending_root: None,
			pending_bones: BTreeMap::new(),
			pending_blendshapes: BTreeMap::new(),
			pending_received_ts: 0,
			pending_dirty: false,
			telemetry: SourceTelemetryHandle::new("vmc-receive", source_id),
			announced_first_datagram: false,
			announced_first_frame: false,
			announced_first_non_vmc: false,
		})
	}

	/// 1 つの `VmcInputFrame` を accumulator にマージする。同じ channel の値が
	/// 既にあれば上書き (=「最後に届いた値が現在の状態」)。
	fn merge_bundle(&mut self, frame: &VmcInputFrame) {
		if let Some(root) = &frame.root {
			self.pending_root = Some(root.clone());
		}
		for bone in &frame.bones {
			self.pending_bones.insert(bone.name.clone(), bone.clone());
		}
		for blend in &frame.blendshapes {
			self.pending_blendshapes.insert(blend.name.clone(), blend.value);
		}
		if frame.received_timestamp_ns > self.pending_received_ts {
			self.pending_received_ts = frame.received_timestamp_ns;
		}
		if frame.has_vmc_payload() || frame.blend_apply {
			self.pending_dirty = true;
			self.telemetry.atomics.bundles_merged.fetch_add(1, Ordering::Relaxed);
		}
	}

	/// accumulator の現在状態を `VmcInputFrame` 形式の snapshot に詰めて
	/// `vmc_input_frame_to_unmotion_frame` に委譲する。これにより
	/// `BoneSample` / `ExpressionSample` / `FaceMotion` の schema 変更点が
	/// 1 ヶ所 (= 既存の converter) で吸収される。
	fn build_frame_from_accumulator(&self, sequence: u64) -> UNMotionFrame {
		let snapshot = VmcInputFrame {
			source_id: self.source_id.clone(),
			received_timestamp_ns: self.pending_received_ts,
			raw_datagram: None,
			ok: None,
			root: self.pending_root.clone(),
			bones: self.pending_bones.values().cloned().collect(),
			blendshapes: self
				.pending_blendshapes
				.iter()
				.map(|(name, value)| VmcBlendshapeSample {
					name: name.clone(),
					value: *value,
				})
				.collect(),
			blend_apply: false,
			message_count: 0,
		};
		vmc_input_frame_to_unmotion_frame(&snapshot, sequence)
	}

	pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
		self.source.local_addr()
	}

	pub fn source_id(&self) -> &str {
		&self.source_id
	}

	/// Phase E telemetry: ロックフリーカウンタへの読み取り専用ハンドル。
	/// Capturer の runtime loop が `runtime_snapshot` 直前に `load(Relaxed)` する。
	pub fn telemetry_handle(&self) -> SourceTelemetryHandle {
		self.telemetry.clone()
	}
}

impl MotionFrameSource for VmcUnmotionSource {
	fn next_frame(&mut self) -> anyhow::Result<Option<UNMotionFrame>> {
		let batch = self.source.poll_batch()?;
		// 観測ログ用に poll サイクル単位の差分を累積へ反映する。
		const RECV_LOG_INTERVAL: u64 = 300;
		const FRAME_LOG_INTERVAL: u64 = 300;

		if batch.received_datagrams > 0 {
			let before = self
				.telemetry
				.atomics
				.raw_received
				.fetch_add(batch.received_datagrams, Ordering::Relaxed);
			let after = before.saturating_add(batch.received_datagrams);
			if !self.announced_first_datagram {
				self.announced_first_datagram = true;
				info!(
					target: "un_motion_runtime::vmc_unmotion_source",
					source_id = %self.source_id,
					received_datagrams = batch.received_datagrams,
					decoded_frames = batch.frames.len(),
					non_vmc_dropped = batch.non_vmc_dropped,
					decode_errors = batch.decode_errors,
					"VMC receive engine received first inbound UDP datagram",
				);
			}
			// `RECV_LOG_INTERVAL` の境界を跨いだ場合に periodic 累積ログを出す。
			if after / RECV_LOG_INTERVAL > before / RECV_LOG_INTERVAL {
				let snap = self.telemetry.atomics.snapshot();
				info!(
					target: "un_motion_runtime::vmc_unmotion_source",
					source_id = %self.source_id,
					total_received = snap.raw_received,
					total_frames_emitted = snap.frames_emitted,
					total_decode_errors = snap.decode_errors,
					total_non_vmc_dropped = snap.non_vmc_dropped,
					"VMC receive engine cumulative receive counters",
				);
			}
		}

		if batch.non_vmc_dropped > 0 {
			self.telemetry
				.atomics
				.non_vmc_dropped
				.fetch_add(batch.non_vmc_dropped, Ordering::Relaxed);
			if !self.announced_first_non_vmc {
				self.announced_first_non_vmc = true;
				info!(
					target: "un_motion_runtime::vmc_unmotion_source",
					source_id = %self.source_id,
					non_vmc_dropped_this_batch = batch.non_vmc_dropped,
					"VMC receive engine dropped non-VMC datagrams (e.g. Waidayo /MP/ MotionPath extension); only standard /VMC/Ext/* messages are processed",
				);
			}
		}

		if batch.decode_errors > 0 {
			self.telemetry
				.atomics
				.decode_errors
				.fetch_add(batch.decode_errors, Ordering::Relaxed);
			for example in batch.decode_error_examples.iter() {
				warn!(
					target: "un_motion_runtime::vmc_unmotion_source",
					source_id = %self.source_id,
					%example,
					"VMC OSC decode error (datagram discarded)",
				);
			}
		}

		// 1 batch 内に複数 VmcInputFrame が含まれることが普通 (Waidayo は 1 論理
		// フレームを 4 bundle に分割して送る)。1 つずつ accumulator にマージ
		// する。マージ自体は安価 (BTreeMap insert)。
		for frame in batch.frames.iter() {
			self.merge_bundle(frame);
		}
		debug!(
			target: "un_motion_runtime::vmc_unmotion_source",
			source_id = %self.source_id,
			bundles_in_batch = batch.frames.len(),
			pending_bones = self.pending_bones.len(),
			pending_blendshapes = self.pending_blendshapes.len(),
			pending_root = self.pending_root.is_some(),
			"merged VMC bundles into accumulator",
		);

		// dirty でない (=この poll cycle で何も merge していない / 既に emit 済み)
		// なら何も返さない。renderer の idle tick で同じ frame を 60fps で再送
		// するのを防ぐ。
		if !self.pending_dirty {
			return Ok(None);
		}

		let unmotion = self.build_frame_from_accumulator(self.sequence);
		self.pending_dirty = false;

		let before_frames = self.telemetry.atomics.frames_emitted.fetch_add(1, Ordering::Relaxed);
		let after_frames = before_frames.saturating_add(1);
		if !self.announced_first_frame {
			self.announced_first_frame = true;
			let snap = self.telemetry.atomics.snapshot();
			info!(
				target: "un_motion_runtime::vmc_unmotion_source",
				source_id = %self.source_id,
				bundles_merged = snap.bundles_merged,
				bones = self.pending_bones.len(),
				blendshapes = self.pending_blendshapes.len(),
				root = self.pending_root.is_some(),
				"VMC receive engine produced first UNMotionFrame from accumulated VMC payload",
			);
		}
		if after_frames / FRAME_LOG_INTERVAL > before_frames / FRAME_LOG_INTERVAL {
			let snap = self.telemetry.atomics.snapshot();
			info!(
				target: "un_motion_runtime::vmc_unmotion_source",
				source_id = %self.source_id,
				total_frames_emitted = snap.frames_emitted,
				total_bundles_merged = snap.bundles_merged,
				pending_bones = self.pending_bones.len(),
				pending_blendshapes = self.pending_blendshapes.len(),
				"VMC receive engine cumulative frame emission",
			);
		}

		self.sequence = self.sequence.saturating_add(1);
		Ok(Some(unmotion))
	}

	fn telemetry_handle(&self) -> Option<SourceTelemetryHandle> {
		Some(self.telemetry.clone())
	}
}

/// VMC OSC frame を `UNMotionFrame` に変換する。
///
/// # Coordinate space (Phase E debug でユーザー実機から判明したバグの恒久対応)
///
/// VMC プロトコルは Unity の Humanoid 規約 (Y-up, **left-handed**) で
/// quaternion / translation を運ぶ。一方 `UNMotionFrame::new()` の既定は
/// `coordinate_space = Unknown` で、これをそのまま Zenoh に publish すると
/// `ZenohOutputWorker::finalize_frame` が `Unknown → UNMotion` で上書きし、
/// UN Avatar 側の `convert_rotation_from_coordinate_space`
/// (`crates/un-avatar-skeleton/src/humanoid_retarget.rs`) は `Vmc` 専用の
/// 軸変換 (`(qx, qy, qz, qw) → (-qx, -qy, qz, qw)` for VRM0) を skip する。
///
/// その結果、同じ VMC 入力が:
/// * VMC 出力 → VSeeFace: 正しく表示 (consumer も Unity 規約)
/// * UNMF/Z 出力 → UN Avatar: **Head 含む全 bone の X/Y 回転が反転**
///
/// となり、ユーザーから「Head の X 軸 / Y 軸が反転している」と報告された。
///
/// 修正: VMC 受信エンジンが作る UNMotionFrame は header で
/// `coordinate_space = Vmc` / `handedness = LeftHanded` / `length_unit = Meter`
/// を明示する。`finalize_frame` の `Unknown → UNMotion` 既定は `Vmc` を
/// 上書きしないので、UN Avatar 側は Vmc 専用の正しい変換を適用する。
pub fn vmc_input_frame_to_unmotion_frame(frame: &VmcInputFrame, sequence: u64) -> UNMotionFrame {
	let mut output = UNMotionFrame::new(sequence);
	output.header.capture_timestamp_ns = frame.received_timestamp_ns;
	output.header.frame_timestamp_ns = frame.received_timestamp_ns;
	output.header.coordinate_space = CoordinateSpace::Vmc;
	output.header.handedness = Handedness::LeftHanded;
	output.header.length_unit = LengthUnit::Meter;

	let root_sample = frame.root.as_ref().map(vmc_transform_to_frame_transform);
	let bone_samples: Vec<BoneSample> = frame
		.bones
		.iter()
		.filter_map(|bone| {
			let bone_kind = humanoid_bone_from_name(&bone.name)?;
			Some(BoneSample {
				bone: bone_kind,
				transform: TransformSample {
					translation: Some(Vec3f {
						x: bone.position[0],
						y: bone.position[1],
						z: bone.position[2],
					}),
					rotation: Some(Quatf {
						x: bone.rotation[0],
						y: bone.rotation[1],
						z: bone.rotation[2],
						w: bone.rotation[3],
					}),
					scale: None,
					linear_velocity: None,
					angular_velocity: None,
				},
				confidence: 1.0,
				source_index: None,
				state: SampleState::Valid,
			})
		})
		.collect();

	if root_sample.is_some() || !bone_samples.is_empty() {
		output.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: root_sample,
				bones: bone_samples,
			}),
		});
	}

	if !frame.blendshapes.is_empty() {
		let expressions: Vec<ExpressionSample> = frame
			.blendshapes
			.iter()
			.enumerate()
			.map(|(idx, sample)| ExpressionSample {
				name: sample.name.clone(),
				value: sample.value,
				confidence: 1.0,
				source_index: Some(idx as u16),
				state: SampleState::Valid,
			})
			.collect();
		output.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions,
		});
	}

	output
}

fn vmc_transform_to_frame_transform(transform: &VmcTransform) -> TransformSample {
	TransformSample {
		translation: Some(Vec3f {
			x: transform.position[0],
			y: transform.position[1],
			z: transform.position[2],
		}),
		rotation: Some(Quatf {
			x: transform.rotation[0],
			y: transform.rotation[1],
			z: transform.rotation[2],
			w: transform.rotation[3],
		}),
		scale: None,
		linear_velocity: None,
		angular_velocity: None,
	}
}

/// Unity Humanoid bone 名 (VMC OSC で使われる) を `HumanoidBone` enum に解決する。
/// 未知の bone 名は `None` を返し、呼出側は warn ログのみで捨てる。
///
/// VMC 受信側で対応する bone は VMC Protocol 公式の Unity HumanBodyBones に準拠:
/// <https://protocol.vmc.info/english.html>
pub fn humanoid_bone_from_name(name: &str) -> Option<HumanoidBone> {
	Some(match name {
		"Hips" => HumanoidBone::Hips,
		"Spine" => HumanoidBone::Spine,
		"Chest" => HumanoidBone::Chest,
		"UpperChest" => HumanoidBone::UpperChest,
		"Neck" => HumanoidBone::Neck,
		"Head" => HumanoidBone::Head,
		"LeftShoulder" => HumanoidBone::LeftShoulder,
		"LeftUpperArm" => HumanoidBone::LeftUpperArm,
		"LeftLowerArm" => HumanoidBone::LeftLowerArm,
		"LeftHand" => HumanoidBone::LeftHand,
		"RightShoulder" => HumanoidBone::RightShoulder,
		"RightUpperArm" => HumanoidBone::RightUpperArm,
		"RightLowerArm" => HumanoidBone::RightLowerArm,
		"RightHand" => HumanoidBone::RightHand,
		"LeftUpperLeg" => HumanoidBone::LeftUpperLeg,
		"LeftLowerLeg" => HumanoidBone::LeftLowerLeg,
		"LeftFoot" => HumanoidBone::LeftFoot,
		"LeftToes" => HumanoidBone::LeftToes,
		"RightUpperLeg" => HumanoidBone::RightUpperLeg,
		"RightLowerLeg" => HumanoidBone::RightLowerLeg,
		"RightFoot" => HumanoidBone::RightFoot,
		"RightToes" => HumanoidBone::RightToes,
		"LeftEye" => HumanoidBone::LeftEye,
		"RightEye" => HumanoidBone::RightEye,
		"Jaw" => HumanoidBone::Jaw,
		// 未対応: 個別 finger bone (LeftThumbProximal 等) は HumanoidBone enum に
		// 含まれないので捨てる。手指追跡が必要なら別 trait / 別 field で対応する。
		_ => return None,
	})
}

/// `SystemTime::now()` を Unix epoch ns に変換するヘルパー (内部用)。
#[allow(dead_code)]
fn now_unix_ns() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_nanos() as u64)
		.unwrap_or(0)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::SourceStageCounters;
	use rosc::{OscMessage, OscType, encoder};
	use std::net::{SocketAddr, UdpSocket};
	use std::time::{Duration, Instant};
	use un_motion_input_vmc::{VmcBlendshapeSample, VmcBoneSample};

	/// Direct route 経路 (VmcUnmotionSource) の bind → receive → frame emit が
	/// 通り、観測カウンタが正しく動くことを検証する。Waidayo profile で
	/// 「ログだけ見れば送信側か Capturer 側かが判別できる」性質を保証する。
	#[test]
	fn vmc_unmotion_source_observes_received_datagrams_and_emits_frame() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
		let mut source = VmcUnmotionSource::bind("test-vmc-unmotion", listen_addr).expect("bind");
		let bound = source.local_addr().expect("local_addr");
		// 観測カウンタ初期値
		{
			let snap = source.telemetry.atomics.snapshot();
			assert_eq!(snap.raw_received, 0);
			assert_eq!(snap.frames_emitted, 0);
		}
		assert!(!source.announced_first_datagram);

		// VMC sender 役: 単一の /VMC/Ext/Bone/Pos message を bundle で送信。
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender bind");
		let bone_msg = OscMessage {
			addr: "/VMC/Ext/Bone/Pos".to_string(),
			args: vec![
				OscType::String("Head".to_string()),
				OscType::Float(0.0),
				OscType::Float(1.5),
				OscType::Float(0.0),
				OscType::Float(0.0),
				OscType::Float(0.0),
				OscType::Float(0.0),
				OscType::Float(1.0),
			],
		};
		let bundle = rosc::OscBundle {
			timetag: rosc::OscTime { seconds: 0, fractional: 1 },
			content: vec![rosc::OscPacket::Message(bone_msg)],
		};
		let datagram = encoder::encode(&rosc::OscPacket::Bundle(bundle)).expect("encode");
		sender.send_to(&datagram, bound).expect("send datagram");

		// next_frame() が Some(UNMotionFrame) を返すまで poll する (datagram の到達は非同期)。
		let deadline = Instant::now() + Duration::from_secs(1);
		let mut got_frame = None;
		while Instant::now() < deadline {
			match source.next_frame().expect("next_frame") {
				Some(frame) => {
					got_frame = Some(frame);
					break;
				}
				None => std::thread::sleep(Duration::from_millis(10)),
			}
		}
		let frame = got_frame.expect("expected at least one UNMotionFrame");
		// 受信観測カウンタが更新されている。
		let snap = source.telemetry.atomics.snapshot();
		assert!(snap.raw_received >= 1, "raw_received={}", snap.raw_received);
		assert!(source.announced_first_datagram);
		assert_eq!(snap.frames_emitted, 1);
		assert!(source.announced_first_frame);
		// frame に Head bone が乗っている。
		let humanoid = frame.body.expect("body").humanoid.expect("humanoid");
		assert_eq!(humanoid.bones.len(), 1);
		assert_eq!(humanoid.bones[0].bone, HumanoidBone::Head);
	}

	/// VMC 受信エンジン経由で emit された UNMotionFrame は header に
	/// `coordinate_space = Vmc` を立てている必要がある。これが立っていないと
	/// `ZenohOutputWorker::finalize_frame` が Unknown → UNMotion で上書きし、
	/// UN Avatar 側の `convert_rotation_from_coordinate_space` が Vmc 専用の
	/// (qx, qy, qz, qw) → (-qx, -qy, qz, qw) 軸変換 (VRM0) を skip して全 bone の
	/// X/Y 回転が反転して見えるバグになる (Phase E debug でユーザー実機から判明)。
	#[test]
	fn vmc_unmotion_source_stamps_coordinate_space_as_vmc() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
		let mut source = VmcUnmotionSource::bind("test-coord", listen_addr).expect("bind");
		let bound = source.local_addr().expect("local_addr");
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender bind");
		let bundle = rosc::OscBundle {
			timetag: rosc::OscTime { seconds: 0, fractional: 1 },
			content: vec![rosc::OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Bone/Pos".to_string(),
				args: vec![
					OscType::String("Head".to_string()),
					OscType::Float(0.0),
					OscType::Float(1.5),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(1.0),
				],
			})],
		};
		let datagram = encoder::encode(&rosc::OscPacket::Bundle(bundle)).expect("encode");
		sender.send_to(&datagram, bound).expect("send");
		let deadline = Instant::now() + Duration::from_secs(1);
		let mut emitted: Option<UNMotionFrame> = None;
		while Instant::now() < deadline {
			if let Some(frame) = source.next_frame().expect("next_frame") {
				emitted = Some(frame);
				break;
			}
			std::thread::sleep(Duration::from_millis(10));
		}
		let frame = emitted.expect("expected emitted frame");
		assert_eq!(
			frame.header.coordinate_space,
			CoordinateSpace::Vmc,
			"VMC source must stamp coordinate_space=Vmc so UN Avatar applies Unity LH → VRM axis conversion",
		);
		assert_eq!(frame.header.handedness, Handedness::LeftHanded);
		assert_eq!(frame.header.length_unit, LengthUnit::Meter);
	}

	/// Waidayo / VSeeFace は 1 論理 frame を複数 OSC bundle に分割して送ってくる。
	/// このテストは「bones bundle, blendshape bundle, Apply bundle」を別個に
	/// 送って、accumulator が**1 つの完全な UNMotionFrame**にマージできることを
	/// 検証する。bundle 単位ではなく論理 frame 単位で emit する必要がある。
	#[test]
	fn vmc_unmotion_source_aggregates_split_bundles_into_single_unmotion_frame() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
		let mut source = VmcUnmotionSource::bind("test-agg", listen_addr).expect("bind");
		let bound = source.local_addr().expect("local_addr");
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender bind");

		// 1) Bones-only bundle
		let bones_bundle = rosc::OscBundle {
			timetag: rosc::OscTime { seconds: 0, fractional: 1 },
			content: vec![
				rosc::OscPacket::Message(OscMessage {
					addr: "/VMC/Ext/Bone/Pos".to_string(),
					args: vec![
						OscType::String("Head".to_string()),
						OscType::Float(0.0),
						OscType::Float(1.5),
						OscType::Float(0.0),
						OscType::Float(0.0),
						OscType::Float(0.0),
						OscType::Float(0.0),
						OscType::Float(1.0),
					],
				}),
				rosc::OscPacket::Message(OscMessage {
					addr: "/VMC/Ext/Bone/Pos".to_string(),
					args: vec![
						OscType::String("Hips".to_string()),
						OscType::Float(0.0),
						OscType::Float(0.9),
						OscType::Float(0.0),
						OscType::Float(0.0),
						OscType::Float(0.0),
						OscType::Float(0.0),
						OscType::Float(1.0),
					],
				}),
			],
		};
		let bones_data = encoder::encode(&rosc::OscPacket::Bundle(bones_bundle)).expect("encode bones");
		sender.send_to(&bones_data, bound).expect("send bones");

		// 2) Blendshape Val bundle (12 expressions)
		let mut blend_msgs = Vec::new();
		for i in 0..12 {
			blend_msgs.push(rosc::OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Val".to_string(),
				args: vec![OscType::String(format!("Blend{}", i)), OscType::Float(0.5)],
			}));
		}
		let blend_bundle = rosc::OscBundle {
			timetag: rosc::OscTime { seconds: 0, fractional: 1 },
			content: blend_msgs,
		};
		let blend_data = encoder::encode(&rosc::OscPacket::Bundle(blend_bundle)).expect("encode blend");
		sender.send_to(&blend_data, bound).expect("send blend");

		// 3) Apply bundle (Waidayo の "commit" シグナル)
		let apply_bundle = rosc::OscBundle {
			timetag: rosc::OscTime { seconds: 0, fractional: 1 },
			content: vec![rosc::OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Apply".to_string(),
				args: vec![],
			})],
		};
		let apply_data = encoder::encode(&rosc::OscPacket::Bundle(apply_bundle)).expect("encode apply");
		sender.send_to(&apply_data, bound).expect("send apply");

		// poll で 1 つの統合 UNMotionFrame が返るまで待つ。複数 bundle が 1 batch
		// に乗ることも、3 batch に分かれて 3 回 next_frame() を要することもある。
		let deadline = Instant::now() + Duration::from_secs(2);
		let mut last_frame: Option<UNMotionFrame> = None;
		while Instant::now() < deadline {
			match source.next_frame().expect("next_frame") {
				Some(frame) => {
					last_frame = Some(frame);
					// 3 datagram 全部読み込んだ後に届く最後の frame には bone と
					// blendshape の両方が乗っているはず。判定は loop 後に行う。
					if source.telemetry.atomics.raw_received.load(Ordering::Relaxed) >= 3 && !source.pending_dirty {
						break;
					}
				}
				None => std::thread::sleep(Duration::from_millis(20)),
			}
		}

		let snap = source.telemetry.atomics.snapshot();
		assert!(snap.raw_received >= 3, "raw_received={} (expected ≥3)", snap.raw_received);
		assert!(snap.frames_emitted >= 1, "frames_emitted={}", snap.frames_emitted);
		assert_eq!(snap.decode_errors, 0);
		assert_eq!(snap.non_vmc_dropped, 0);

		// accumulator は 2 bones + 12 blendshapes を保持し続ける (emit してもクリアしない)
		assert_eq!(source.pending_bones.len(), 2, "pending_bones should retain Head+Hips after emit");
		assert_eq!(
			source.pending_blendshapes.len(),
			12,
			"pending_blendshapes should retain 12 Vals after emit"
		);

		// 最終 emit された UNMotionFrame は bones も blendshapes も両方持っている
		// (= split bundles が 1 frame に統合された)。
		let frame = last_frame.expect("expected at least one UNMotionFrame");
		let body = frame.body.expect("body present");
		let humanoid = body.humanoid.expect("humanoid present");
		assert_eq!(humanoid.bones.len(), 2, "humanoid.bones should have Head+Hips");
		let face = frame.face.expect("face present");
		assert_eq!(face.expressions.len(), 12, "face.expressions should have 12 blendshapes");
	}

	/// `/MP/` (Waidayo MotionPath 拡張) datagram は silent drop されるが、
	/// `non_vmc_dropped` カウンタは加算される。これによりログに
	/// 「VMC 受信 engine dropped non-VMC datagrams」が出てユーザーが
	/// 「Waidayo は送ってきているが MP 拡張だけだ」を視認できる。
	#[test]
	fn vmc_unmotion_source_counts_non_vmc_dropped_for_waidayo_mp_extension() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
		let mut source = VmcUnmotionSource::bind("test-mp-drop", listen_addr).expect("bind");
		let bound = source.local_addr().expect("local_addr");
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender bind");
		// `/MP/...` で始まる Waidayo MotionPath 拡張 datagram。
		// 内部の `is_unsupported_waidayo_mp_datagram` で受信 → drop される。
		// content 自体は不正な OSC でも構わない (decode より前の prefix check で落ちる)。
		let datagram = b"/MP/SomethingWaidayo\0\0\0\0,sf\0\x00\x00\x00\x00\x00";
		sender.send_to(datagram, bound).expect("send mp datagram");

		let deadline = Instant::now() + Duration::from_secs(1);
		while Instant::now() < deadline {
			let _ = source.next_frame().expect("next_frame");
			if source.telemetry.atomics.raw_received.load(Ordering::Relaxed) >= 1 {
				break;
			}
			std::thread::sleep(Duration::from_millis(10));
		}
		let snap = source.telemetry.atomics.snapshot();
		assert!(snap.raw_received >= 1, "raw_received={}", snap.raw_received);
		assert!(snap.non_vmc_dropped >= 1, "non_vmc_dropped={}", snap.non_vmc_dropped);
		assert!(source.announced_first_non_vmc);
		// VMC payload は無かったので frame は emit されない。
		assert_eq!(snap.frames_emitted, 0);
	}

	/// Phase E telemetry: `MotionFrameSource::telemetry_handle()` 経由で取った
	/// ロックフリーカウンタは inherent な `next_frame()` の write を観測できる。
	/// Capturer の runtime loop は worker thread を停止せずにこのカウンタを
	/// `load(Relaxed)` で読むので、Mutex を使わない設計を回帰防止する。
	#[test]
	fn vmc_unmotion_source_telemetry_handle_observes_lockfree_counters() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
		let mut source = VmcUnmotionSource::bind("test-telemetry", listen_addr).expect("bind");
		let bound = source.local_addr().expect("local_addr");
		let handle: SourceTelemetryHandle = MotionFrameSource::telemetry_handle(&source).expect("handle");
		assert_eq!(handle.kind, "vmc-receive");
		assert_eq!(handle.source_id, "test-telemetry");
		assert_eq!(handle.atomics.snapshot(), SourceStageCounters::default());

		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender bind");
		let bundle = rosc::OscBundle {
			timetag: rosc::OscTime { seconds: 0, fractional: 1 },
			content: vec![rosc::OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Bone/Pos".to_string(),
				args: vec![
					OscType::String("Head".to_string()),
					OscType::Float(0.0),
					OscType::Float(1.5),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(1.0),
				],
			})],
		};
		let datagram = encoder::encode(&rosc::OscPacket::Bundle(bundle)).expect("encode");
		sender.send_to(&datagram, bound).expect("send");

		let deadline = Instant::now() + Duration::from_secs(1);
		while Instant::now() < deadline {
			if source.next_frame().expect("next_frame").is_some() {
				break;
			}
			std::thread::sleep(Duration::from_millis(10));
		}

		// ハンドル経由のスナップショットが inherent な `total_*` 相当の値を
		// 観測できている。
		let snap = handle.atomics.snapshot();
		assert!(snap.raw_received >= 1, "raw_received={}", snap.raw_received);
		assert_eq!(snap.frames_emitted, 1);
		assert_eq!(snap.decode_errors, 0);
		assert_eq!(snap.non_vmc_dropped, 0);
	}

	#[test]
	fn maps_known_bone_names() {
		assert_eq!(humanoid_bone_from_name("Hips"), Some(HumanoidBone::Hips));
		assert_eq!(humanoid_bone_from_name("LeftHand"), Some(HumanoidBone::LeftHand));
		assert_eq!(humanoid_bone_from_name("RightToes"), Some(HumanoidBone::RightToes));
		assert_eq!(humanoid_bone_from_name("Jaw"), Some(HumanoidBone::Jaw));
	}

	#[test]
	fn drops_unknown_bone_names() {
		assert_eq!(humanoid_bone_from_name("LeftThumbProximal"), None);
		assert_eq!(humanoid_bone_from_name(""), None);
		assert_eq!(humanoid_bone_from_name("UnknownBone"), None);
	}

	#[test]
	fn converts_root_and_bones() {
		let vmc_frame = VmcInputFrame {
			source_id: "test".to_string(),
			received_timestamp_ns: 1_234_567_890,
			raw_datagram: None,
			ok: Some(1),
			root: Some(VmcTransform {
				name: "root".to_string(),
				position: [0.1, 1.6, 0.0],
				rotation: [0.0, 0.0, 0.0, 1.0],
			}),
			bones: vec![
				VmcBoneSample {
					name: "Hips".to_string(),
					position: [0.0, 1.0, 0.0],
					rotation: [0.0, 0.0, 0.0, 1.0],
				},
				VmcBoneSample {
					name: "Head".to_string(),
					position: [0.0, 0.4, 0.0],
					rotation: [0.0, 0.1, 0.0, 0.9949874],
				},
				VmcBoneSample {
					name: "UnknownBone".to_string(),
					position: [0.0; 3],
					rotation: [0.0, 0.0, 0.0, 1.0],
				},
			],
			blendshapes: Vec::new(),
			blend_apply: false,
			message_count: 5,
		};
		let unmotion = vmc_input_frame_to_unmotion_frame(&vmc_frame, 42);
		assert_eq!(unmotion.header.sequence, 42);
		assert_eq!(unmotion.header.frame_timestamp_ns, 1_234_567_890);
		let body = unmotion.body.as_ref().expect("body");
		let humanoid = body.humanoid.as_ref().expect("humanoid");
		let root = humanoid.root.as_ref().expect("root");
		assert!((root.translation.unwrap().y - 1.6).abs() < 1e-5);
		// 未知 bone は捨てて 2 件のみ残る (Hips + Head)
		let bones: Vec<_> = humanoid.bones.iter().map(|b| b.bone).collect();
		assert_eq!(bones, vec![HumanoidBone::Hips, HumanoidBone::Head]);
	}

	#[test]
	fn converts_blendshapes_to_face_expressions() {
		let vmc_frame = VmcInputFrame {
			source_id: "test".to_string(),
			received_timestamp_ns: 0,
			raw_datagram: None,
			ok: Some(1),
			root: None,
			bones: Vec::new(),
			blendshapes: vec![
				VmcBlendshapeSample {
					name: "Joy".to_string(),
					value: 0.7,
				},
				VmcBlendshapeSample {
					name: "Blink_L".to_string(),
					value: 0.3,
				},
			],
			blend_apply: true,
			message_count: 3,
		};
		let unmotion = vmc_input_frame_to_unmotion_frame(&vmc_frame, 0);
		let face = unmotion.face.as_ref().expect("face");
		assert_eq!(face.expressions.len(), 2);
		assert_eq!(face.expressions[0].name, "Joy");
		assert!((face.expressions[0].value - 0.7).abs() < 1e-5);
		assert_eq!(face.expressions[1].name, "Blink_L");
	}

	#[test]
	fn empty_payload_yields_empty_unmotion_frame() {
		// payload が全部 None / empty の場合、body / face が None になる。
		let vmc_frame = VmcInputFrame {
			source_id: "test".to_string(),
			received_timestamp_ns: 0,
			raw_datagram: None,
			ok: None,
			root: None,
			bones: Vec::new(),
			blendshapes: Vec::new(),
			blend_apply: false,
			message_count: 0,
		};
		let unmotion = vmc_input_frame_to_unmotion_frame(&vmc_frame, 0);
		assert!(unmotion.body.is_none());
		assert!(unmotion.face.is_none());
	}
}
