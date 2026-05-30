use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

mod cadence;
mod ifacialmocap_unmotion_source;
mod modifier;
mod output;
mod signal_enrich;
mod unmotion;
mod vmc_unmotion_source;
mod zenoh_output;

pub use cadence::OutputCadence;
pub use ifacialmocap_unmotion_source::IfacialMocapUnmotionSource;
pub use modifier::{
	BoneFilterStage, BoneSubsetConfig, MirrorConfig, MirrorMode, MirrorStage, ModifierConfig, ModifierPipeline, ModifierStage,
	NeutralCalibrationConfig, SmoothingConfig, SmoothingPreset, SmoothingStage, StageKind, default_stage_order,
};
pub use output::{
	FileOutputCommand, FileOutputConfig, FileOutputEvent, FileOutputFormat, FileOutputStats, FileOutputWorker, FileOutputWorkerHandle,
	VmcOutputCommand, VmcOutputConfig, VmcOutputEvent, VmcOutputFrame, VmcOutputStats, VmcOutputWorker, VmcOutputWorkerHandle,
	VrcOscOutputCommand, VrcOscOutputConfig, VrcOscOutputEvent, VrcOscOutputFrame, VrcOscOutputStats, VrcOscOutputWorker,
	VrcOscOutputWorkerHandle, spawn_file_output_worker, spawn_vmc_output_worker, spawn_vrc_osc_output_worker,
};
pub use unmotion::{
	LatestMotionFrame, LatestMotionFrameSlot, LatestMotionFrameStreamWorkerHandle, MotionFrameSource, MotionFrameStreamConfig,
	MotionFrameStreamPoll, MotionFrameStreamWorker, MotionFrameStreamWorkerHandle, MotionFrameStreamWorkerMessage,
	apply_unmotion_frame_to_stream_state, spawn_latest_motion_frame_stream_worker, spawn_motion_frame_stream_worker,
	stream_state_from_unmotion_frame,
};
pub use vmc_unmotion_source::{VmcUnmotionSource, humanoid_bone_from_name, vmc_input_frame_to_unmotion_frame};
pub use zenoh_output::{
	ZenohOutputCommand, ZenohOutputConfig, ZenohOutputEvent, ZenohOutputFrame, ZenohOutputStats, ZenohOutputWorker,
	ZenohOutputWorkerHandle, spawn_zenoh_output_worker, vmc_bone_name_to_humanoid_bone,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProfileId(pub String);

impl ProfileId {
	pub fn new(value: impl Into<String>) -> Self {
		Self(value.into())
	}
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StreamId(pub String);

impl StreamId {
	pub fn new(value: impl Into<String>) -> Self {
		Self(value.into())
	}
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeCommand {
	ActivateProfile(ProfileId),
	Start,
	Stop,
	Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RuntimeEvent {
	ProfileActivated { profile_id: ProfileId },
	Started { profile_id: ProfileId },
	Stopped { reason: StopReason },
	StreamUpdated { stream_id: StreamId, updated_at_ns: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
	User,
	ProfileChanged,
	Shutdown,
	Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeState {
	Stopped,
	Starting,
	Running,
	Stopping,
	Failed(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
	pub state: RuntimeState,
	pub active_profile_id: Option<ProfileId>,
	pub generated_at_ns: u64,
	pub streams: Vec<StreamSnapshot>,
	/// 出力ステージ (Zenoh / VMC) の送信側 telemetry。Phase E e2e Step A で追加された。
	#[serde(default)]
	pub output_telemetry: OutputTelemetry,
}

impl RuntimeSnapshot {
	pub fn stopped(generated_at_ns: u64) -> Self {
		Self {
			state: RuntimeState::Stopped,
			active_profile_id: None,
			generated_at_ns,
			streams: Vec::new(),
			output_telemetry: OutputTelemetry::default(),
		}
	}
}

/// Capturer の出力ステージ (Zenoh / VMC) のテレメトリ集約。
///
/// Phase E で「送信側で詰まっているのか / 受信側で届いていないのか」を Supervisor から
/// 目視で切り分けるために導入した。`RuntimeSnapshot` に埋め込まれ、`/api/runtime/snapshot`
/// から Supervisor 経由で GUI まで届く。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct OutputTelemetry {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zenoh: Option<ZenohOutputTelemetry>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vmc: Option<VmcOutputTelemetry>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vrc_osc: Option<VrcOscOutputTelemetry>,
	/// 各 source / engine ステージから集めた累積カウンタ。Capturer の runtime loop
	/// は `refresh_source_telemetry()` で `Arc<SourceStageAtomics>` を `load(Relaxed)`
	/// するだけなので、source 側 worker と排他にならない (ロックフリー)。Supervisor 側
	/// で前回サンプルとの差分から FPS を算出する。
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub sources: Vec<SourceStageTelemetry>,
}

/// Source / engine ステージのロックフリー累積カウンタ。
///
/// `MotionFrameSource` 実装が所有し、Capturer の runtime loop に `Arc` クローン
/// として公開する。worker thread は `fetch_add(Relaxed)` で更新し、runtime loop は
/// `load(Relaxed)` で読むだけなので mutex / channel が不要。
///
/// FPS の計算は Supervisor 側で「2 サンプル間の差 / 経過秒数」で行う。Capturer 内では
/// 単純な monotonically increasing なカウンタを公開するだけにとどめる。
#[derive(Debug, Default)]
pub struct SourceStageAtomics {
	/// 受信した「未加工 1 単位」の数。VMC ならば UDP datagram、Webcam ならば
	/// captured camera frame、iFacialMocap ならば TCP/UDP datagram。
	pub raw_received: AtomicU64,
	/// `MotionFrameSource::next_frame()` から `Some(UNMotionFrame)` として
	/// emit された数。
	pub frames_emitted: AtomicU64,
	/// `raw_received` のうち、ステージ内で merge された bundle の数。
	/// VMC のように 1 frame を複数 bundle に分割する protocol で使う。
	/// それ以外は 0。
	pub bundles_merged: AtomicU64,
	/// decode 失敗した datagram / frame の数。
	pub decode_errors: AtomicU64,
	/// このステージが「自分のプロトコルではない」と判断して drop した数
	/// (例: VMC 受信で Waidayo の `/MP/` MotionPath 拡張を捨てた数)。
	pub non_vmc_dropped: AtomicU64,
	/// 入力 backend が報告する実 source fps。小数を避けるため milli-fps で保持する。
	pub observed_source_fps_milli: AtomicU64,
	/// MediaPipe LIVE_STREAM の native callback 累積数。複数 branch が有効な場合は、
	/// そのフレーム成立を律速する branch の callback 数を入れる。
	pub native_callbacks: AtomicU64,
	/// MediaPipe LIVE_STREAM に submit した累積数。複数 branch が有効な場合は、
	/// フレーム成立を律速する branch と同じ branch の submit 数を入れる。
	pub native_submissions: AtomicU64,
	/// MediaPipe LIVE_STREAM submit が error status を返した累積数。
	pub native_submission_errors: AtomicU64,
	/// LIVE_STREAM poll が「まだ新しい callback がない」と返した回数。
	pub live_stream_poll_misses: AtomicU64,
}

impl SourceStageAtomics {
	pub fn snapshot(&self) -> SourceStageCounters {
		SourceStageCounters {
			raw_received: self.raw_received.load(Ordering::Relaxed),
			frames_emitted: self.frames_emitted.load(Ordering::Relaxed),
			bundles_merged: self.bundles_merged.load(Ordering::Relaxed),
			decode_errors: self.decode_errors.load(Ordering::Relaxed),
			non_vmc_dropped: self.non_vmc_dropped.load(Ordering::Relaxed),
			observed_source_fps_milli: self.observed_source_fps_milli.load(Ordering::Relaxed),
			native_callbacks: self.native_callbacks.load(Ordering::Relaxed),
			native_submissions: self.native_submissions.load(Ordering::Relaxed),
			native_submission_errors: self.native_submission_errors.load(Ordering::Relaxed),
			live_stream_poll_misses: self.live_stream_poll_misses.load(Ordering::Relaxed),
		}
	}
}

/// `SourceStageAtomics` を serialize 可能なスナップショットに落としたもの。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStageCounters {
	#[serde(default)]
	pub raw_received: u64,
	#[serde(default)]
	pub frames_emitted: u64,
	#[serde(default)]
	pub bundles_merged: u64,
	#[serde(default)]
	pub decode_errors: u64,
	#[serde(default)]
	pub non_vmc_dropped: u64,
	#[serde(default)]
	pub observed_source_fps_milli: u64,
	#[serde(default)]
	pub native_callbacks: u64,
	#[serde(default)]
	pub native_submissions: u64,
	#[serde(default)]
	pub native_submission_errors: u64,
	#[serde(default)]
	pub live_stream_poll_misses: u64,
}

/// 1 つの source / engine ステージから集めたテレメトリ。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStageTelemetry {
	/// ステージ種別。Supervisor の UI 上の表示に使う。例: `vmc-receive`,
	/// `webcam-directshow`, `webcam-mediafoundation`, `mediapipe`.
	pub kind: String,
	/// 所属 stream の id (`StreamId.0`)。
	pub stream_id: String,
	/// `UNMotionFrame.metadata.source.id` と一致する識別子。
	pub source_id: String,
	#[serde(flatten)]
	pub counters: SourceStageCounters,
}

/// `Arc<SourceStageAtomics>` を kind / source_id とセットで配布するためのハンドル。
/// `MotionFrameSource::telemetry_handle()` で worker spawn 時に取得し、Capturer
/// runtime loop が `runtime_snapshot` 直前にこれを `load` してテレメトリに反映する。
#[derive(Clone, Debug)]
pub struct SourceTelemetryHandle {
	pub kind: String,
	pub source_id: String,
	pub atomics: Arc<SourceStageAtomics>,
	pub debug: Arc<SourceDebugRecorder>,
}

impl SourceTelemetryHandle {
	pub fn new(kind: impl Into<String>, source_id: impl Into<String>) -> Self {
		Self {
			kind: kind.into(),
			source_id: source_id.into(),
			atomics: Arc::new(SourceStageAtomics::default()),
			debug: Arc::new(SourceDebugRecorder::default()),
		}
	}

	pub fn snapshot_with_stream(&self, stream_id: impl Into<String>) -> SourceStageTelemetry {
		SourceStageTelemetry {
			kind: self.kind.clone(),
			stream_id: stream_id.into(),
			source_id: self.source_id.clone(),
			counters: self.atomics.snapshot(),
		}
	}
}

#[derive(Debug, Default)]
pub struct SourceDebugRecorder {
	state: Mutex<SourceDebugState>,
}

#[derive(Debug, Default)]
struct SourceDebugState {
	active: bool,
	output_dir: Option<PathBuf>,
	max_samples: usize,
	samples: VecDeque<serde_json::Value>,
}

impl SourceDebugRecorder {
	pub fn begin_capture(&self, output_dir: impl Into<PathBuf>, max_samples: usize) {
		if let Ok(mut state) = self.state.lock() {
			state.active = true;
			state.output_dir = Some(output_dir.into());
			state.max_samples = max_samples.max(1);
			state.samples.clear();
		}
	}

	pub fn finish_capture(&self) -> Vec<serde_json::Value> {
		if let Ok(mut state) = self.state.lock() {
			state.active = false;
			state.output_dir = None;
			return state.samples.drain(..).collect();
		}
		Vec::new()
	}

	pub fn is_active(&self) -> bool {
		self.state.lock().map(|state| state.active).unwrap_or(false)
	}

	pub fn output_dir(&self) -> Option<PathBuf> {
		self.state.lock().ok().and_then(|state| state.output_dir.clone())
	}

	pub fn push_sample(&self, sample: serde_json::Value) {
		if let Ok(mut state) = self.state.lock() {
			if !state.active {
				return;
			}
			while state.samples.len() >= state.max_samples {
				state.samples.pop_front();
			}
			state.samples.push_back(sample);
		}
	}

	pub fn output_dir_exists(&self) -> bool {
		self.output_dir().as_deref().is_some_and(Path::exists)
	}
}

/// Zenoh 出力ステージのテレメトリ。`sent_frames` は累積カウンタ。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ZenohOutputTelemetry {
	/// 起動時に確定する `base_key_expr` (例: `unmotion/v1/...`)。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub base_key_expr: Option<String>,
	/// 累積送信フレーム数。
	#[serde(default)]
	pub sent_frames: u64,
	/// 累積エラー数。
	#[serde(default)]
	pub error_count: u64,
	/// 直近の send エラー文字列 (任意)。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_error: Option<String>,
}

/// VMC 出力ステージのテレメトリ。`sent_datagrams` は UDP datagram 単位、
/// `sent_packets` は OSC packet 単位 (bundle 1 つに複数 packet が入り得る)。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VmcOutputTelemetry {
	/// 送信先 (例: `127.0.0.1:39539`)。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub target_addr: Option<String>,
	/// 累積送信 datagram 数。
	#[serde(default)]
	pub sent_datagrams: u64,
	/// 累積送信 OSC packet 数。
	#[serde(default)]
	pub sent_packets: u64,
	/// 累積エラー数。
	#[serde(default)]
	pub error_count: u64,
	/// 直近の send エラー文字列 (任意)。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VrcOscOutputTelemetry {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub target_addr: Option<String>,
	#[serde(default)]
	pub vrchat_detected: bool,
	#[serde(default)]
	pub sent_datagrams: u64,
	#[serde(default)]
	pub sent_packets: u64,
	#[serde(default)]
	pub skipped_frames: u64,
	#[serde(default)]
	pub process_gate_blocked_frames: u64,
	#[serde(default)]
	pub error_count: u64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformSample {
	pub name: String,
	pub position: [f32; 3],
	pub rotation: [f32; 4],
}

impl TransformSample {
	pub fn new(name: impl Into<String>, position: [f32; 3], rotation: [f32; 4]) -> Self {
		Self {
			name: name.into(),
			position,
			rotation,
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamHealth {
	Disabled,
	NoSignal,
	Live,
	Stale { stale_for_ns: u64 },
	DecodeError,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamState {
	pub stream_id: StreamId,
	pub enabled: bool,
	pub first_packet_at_ns: Option<u64>,
	pub last_packet_at_ns: Option<u64>,
	pub last_event_at_ns: Option<u64>,
	pub packet_count: u64,
	pub event_count: u64,
	pub decode_error_count: u64,
	pub root: Option<TransformSample>,
	pub bones: BTreeMap<String, TransformSample>,
	pub blendshapes: BTreeMap<String, f32>,
	pub part_diagnostics: BTreeMap<MotionPart, PartDiagnostic>,
}

impl StreamState {
	pub fn new(stream_id: StreamId) -> Self {
		Self {
			stream_id,
			enabled: true,
			first_packet_at_ns: None,
			last_packet_at_ns: None,
			last_event_at_ns: None,
			packet_count: 0,
			event_count: 0,
			decode_error_count: 0,
			root: None,
			bones: BTreeMap::new(),
			blendshapes: BTreeMap::new(),
			part_diagnostics: BTreeMap::new(),
		}
	}

	pub fn health_at(&self, now_ns: u64, stale_after_ns: u64) -> StreamHealth {
		if !self.enabled {
			return StreamHealth::Disabled;
		}
		if self.decode_error_count > 0 && self.event_count == 0 {
			return StreamHealth::DecodeError;
		}
		let Some(last_event_at_ns) = self.last_event_at_ns else {
			return StreamHealth::NoSignal;
		};
		let elapsed_ns = now_ns.saturating_sub(last_event_at_ns);
		if elapsed_ns > stale_after_ns {
			StreamHealth::Stale {
				stale_for_ns: elapsed_ns - stale_after_ns,
			}
		} else {
			StreamHealth::Live
		}
	}

	pub fn snapshot_at(&self, now_ns: u64, stale_after_ns: u64) -> StreamSnapshot {
		StreamSnapshot {
			stream_id: self.stream_id.clone(),
			health: self.health_at(now_ns, stale_after_ns),
			last_packet_at_ns: self.last_packet_at_ns,
			last_event_at_ns: self.last_event_at_ns,
			packet_count: self.packet_count,
			event_count: self.event_count,
			decode_error_count: self.decode_error_count,
			bone_count: self.bones.len(),
			blendshape_count: self.blendshapes.len(),
			part_diagnostics: self.part_diagnostics.clone(),
		}
	}
}

pub fn upsert_part_diagnostic(
	diagnostics: &mut BTreeMap<MotionPart, PartDiagnostic>,
	part: MotionPart,
	status: PartTrackingStatus,
	confidence: f32,
) {
	diagnostics
		.entry(part)
		.and_modify(|diagnostic| diagnostic.merge(status, confidence))
		.or_insert_with(|| PartDiagnostic::new(part, status, confidence));
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamSnapshot {
	pub stream_id: StreamId,
	pub health: StreamHealth,
	pub last_packet_at_ns: Option<u64>,
	pub last_event_at_ns: Option<u64>,
	pub packet_count: u64,
	pub event_count: u64,
	pub decode_error_count: u64,
	pub bone_count: usize,
	pub blendshape_count: usize,
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub part_diagnostics: BTreeMap<MotionPart, PartDiagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MotionPart {
	Head,
	Face,
	Eyes,
	LeftHand,
	RightHand,
	LeftArm,
	RightArm,
	Torso,
	LeftLeg,
	RightLeg,
	LeftFoot,
	RightFoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PartTrackingStatus {
	Estimated,
	Held,
	Recovering,
	Lost,
	Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartDiagnostic {
	pub group: MotionPart,
	pub status: PartTrackingStatus,
	pub confidence: f32,
}

impl PartDiagnostic {
	pub fn new(group: MotionPart, status: PartTrackingStatus, confidence: f32) -> Self {
		Self {
			group,
			status,
			confidence: confidence.clamp(0.0, 1.0),
		}
	}

	pub fn merge(&mut self, status: PartTrackingStatus, confidence: f32) {
		self.status = merge_part_status(self.status, status);
		self.confidence = self.confidence.max(confidence.clamp(0.0, 1.0));
	}
}

fn merge_part_status(left: PartTrackingStatus, right: PartTrackingStatus) -> PartTrackingStatus {
	use PartTrackingStatus::*;
	match (left, right) {
		(Estimated, _) | (_, Estimated) => Estimated,
		(Recovering, _) | (_, Recovering) => Recovering,
		(Held, _) | (_, Held) => Held,
		(Lost, Off) | (Off, Lost) => Lost,
		(Off, Off) => Off,
		(Lost, Lost) => Lost,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn stream_id() -> StreamId {
		StreamId::new("stream-a")
	}

	#[test]
	fn stale_health_uses_last_event_timestamp() {
		let mut state = StreamState::new(stream_id());
		state.last_event_at_ns = Some(100);

		assert_eq!(state.health_at(149, 50), StreamHealth::Live);
		assert_eq!(state.health_at(151, 50), StreamHealth::Stale { stale_for_ns: 1 });
	}

	#[test]
	fn snapshot_reports_frame_stream_counters() {
		let mut state = StreamState::new(stream_id());
		state.packet_count = 1;
		state.last_packet_at_ns = Some(7);

		let snapshot = state.snapshot_at(100, 50);

		assert_eq!(snapshot.packet_count, 1);
		assert_eq!(snapshot.last_packet_at_ns, Some(7));
		assert_eq!(snapshot.health, StreamHealth::NoSignal);
	}
}
