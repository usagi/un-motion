use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{
	Arc,
	mpsc::{self, Receiver, Sender, TryRecvError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use un_motion_frame::UNMotionFrame;
use un_motion_frame_zenoh::TopicMode;
use un_motion_profile_schema::{
	CoreProfileDocument, CoreProfileDocumentProfile, CoreProfileDocumentSource, ProfileMediaPipeAdvancedSettings, ProfileModifierSettings,
	ProfilePipelineComponents, ProfileRuntimeSettings,
};
use un_motion_runtime::{
	LatestMotionFrame, LatestMotionFrameStreamWorkerHandle, MirrorMode, ModifierConfig, ModifierPipeline, MotionFrameStreamConfig,
	OutputCadence, OutputTelemetry, ProfileId, RuntimeSnapshot, RuntimeState, SmoothingPreset, StreamId, StreamState, VmcOutputConfig,
	VmcOutputEvent, VmcOutputTelemetry, VmcOutputWorkerHandle, VrcOscOutputConfig, VrcOscOutputEvent, VrcOscOutputTelemetry,
	VrcOscOutputWorkerHandle, ZenohOutputConfig, ZenohOutputEvent, ZenohOutputTelemetry, ZenohOutputWorkerHandle,
	spawn_latest_motion_frame_stream_worker, spawn_vmc_output_worker, spawn_vrc_osc_output_worker, spawn_zenoh_output_worker,
};

use crate::unmotion_source::open_motion_frame_source;

const DEFAULT_FPS: u32 = 30;
const MAX_FPS: u32 = 240;
const FOLLOW_INPUT_OUTPUT_FPS_CAP: u32 = MAX_FPS;
const DEFAULT_VMC_TARGET: &str = "127.0.0.1:39539";
const DEFAULT_VRC_OSC_TARGET: &str = "127.0.0.1:9000";
const STALE_AFTER_NS: u64 = 500_000_000;
const LOOP_MAX_SLEEP: Duration = Duration::from_millis(5);
const FLOW_IDLE_SLEEP: Duration = Duration::from_millis(1);
const BUILTIN_UNMOTION_FLOW_ID: &str = "mediapipe-main";

type StatusCallback = Arc<dyn Fn(CoreRuntimeStatusUpdate) + Send + Sync + 'static>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreRuntimeStatusUpdate {
	pub active_profile_id: String,
	pub running: bool,
	pub health: String,
	pub frame_count: u64,
	pub packet_count: u64,
	pub runtime_snapshot: Option<RuntimeSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreRuntimeConfig {
	pub active_profile_id: String,
	pub fps: u32,
	pub vmc_output: Option<CoreVmcOutputConfig>,
	pub vrc_osc_output: Option<CoreVrcOscOutputConfig>,
	pub zenoh_output: Option<CoreZenohOutputConfig>,
	pub frame_streams: Vec<CoreMotionFrameStreamConfig>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreVmcOutputConfig {
	pub target_addr: SocketAddr,
	/// Profile の `runtime_selection.modifier.*_enabled` から派生する Modifier 設定。
	/// 正式経路では VMC 出力も post-process 済み `UNMotionFrame` を受け、Modifier
	/// 適用後に OSC へ変換する。
	pub modifier: ModifierConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreVrcOscOutputConfig {
	pub target_addr: SocketAddr,
	pub parameter_prefix: String,
	pub send_only_when_vrchat_running: bool,
	pub process_poll_interval: Duration,
	pub modifier: ModifierConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreZenohOutputConfig {
	pub base_key_expr: String,
	pub topic_mode: CoreZenohTopicMode,
	pub stream_id: Option<String>,
	pub producer: Option<String>,
	/// Profile の `runtime_selection.modifier.*_enabled` から派生する Modifier 設定。
	/// VMC/UDP と UNMF/Z は同じ `UNMotionFrame` に同じ Modifier を適用する。
	pub modifier: ModifierConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CoreZenohTopicMode {
	#[default]
	Frame,
	ByPrimarySource,
	ByStreamId,
}

impl CoreZenohTopicMode {
	pub(crate) fn from_str(value: &str) -> Self {
		match value.to_ascii_lowercase().replace('_', "-").as_str() {
			"by-primary-source" | "primary-source" | "primarysource" => Self::ByPrimarySource,
			"by-stream-id" | "stream-id" | "streamid" => Self::ByStreamId,
			_ => Self::Frame,
		}
	}

	#[allow(dead_code)]
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Frame => "frame",
			Self::ByPrimarySource => "by-primary-source",
			Self::ByStreamId => "by-stream-id",
		}
	}

	pub(crate) fn to_zenoh_mode(self) -> TopicMode {
		match self {
			Self::Frame => TopicMode::Frame,
			Self::ByPrimarySource => TopicMode::ByPrimarySource,
			Self::ByStreamId => TopicMode::ByStreamId,
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreMotionFrameStreamConfig {
	pub profile_stream_id: String,
	pub stream_id: StreamId,
	/// MediaPipe Webcam engine 専用: DirectShow / nokhwa デバイス識別子。
	/// Engine Type が `mediapipe-*` 以外のときは空文字 (参照禁止)。
	pub device_id: String,
	pub runtime_engine: String,
	pub input_component: String,
	/// VMC 受信 engine 専用の listen address
	/// (例: `0.0.0.0:39550`)。Engine Type が `vmc` 以外のときは `None`。
	pub vmc_receive_listen_addr: Option<String>,
	/// iFacialMocap 受信 engine 専用の listen address。
	pub ifacialmocap_receive_listen_addr: Option<String>,
	pub input_path: Option<String>,
	pub input_fps: u32,
	pub input_width: Option<u32>,
	pub input_height: Option<u32>,
	pub input_pixel_format: Option<String>,
	pub input_repeat: bool,
	pub input_ffmpeg_path: Option<String>,
	pub input_denoise_mode: String,
	pub input_denoise_temporal_iir_hz: f64,
	pub input_resize: Option<CoreUnmotionResizeConfig>,
	pub media_pipe_running_mode: String,
	pub media_pipe_holistic_enabled: bool,
	pub media_pipe_delegate: Option<String>,
	pub media_pipe_num_threads: Option<u32>,
	pub media_pipe_holistic_flow_limiter_enabled: bool,
	pub media_pipe_holistic_flow_limiter_max_in_flight: u32,
	pub media_pipe_holistic_flow_limiter_max_in_queue: u32,
	pub post_process_component: String,
	pub media_pipe_post_process: CoreMediaPipePostProcessSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreUnmotionResizeConfig {
	pub preserve_aspect_ratio: bool,
	pub axis: String,
	pub reference: u32,
	pub width: u32,
	pub height: u32,
	pub pad_color: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreMediaPipePostProcessSettings {
	pub head_enabled: bool,
	pub face_enabled: bool,
	pub hands_enabled: bool,
	pub arms_ik_enabled: bool,
	pub torso_enabled: bool,
	pub legs_enabled: bool,
	pub feet_enabled: bool,
	pub camera_diagonal_view_angle_deg: f32,
	pub min_landmark_confidence: f32,
	pub eye_open_bias: f32,
	pub mirror_mode: String,
	pub post_process_rules: CoreUnmotionPostProcessRulesConfig,
	pub face_pose_model: Option<CoreFacePoseModelConfig>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreFacePoseModelConfig {
	pub enabled: bool,
	pub neutral_nose_drop_eye_mouth: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreUnmotionPostProcessRulesConfig {
	pub anatomical_constraints: bool,
	pub hold_lost_landmarks: bool,
	pub ease_recovery: bool,
	pub limit_rotation_jumps: bool,
	pub head_source_switch_blend: bool,
	pub lost_signal_behavior: String,
	pub lost_signal_rest_pose_blend: f32,
	pub lost_signal_hold_seconds: f32,
	pub lost_signal_head_behavior: String,
	pub lost_signal_head_rest_pose_blend: f32,
	pub lost_signal_head_hold_seconds: f32,
	pub lost_signal_hands_behavior: String,
	pub lost_signal_hands_rest_pose_blend: f32,
	pub lost_signal_hands_hold_seconds: f32,
	pub lost_signal_arms_behavior: String,
	pub lost_signal_arms_rest_pose_blend: f32,
	pub lost_signal_arms_hold_seconds: f32,
	pub lost_signal_recovery_seconds: f32,
	pub head_from_pose: bool,
	pub head_from_face_matrix: bool,
	pub head_reconcile: bool,
	pub neutral_eye_fallback: bool,
	pub hand_camera_target: bool,
	pub hand_orientation: bool,
	pub finger_derived: bool,
	pub arm_from_pose: bool,
	pub arm_ik_from_hands: bool,
	pub crossed_hand_heuristic: bool,
	pub coordinate_correction: bool,
	pub final_clamp: bool,
}

impl Default for CoreMediaPipePostProcessSettings {
	fn default() -> Self {
		Self {
			head_enabled: true,
			face_enabled: true,
			hands_enabled: false,
			arms_ik_enabled: false,
			torso_enabled: false,
			legs_enabled: false,
			feet_enabled: false,
			camera_diagonal_view_angle_deg: 70.0,
			min_landmark_confidence: 0.55,
			eye_open_bias: 0.5,
			mirror_mode: "normal".to_string(),
			post_process_rules: CoreUnmotionPostProcessRulesConfig::default(),
			face_pose_model: None,
		}
	}
}

impl Default for CoreUnmotionPostProcessRulesConfig {
	fn default() -> Self {
		Self {
			anatomical_constraints: true,
			hold_lost_landmarks: true,
			ease_recovery: true,
			limit_rotation_jumps: true,
			head_source_switch_blend: true,
			lost_signal_behavior: "rest-pose".to_string(),
			lost_signal_rest_pose_blend: 0.3,
			lost_signal_hold_seconds: 8.2,
			lost_signal_head_behavior: "hold".to_string(),
			lost_signal_head_rest_pose_blend: 0.3,
			lost_signal_head_hold_seconds: 8.2,
			lost_signal_hands_behavior: "rest-pose".to_string(),
			lost_signal_hands_rest_pose_blend: 0.3,
			lost_signal_hands_hold_seconds: 8.2,
			lost_signal_arms_behavior: "rest-pose".to_string(),
			lost_signal_arms_rest_pose_blend: 0.3,
			lost_signal_arms_hold_seconds: 8.2,
			lost_signal_recovery_seconds: 0.25,
			head_from_pose: true,
			head_from_face_matrix: true,
			head_reconcile: true,
			neutral_eye_fallback: true,
			hand_camera_target: true,
			hand_orientation: true,
			finger_derived: true,
			arm_from_pose: true,
			arm_ik_from_hands: true,
			crossed_hand_heuristic: true,
			coordinate_correction: true,
			final_clamp: true,
		}
	}
}

#[derive(Debug)]
enum CoreRuntimeControl {
	Stop,
	CalibrateNeutral {
		pose: NeutralCalibrationPose,
		valid_sample_count: usize,
		response_tx: Sender<anyhow::Result<NeutralCalibrationResult>>,
	},
	BuildFacePoseModel {
		valid_sample_count: usize,
		response_tx: Sender<anyhow::Result<FacePoseModelResult>>,
	},
	CaptureUnmotionFrame {
		response_tx: Sender<anyhow::Result<UNMotionFrame>>,
	},
	BeginAnalysisCapture {
		output_dir: PathBuf,
		max_samples: usize,
		response_tx: Sender<anyhow::Result<()>>,
	},
	FinishAnalysisCapture {
		response_tx: Sender<anyhow::Result<Vec<serde_json::Value>>>,
	},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NeutralCalibrationPose {
	U,
	T,
	I,
}

impl NeutralCalibrationPose {
	pub(crate) fn parse(value: Option<&str>) -> anyhow::Result<Self> {
		match value.unwrap_or("U").trim().to_ascii_uppercase().as_str() {
			"" | "U" | "U-POSE" | "U_POSE" => Ok(Self::U),
			"T" | "T-POSE" | "T_POSE" => Ok(Self::T),
			"I" | "I-POSE" | "I_POSE" => Ok(Self::I),
			value => bail!("unsupported calibration pose: {value}; expected U, T, or I"),
		}
	}

	pub(crate) fn as_profile_value(self) -> &'static str {
		match self {
			Self::U => "U",
			Self::T => "T",
			Self::I => "I",
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NeutralCalibrationResult {
	pub valid_samples: usize,
	pub rotations: BTreeMap<String, [f32; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FacePoseModelResult {
	pub valid_samples: usize,
	pub neutral_nose_drop_eye_mouth: f32,
	pub median_abs_yaw: f32,
	pub median_abs_roll: f32,
}

const FACE_POSE_MODEL_MIN_CONFIDENCE: f32 = 0.55;
const FACE_POSE_MODEL_MAX_SAMPLE_ABS_YAW: f32 = 0.50;
const FACE_POSE_MODEL_MAX_SAMPLE_ABS_ROLL: f32 = 0.45;
const FACE_POSE_MODEL_MAX_MEDIAN_ABS_YAW: f32 = 0.22;
const FACE_POSE_MODEL_MAX_MEDIAN_ABS_ROLL: f32 = 0.20;
const FACE_POSE_MODEL_MAX_NOSE_DROP_MAD: f32 = 0.055;

pub(crate) struct CoreRuntimeHost {
	control_tx: Sender<CoreRuntimeControl>,
	join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for CoreRuntimeHost {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("CoreRuntimeHost").finish_non_exhaustive()
	}
}

impl CoreRuntimeHost {
	pub(crate) fn spawn(config: CoreRuntimeConfig, status_callback: StatusCallback) -> anyhow::Result<Self> {
		let (control_tx, control_rx) = mpsc::channel();
		let (init_tx, init_rx) = mpsc::channel();
		let join = thread::spawn(move || run_core_runtime(config, control_rx, init_tx, status_callback));
		match init_rx
			.recv_timeout(Duration::from_secs(5))
			.context("core runtime did not finish startup")?
		{
			Ok(()) => Ok(Self {
				control_tx,
				join: Some(join),
			}),
			Err(error) => {
				let _ = join.join();
				Err(error)
			}
		}
	}

	pub(crate) fn stop(&self) {
		let _ = self.control_tx.send(CoreRuntimeControl::Stop);
	}

	pub(crate) fn calibrate_neutral(
		&self,
		valid_sample_count: usize,
		pose: NeutralCalibrationPose,
	) -> anyhow::Result<NeutralCalibrationResult> {
		let (response_tx, response_rx) = mpsc::channel();
		self.control_tx
			.send(CoreRuntimeControl::CalibrateNeutral {
				pose,
				valid_sample_count,
				response_tx,
			})
			.map_err(|_| anyhow!("core runtime is stopped"))?;
		response_rx
			.recv_timeout(Duration::from_secs(10))
			.context("neutral calibration timed out")?
	}

	pub(crate) fn capture_unmotion_frame(&self) -> anyhow::Result<UNMotionFrame> {
		let (response_tx, response_rx) = mpsc::channel();
		self.control_tx
			.send(CoreRuntimeControl::CaptureUnmotionFrame { response_tx })
			.map_err(|_| anyhow!("core runtime is stopped"))?;
		response_rx
			.recv_timeout(Duration::from_secs(3))
			.context("UNMotionFrame capture timed out")?
	}

	pub(crate) fn capture_analysis_extras(&self, output_dir: PathBuf, duration: Duration) -> anyhow::Result<Vec<serde_json::Value>> {
		fs::create_dir_all(&output_dir).with_context(|| format!("create analysis output dir {}", output_dir.display()))?;
		let max_samples = analysis_extras_max_samples(duration);
		let (begin_tx, begin_rx) = mpsc::channel();
		self.control_tx
			.send(CoreRuntimeControl::BeginAnalysisCapture {
				output_dir,
				max_samples,
				response_tx: begin_tx,
			})
			.map_err(|_| anyhow!("core runtime is stopped"))?;
		begin_rx
			.recv_timeout(Duration::from_secs(3))
			.context("analysis capture begin timed out")??;
		thread::sleep(duration);
		let (finish_tx, finish_rx) = mpsc::channel();
		self.control_tx
			.send(CoreRuntimeControl::FinishAnalysisCapture { response_tx: finish_tx })
			.map_err(|_| anyhow!("core runtime is stopped"))?;
		finish_rx
			.recv_timeout(Duration::from_secs(3))
			.context("analysis capture finish timed out")?
	}

	pub(crate) fn build_face_pose_model(&self, valid_sample_count: usize) -> anyhow::Result<FacePoseModelResult> {
		let (response_tx, response_rx) = mpsc::channel();
		self.control_tx
			.send(CoreRuntimeControl::BuildFacePoseModel {
				valid_sample_count,
				response_tx,
			})
			.map_err(|_| anyhow!("core runtime is stopped"))?;
		response_rx
			.recv_timeout(Duration::from_secs(10))
			.context("face pose model build timed out")?
	}

	pub(crate) fn join(mut self) -> thread::Result<()> {
		self.stop();
		if let Some(join) = self.join.take() { join.join() } else { Ok(()) }
	}
}

fn analysis_extras_max_samples(duration: Duration) -> usize {
	let duration_ms = duration.as_millis() as usize;
	// 最大 120fps 程度を想定し、少し余裕を持たせる。3s 診断でも時間系列を
	// 最後まで残す必要があり、短い上限で切ると周波数解析や応答性確認が壊れる。
	let estimated = duration_ms.saturating_mul(120).saturating_div(1000).saturating_add(240);
	estimated.clamp(120, 10_000)
}

impl Drop for CoreRuntimeHost {
	fn drop(&mut self) {
		self.stop();
	}
}

pub(crate) fn core_runtime_config_from_document(document: &CoreProfileDocument) -> anyhow::Result<CoreRuntimeConfig> {
	let profile = document
		.profiles
		.iter()
		.find(|profile| profile.id == document.selected_profile_id)
		.or_else(|| document.profiles.first())
		.context("profile document contains no profiles")?;
	let runtime = profile.runtime_selection.as_ref();
	let frame_streams = core_frame_stream_configs(profile, &document.profile_sources);
	let fps = runtime.and_then(|r| r.fps).unwrap_or(FOLLOW_INPUT_OUTPUT_FPS_CAP).clamp(1, MAX_FPS);
	let vmc_output = if runtime.and_then(|r| r.vmc_enabled).unwrap_or(false) {
		let target = runtime.and_then(|r| r.vmc_target_addr.as_deref()).unwrap_or(DEFAULT_VMC_TARGET);
		let target_addr: SocketAddr = target
			.parse()
			.with_context(|| format!("invalid VMC output target address: {target}"))?;
		// 一般的な VMC / iFacialMocap pass-through 構成 (Engine Type = "vmc" / "ifacialmocap")
		// で「listen と target が同 host 同 port」になると Capturer 自身の OSC packet が
		// 自分の listener に echo してしまうという既知のハマりどころに対する safety check。
		// 注意: これは「ある種の誤設定」一般に対する diagnostic であって、特定の Waidayo
		// プロファイル不具合の原因究明そのものではない (ユーザーの実機プロファイルは
		// listen 39540 / target 39551 で別 port なのでこの warn は発火しない)。
		if let Some(engine) = runtime.and_then(|r| r.engine.as_deref())
			&& (engine == "vmc" || engine == "ifacialmocap")
		{
			let listen_addr_str = match engine {
				"vmc" => runtime.and_then(|r| r.vmc_receive_listen_addr.as_deref()),
				"ifacialmocap" => runtime.and_then(|r| r.ifacialmocap_receive_listen_addr.as_deref()),
				_ => None,
			};
			let default_listen_port = if engine == "ifacialmocap" { 49983 } else { 39539 };
			let listen_port = listen_addr_str
				.and_then(|s| s.parse::<SocketAddr>().ok())
				.map(|sa| sa.port())
				.unwrap_or(default_listen_port);
			let target_ip = target_addr.ip();
			if target_addr.port() == listen_port && (target_ip.is_loopback() || target_ip.is_unspecified()) {
				tracing::warn!(
					target: "un_motion_core::runtime_host",
					engine,
					target_addr = %target_addr,
					listen_port,
					"VMC output target collides with the receive listen port on the same host (loopback). \
					 This causes an OSC echo loop where the capturer's own packets re-enter the listener, \
					 and downstream apps (VSeeFace etc.) will not receive proper data. \
					 Change Output → VMC target port to something distinct (e.g. VSeeFace receive port 39540), \
					 or disable VMC output in this profile."
				);
			}
		}
		Some(CoreVmcOutputConfig {
			target_addr,
			modifier: modifier_config_from_runtime(runtime),
		})
	} else {
		None
	};
	let vrc_osc_output = parse_vrc_osc_output_config(runtime)?;
	let zenoh_output = parse_zenoh_output_config(runtime, &profile.id);
	Ok(CoreRuntimeConfig {
		active_profile_id: profile.id.clone(),
		fps,
		vmc_output,
		vrc_osc_output,
		zenoh_output,
		frame_streams,
	})
}

fn parse_vrc_osc_output_config(runtime: Option<&ProfileRuntimeSettings>) -> anyhow::Result<Option<CoreVrcOscOutputConfig>> {
	if !runtime.and_then(|r| r.vrc_osc_enabled).unwrap_or(false) {
		return Ok(None);
	}
	let target = runtime
		.and_then(|r| r.vrc_osc_target_addr.as_deref())
		.unwrap_or(DEFAULT_VRC_OSC_TARGET);
	let target_addr: SocketAddr = target
		.parse()
		.with_context(|| format!("invalid VRC OSC output target address: {target}"))?;
	let parameter_prefix = runtime.and_then(|r| r.vrc_osc_parameter_prefix.clone()).unwrap_or_default();
	let send_only_when_vrchat_running = runtime.and_then(|r| r.vrc_osc_send_only_when_vrchat_running).unwrap_or(true);
	let poll_secs = runtime
		.and_then(|r| r.vrc_osc_process_poll_interval_secs)
		.unwrap_or(10)
		.clamp(1, 3600);
	Ok(Some(CoreVrcOscOutputConfig {
		target_addr,
		parameter_prefix,
		send_only_when_vrchat_running,
		process_poll_interval: Duration::from_secs(poll_secs),
		modifier: modifier_config_from_runtime(runtime),
	}))
}

fn parse_zenoh_output_config(runtime: Option<&ProfileRuntimeSettings>, profile_id: &str) -> Option<CoreZenohOutputConfig> {
	if !runtime.and_then(|r| r.zenoh_enabled).unwrap_or(false) {
		return None;
	}
	let base_key_expr = runtime
		.and_then(|r| r.zenoh_key_expr.as_deref())
		.map(|value| value.trim().trim_end_matches('/'))
		.filter(|value| !value.is_empty())
		.unwrap_or("un-motion/frame")
		.to_string();
	let topic_mode = CoreZenohTopicMode::from_str(runtime.and_then(|r| r.zenoh_topic_mode.as_deref()).unwrap_or("frame"));
	let stream_id = runtime
		.and_then(|r| r.zenoh_stream_id.clone())
		.or_else(|| Some(profile_id.to_string()));
	let producer = runtime
		.and_then(|r| r.zenoh_producer.clone())
		.or_else(|| Some("un-motion-core".to_string()));
	let modifier = modifier_config_from_runtime(runtime);
	Some(CoreZenohOutputConfig {
		base_key_expr,
		topic_mode,
		stream_id,
		producer,
		modifier,
	})
}

/// Profile の `runtime_selection.modifier.*_enabled` および `mirror_mode` から
/// Modifier 設定を作る。`modifier` ブロックが存在しない / 個別フラグが未指定の場合は
/// `ModifierConfig::default()` (全 ON = pass-through) にフォールバックする。
///
/// # mirror_mode の移管
///
/// 以前は Engine (`MediaPipePostProcessConfig::mirror_mode`) が signal-level で
/// 反転を行っていたが、bone-transform level の `MirrorStage` に処理を
/// 移管した。`modifier.mirror_mode` の文字列値 (`"normal"` / `"mirror-output"` /
/// `"swap-sides"`) を `MirrorMode` enum にマップし、Engine 側は `core_media_pipe_post_process_settings`
/// で常に `"normal"` (passthrough) に固定される。
fn modifier_config_from_runtime(runtime: Option<&ProfileRuntimeSettings>) -> ModifierConfig {
	let settings = runtime.and_then(|runtime| runtime.modifier.as_ref());
	let mut config = ModifierConfig::default();
	let Some(settings) = settings else {
		return config;
	};
	if let Some(value) = settings.head_enabled {
		config.head_enabled = value;
	}
	if let Some(value) = settings.face_enabled {
		config.face_enabled = value;
	}
	if let Some(value) = settings.hands_enabled {
		config.hands_enabled = value;
	}
	if let Some(value) = settings.arms_ik_enabled {
		config.arms_ik_enabled = value;
	}
	if let Some(value) = settings.torso_enabled {
		config.torso_enabled = value;
	}
	if let Some(value) = settings.legs_enabled {
		config.legs_enabled = value;
	}
	if let Some(value) = settings.feet_enabled {
		config.feet_enabled = value;
	}
	if let Some(value) = settings.torso_pitch_scale {
		config.torso_pitch_scale = value.clamp(0.0, 1.0);
	}
	if let Some(value) = settings.neutral_calibration_enabled {
		config.neutral_calibration.enabled = value;
	}
	if let Some(value) = settings.neutral_calibration_rotations.as_ref() {
		config.neutral_calibration.rotations = value.iter().map(|(key, rotation)| (key.clone(), *rotation)).collect();
	}
	if let Some(value) = settings.mirror_mode.as_deref() {
		config.mirror.mode = match value {
			"mirror-output" => MirrorMode::MirrorOutput,
			"swap-sides" => MirrorMode::SwapSides,
			_ => MirrorMode::Normal,
		};
	}
	if let Some(value) = settings.smoothing_preset.as_deref() {
		config.smoothing.preset = parse_smoothing_preset(value);
	}
	if let Some(value) = settings.smoothing_ema_enabled {
		config.smoothing.ema_enabled = value;
	}
	if let Some(value) = settings.smoothing_ema_alpha {
		config.smoothing.ema_alpha = value;
	}
	if let Some(value) = settings.smoothing_one_euro_enabled {
		config.smoothing.one_euro_enabled = value;
	}
	if let Some(value) = settings.smoothing_confidence_adaptive_cutoff {
		config.smoothing.confidence_adaptive_cutoff_enabled = value;
	}
	if let Some(value) = settings.adaptive_min_cutoff_hz {
		config.smoothing.adaptive_min_cutoff_hz = value;
	}
	if let Some(value) = settings.adaptive_beta {
		config.smoothing.adaptive_beta = value;
	}
	if let Some(value) = settings.adaptive_derivative_cutoff_hz {
		config.smoothing.adaptive_derivative_cutoff_hz = value;
	}
	config
}

/// Profile TOML 上の `smoothingPreset` 文字列を runtime の
/// `SmoothingPreset` enum にマップする。値は ASCII 大文字小文字を無視し、
/// 不明な値はログ無しで `Off` (pass-through) にフォールバックする。enum 側を増やしたらここも追従させる。
fn parse_smoothing_preset(value: &str) -> SmoothingPreset {
	match value.to_ascii_lowercase().as_str() {
		"low" => SmoothingPreset::Low,
		"medium" => SmoothingPreset::Medium,
		"high" => SmoothingPreset::High,
		"adaptive" => SmoothingPreset::Adaptive,
		_ => SmoothingPreset::Off,
	}
}

fn core_frame_stream_configs(
	profile: &CoreProfileDocumentProfile,
	sources: &[CoreProfileDocumentSource],
) -> Vec<CoreMotionFrameStreamConfig> {
	let mut out = Vec::new();
	let runtime = profile.runtime_selection.as_ref();
	let pipeline = profile.pipeline_components.as_ref();
	if profile.default_source_enabled {
		out.push(core_frame_stream_config(
			BUILTIN_UNMOTION_FLOW_ID.to_string(),
			unmotion_runtime_stream_id(BUILTIN_UNMOTION_FLOW_ID),
			runtime,
			pipeline,
		));
	}
	out.extend(
		sources
			.iter()
			.filter(|source| source.profile_id == profile.id && source.kind == "unmotion")
			.map(|source| core_frame_stream_config(source.id.clone(), unmotion_runtime_stream_id(&source.id), runtime, pipeline)),
	);
	out
}

fn core_frame_stream_config(
	profile_stream_id: String,
	stream_id: StreamId,
	runtime: Option<&ProfileRuntimeSettings>,
	pipeline: Option<&ProfilePipelineComponents>,
) -> CoreMotionFrameStreamConfig {
	let input_component = pipeline
		.and_then(|p| p.input.as_deref())
		.unwrap_or(default_native_input_component())
		.to_string();
	let (runtime_width, runtime_height) = runtime
		.and_then(|r| r.resolution.as_deref())
		.and_then(parse_resolution)
		.unwrap_or((640, 480));
	let input_width = pipeline.and_then(|p| p.input_width).or(Some(runtime_width));
	let input_height = pipeline.and_then(|p| p.input_height).or(Some(runtime_height));
	let resize_width = pipeline.and_then(|p| p.input_resize_width).or(input_width).unwrap_or(1280);
	let resize_height = pipeline.and_then(|p| p.input_resize_height).or(input_height).unwrap_or(720);
	let resize_axis = pipeline.and_then(|p| p.input_resize_axis.as_deref()).unwrap_or("width").to_string();
	let input_resize = pipeline
		.and_then(|p| p.input_resize_enabled)
		.unwrap_or(false)
		.then(|| CoreUnmotionResizeConfig {
			preserve_aspect_ratio: pipeline.and_then(|p| p.input_resize_preserve_aspect).unwrap_or(true),
			axis: resize_axis.clone(),
			reference: pipeline
				.and_then(|p| p.input_resize_reference)
				.or_else(|| {
					if resize_axis == "height" {
						Some(resize_height)
					} else {
						Some(resize_width)
					}
				})
				.unwrap_or(resize_width),
			width: resize_width,
			height: resize_height,
			pad_color: pipeline
				.and_then(|p| p.input_resize_pad_color.as_deref())
				.unwrap_or("000000ff")
				.to_string(),
		});
	let runtime_engine = normalize_mediapipe_engine_id(runtime.and_then(|r| r.engine.as_deref()).unwrap_or("mediapipe-native")).to_string();
	let input_repeat = pipeline.and_then(|p| p.input_repeat).unwrap_or(input_component == "file-image");
	let input_fps = effective_input_fps(&input_component, runtime, pipeline);

	// MediaPipe webcam device は MediaPipe Native のときのみ意味を持つ。
	// 他 engine では空文字に固定して "用途違いの流用" の余地を完全に断つ。
	let device_id = if runtime_engine == "mediapipe-native" {
		runtime.and_then(|r| r.device.as_deref()).unwrap_or_default().to_string()
	} else {
		String::new()
	};

	CoreMotionFrameStreamConfig {
		profile_stream_id,
		stream_id,
		device_id,
		runtime_engine,
		input_component,
		vmc_receive_listen_addr: runtime.and_then(|r| r.vmc_receive_listen_addr.clone()),
		ifacialmocap_receive_listen_addr: runtime.and_then(|r| r.ifacialmocap_receive_listen_addr.clone()),
		input_path: pipeline.and_then(|p| p.input_path.clone()),
		input_fps,
		input_width,
		input_height,
		input_pixel_format: pipeline.and_then(|p| p.input_pixel_format.clone()),
		// static image input は、live output の再現可能な pose source として使う。
		// Zenoh Pub/Sub は retained ではないため、peer discovery 中の one-shot publish は
		// 既に起動している subscriber からも取り逃がされることがある。
		input_repeat,
		input_ffmpeg_path: pipeline
			.and_then(|p| p.input_ffmpeg_path.clone())
			.or_else(load_settings_ffmpeg_path),
		input_denoise_mode: normalize_input_denoise_mode(pipeline.and_then(|p| p.input_denoise_mode.as_deref())).to_string(),
		input_denoise_temporal_iir_hz: effective_input_denoise_temporal_iir_hz(pipeline),
		input_resize,
		media_pipe_running_mode: normalize_media_pipe_running_mode(
			runtime.and_then(|r| r.media_pipe_running_mode.as_deref()).unwrap_or("live-stream"),
		),
		media_pipe_holistic_enabled: runtime.and_then(|r| r.media_pipe_holistic_enabled).unwrap_or(true),
		media_pipe_delegate: runtime.and_then(|r| r.media_pipe_delegate.clone()),
		media_pipe_num_threads: runtime.and_then(|r| r.media_pipe_num_threads).map(|value| value.max(1)),
		media_pipe_holistic_flow_limiter_enabled: runtime.and_then(|r| r.media_pipe_holistic_flow_limiter_enabled).unwrap_or(true),
		media_pipe_holistic_flow_limiter_max_in_flight: runtime
			.and_then(|r| r.media_pipe_holistic_flow_limiter_max_in_flight)
			.unwrap_or(1)
			.max(1),
		media_pipe_holistic_flow_limiter_max_in_queue: runtime.and_then(|r| r.media_pipe_holistic_flow_limiter_max_in_queue).unwrap_or(1),
		post_process_component: pipeline
			.and_then(|p| p.post_process.as_deref())
			.unwrap_or("media-pipe-default")
			.to_string(),
		media_pipe_post_process: core_media_pipe_post_process_settings(runtime),
	}
}

fn normalize_input_denoise_mode(value: Option<&str>) -> &'static str {
	match value.unwrap_or("off").trim().to_ascii_lowercase().as_str() {
		"on" | "true" => "temporal-iir",
		"temporal-iir" | "temporal_iir" | "temporal-iir-10hz" | "temporal_iir_10hz" | "temporal-iir-8hz" | "temporal_iir_8hz"
		| "temporal-iir-6hz" | "temporal_iir_6hz" | "temporal-iir-4hz" | "temporal_iir_4hz" | "temporal-iir-2hz" | "temporal_iir_2hz" => {
			"temporal-iir"
		}
		_ => "off",
	}
}

fn effective_input_denoise_temporal_iir_hz(pipeline: Option<&ProfilePipelineComponents>) -> f64 {
	if let Some(value) = pipeline
		.and_then(|p| p.input_denoise_temporal_iir_hz)
		.map(|value| value as f64)
		.filter(|value| value.is_finite())
	{
		return value.clamp(1.0, 32.0);
	}
	match pipeline
		.and_then(|p| p.input_denoise_mode.as_deref())
		.unwrap_or_default()
		.trim()
		.to_ascii_lowercase()
		.as_str()
	{
		"temporal-iir-8hz" | "temporal_iir_8hz" => 8.0,
		"temporal-iir-6hz" | "temporal_iir_6hz" => 6.0,
		"temporal-iir-4hz" | "temporal_iir_4hz" => 4.0,
		"temporal-iir-2hz" | "temporal_iir_2hz" => 2.0,
		_ => 10.0,
	}
}

fn effective_input_fps(
	input_component: &str,
	runtime: Option<&ProfileRuntimeSettings>,
	pipeline: Option<&ProfilePipelineComponents>,
) -> u32 {
	let explicit_output_fps = runtime.and_then(|r| r.fps);
	let pipeline_fps = pipeline.and_then(|p| p.input_fps);
	match input_component {
		// file-image には実 source clock がない。output fps が明示されているなら
		// repeat clock に使い、古い inputFps が UNMF/Z output を隠れて制限しないようにする。
		"file-image" => explicit_output_fps.or(pipeline_fps).unwrap_or(DEFAULT_FPS),
		_ => pipeline_fps.or(explicit_output_fps).unwrap_or(DEFAULT_FPS),
	}
	.clamp(1, MAX_FPS)
}

fn load_settings_ffmpeg_path() -> Option<String> {
	let path = user_config_dir().join("settings.toml");
	let raw = fs::read_to_string(path).ok()?;
	let doc = raw.parse::<toml::Value>().ok()?;
	doc.get("externalToolsFfmpegPath")
		.and_then(toml::Value::as_str)
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(ToString::to_string)
		.or_else(|| {
			doc.get("desktop")
				.and_then(|desktop| desktop.get("external_tools"))
				.and_then(|tools| tools.get("ffmpeg_path"))
				.and_then(toml::Value::as_str)
				.map(str::trim)
				.filter(|value| !value.is_empty())
				.map(ToString::to_string)
		})
}

fn user_config_dir() -> PathBuf {
	if let Some(path) = env::var_os("UN_MOTION_CONFIG_DIR") {
		return PathBuf::from(path);
	}
	if let Some(path) = env::var_os("APPDATA") {
		return PathBuf::from(path).join("UN Motion");
	}
	if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
		return PathBuf::from(path).join("un-motion");
	}
	if let Some(path) = env::var_os("HOME") {
		return PathBuf::from(path).join(".config").join("un-motion");
	}
	PathBuf::from(".").join("un-motion-config")
}

fn run_core_runtime(
	config: CoreRuntimeConfig,
	control_rx: Receiver<CoreRuntimeControl>,
	init_tx: Sender<anyhow::Result<()>>,
	status_callback: StatusCallback,
) {
	let active_profile_id = config.active_profile_id.clone();
	let startup = CoreRuntimeWorkers::start(&config);
	let workers = match startup {
		Ok(workers) => workers,
		Err(error) => {
			let _ = init_tx.send(Err(error));
			return;
		}
	};
	let mut latest_states = initial_stream_states(&config);
	let mut output_telemetry = workers.initial_output_telemetry();
	workers.refresh_source_telemetry(&mut output_telemetry);
	status_callback(CoreRuntimeStatusUpdate {
		active_profile_id: active_profile_id.clone(),
		running: true,
		health: "running".to_string(),
		frame_count: 0,
		packet_count: 0,
		runtime_snapshot: Some(runtime_snapshot(&config, &latest_states, RuntimeState::Starting, &output_telemetry)),
	});
	let _ = init_tx.send(Ok(()));

	let mut frame_count = 0_u64;
	let mut packet_count = 0_u64;
	let mut health = "streaming".to_string();
	let mut cadence = OutputCadence::for_fps(config.fps, Instant::now());
	// 正式経路: source worker がイベント駆動で latest slot へ書いた UNMotionFrame を、
	// output cadence で snapshot して両 output worker に流す。
	let mut latest_unmotion_frame: Option<Arc<un_motion_frame::UNMotionFrame>> = None;
	let mut last_output_sequences: HashMap<StreamId, u64> = HashMap::new();
	let mut neutral_calibration: Option<PendingNeutralCalibration> = None;
	let mut face_pose_model: Option<PendingFacePoseModel> = None;
	tracing::info!(
		frame_streams = config.frame_streams.len(),
		vmc_output = workers.vmc_output.is_some(),
		vrc_osc_output = workers.vrc_osc_output.is_some(),
		zenoh_output = workers.zenoh_output.is_some(),
		"UNMotionFrame runtime route selected"
	);

	loop {
		// drain / cadence いずれが先に snapshot を発行しても `output_telemetry.sources` が
		// 直近の値を反映するよう、ループ先頭で 1 回 atomic load する。worker 側は fetch_add のみで
		// 競合せず、Supervisor 側はこれを 1.5s 周期で差分→FPS 計算する。
		workers.refresh_source_telemetry(&mut output_telemetry);
		let control = drain_control_messages(
			&control_rx,
			&workers,
			&mut latest_states,
			&config,
			&mut neutral_calibration,
			&mut face_pose_model,
			&latest_unmotion_frame,
		);
		if control.stop {
			break;
		}
		if apply_frame_stream_startup_errors(&workers.frame_stream_errors, &mut latest_states, &mut health) {
			status_callback(CoreRuntimeStatusUpdate {
				active_profile_id: active_profile_id.clone(),
				running: true,
				health: health.clone(),
				frame_count,
				packet_count,
				runtime_snapshot: Some(runtime_snapshot(&config, &latest_states, RuntimeState::Running, &output_telemetry)),
			});
		}

		let received_unmotion = refresh_latest_motion_frames(
			&workers.frame_streams,
			&mut latest_states,
			&mut latest_unmotion_frame,
			&mut packet_count,
			&mut health,
			&active_profile_id,
			frame_count,
			&config,
			&output_telemetry,
			&status_callback,
		);
		collect_pending_neutral_calibration(&mut neutral_calibration, &latest_states);
		collect_pending_face_pose_model(&mut face_pose_model, &latest_unmotion_frame);
		drain_output_messages(&workers.output_event_rx, &mut health, &mut output_telemetry);
		drain_vrc_osc_output_messages(&workers.vrc_osc_event_rx, &mut health, &mut output_telemetry);
		drain_zenoh_output_messages(&workers.zenoh_event_rx, &mut health, &mut output_telemetry);

		let now = Instant::now();
		let mut sent = false;
		if cadence.mark_due(now) {
			if let Some(frame) = select_latest_unsent_frame(&workers.frame_streams, &mut last_output_sequences) {
				frame_count = frame_count.saturating_add(1);
				if let Some(vmc_output) = &workers.vmc_output
					&& vmc_output.send(frame.clone()).is_err()
				{
					health = "output disconnected".to_string();
				}
				if let Some(vrc_osc_output) = &workers.vrc_osc_output
					&& vrc_osc_output.send(frame.clone()).is_err()
				{
					health = "vrc osc output disconnected".to_string();
				}
				if let Some(zenoh_output) = &workers.zenoh_output
					&& zenoh_output.send(frame).is_err()
				{
					health = "zenoh output disconnected".to_string();
				}
				sent = true;
				workers.refresh_source_telemetry(&mut output_telemetry);
				status_callback(CoreRuntimeStatusUpdate {
					active_profile_id: active_profile_id.clone(),
					running: true,
					health: health.clone(),
					frame_count,
					packet_count,
					runtime_snapshot: Some(runtime_snapshot(&config, &latest_states, RuntimeState::Running, &output_telemetry)),
				});
			}
		}

		if !control.received && !received_unmotion && !sent {
			thread::sleep(cadence.sleep_duration(Instant::now(), LOOP_MAX_SLEEP));
		}
	}

	// stop 直前にもう一度カウンタを掬う。worker は join 直後に
	// `Arc<SourceStageAtomics>` への参照を 1 つしか残さないが、Capturer の最後の
	// `runtime_snapshot` に「停止時点の最終値」が乗ったほうが UI で混乱が無い。
	workers.refresh_source_telemetry(&mut output_telemetry);
	workers.stop();
	status_callback(CoreRuntimeStatusUpdate {
		active_profile_id,
		running: false,
		health: "stopped".to_string(),
		frame_count,
		packet_count,
		runtime_snapshot: Some(runtime_snapshot(&config, &latest_states, RuntimeState::Stopped, &output_telemetry)),
	});
}

fn initial_stream_states(config: &CoreRuntimeConfig) -> HashMap<StreamId, Arc<StreamState>> {
	config
		.frame_streams
		.iter()
		.map(|stream| (stream.stream_id.clone(), Arc::new(StreamState::new(stream.stream_id.clone()))))
		.collect()
}

fn apply_frame_stream_startup_errors(
	errors: &[(StreamId, String)],
	latest_states: &mut HashMap<StreamId, Arc<StreamState>>,
	health: &mut String,
) -> bool {
	let mut changed = false;
	for (stream_id, message) in errors {
		let state = latest_states
			.entry(stream_id.clone())
			.or_insert_with(|| Arc::new(StreamState::new(stream_id.clone())));
		let state = Arc::make_mut(state);
		if state.decode_error_count == 0 {
			state.decode_error_count = 1;
			changed = true;
		}
		*health = format!("{} Motion frame stream failed: {message}", stream_id.0);
	}
	changed
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ControlDrainResult {
	stop: bool,
	received: bool,
}

#[allow(clippy::too_many_arguments)]
fn drain_control_messages(
	control_rx: &Receiver<CoreRuntimeControl>,
	workers: &CoreRuntimeWorkers,
	latest_states: &mut HashMap<StreamId, Arc<StreamState>>,
	config: &CoreRuntimeConfig,
	neutral_calibration: &mut Option<PendingNeutralCalibration>,
	face_pose_model: &mut Option<PendingFacePoseModel>,
	latest_unmotion_frame: &Option<Arc<UNMotionFrame>>,
) -> ControlDrainResult {
	let mut result = ControlDrainResult::default();
	loop {
		match control_rx.try_recv() {
			Ok(CoreRuntimeControl::Stop) | Err(TryRecvError::Disconnected) => {
				result.stop = true;
				return result;
			}
			Ok(CoreRuntimeControl::CalibrateNeutral {
				pose,
				valid_sample_count,
				response_tx,
			}) => {
				result.received = true;
				let sample_count = valid_sample_count.clamp(1, 240);
				*neutral_calibration = Some(PendingNeutralCalibration::new(sample_count, pose, response_tx));
				collect_pending_neutral_calibration(neutral_calibration, latest_states);
			}
			Ok(CoreRuntimeControl::BuildFacePoseModel {
				valid_sample_count,
				response_tx,
			}) => {
				result.received = true;
				let sample_count = valid_sample_count.clamp(1, 240);
				*face_pose_model = Some(PendingFacePoseModel::new(sample_count, response_tx));
				collect_pending_face_pose_model(face_pose_model, latest_unmotion_frame);
			}
			Ok(CoreRuntimeControl::CaptureUnmotionFrame { response_tx }) => {
				result.received = true;
				let response = latest_unmotion_frame
					.as_ref()
					.map(|frame| {
						let mut frame = (**frame).clone();
						let mut pipeline = ModifierPipeline::from_config(capture_unmotion_frame_modifier(config));
						pipeline.apply(&mut frame);
						frame
					})
					.context("no UNMotionFrame has been produced yet");
				let _ = response_tx.send(response);
			}
			Ok(CoreRuntimeControl::BeginAnalysisCapture {
				output_dir,
				max_samples,
				response_tx,
			}) => {
				result.received = true;
				for handle in &workers.frame_streams {
					if let Some(telemetry) = &handle.telemetry {
						telemetry.debug.begin_capture(output_dir.clone(), max_samples);
					}
				}
				let _ = response_tx.send(Ok(()));
			}
			Ok(CoreRuntimeControl::FinishAnalysisCapture { response_tx }) => {
				result.received = true;
				let mut samples = Vec::new();
				for handle in &workers.frame_streams {
					if let Some(telemetry) = &handle.telemetry {
						samples.extend(telemetry.debug.finish_capture());
					}
				}
				let _ = response_tx.send(Ok(samples));
			}
			Err(TryRecvError::Empty) => return result,
		}
	}
}

fn capture_unmotion_frame_modifier(config: &CoreRuntimeConfig) -> &ModifierConfig {
	config
		.zenoh_output
		.as_ref()
		.map(|output| &output.modifier)
		.or_else(|| config.vmc_output.as_ref().map(|output| &output.modifier))
		.unwrap_or(&DEFAULT_MODIFIER_CONFIG)
}

static DEFAULT_MODIFIER_CONFIG: std::sync::LazyLock<ModifierConfig> = std::sync::LazyLock::new(ModifierConfig::default);

struct PendingNeutralCalibration {
	target_samples: usize,
	pose: NeutralCalibrationPose,
	valid_samples: usize,
	last_sample_at_ns: Option<u64>,
	rotations: BTreeMap<String, Vec<[f32; 4]>>,
	response_tx: Option<Sender<anyhow::Result<NeutralCalibrationResult>>>,
}

impl PendingNeutralCalibration {
	fn new(target_samples: usize, pose: NeutralCalibrationPose, response_tx: Sender<anyhow::Result<NeutralCalibrationResult>>) -> Self {
		Self {
			target_samples,
			pose,
			valid_samples: 0,
			last_sample_at_ns: None,
			rotations: BTreeMap::new(),
			response_tx: Some(response_tx),
		}
	}
}

fn collect_pending_neutral_calibration(
	pending: &mut Option<PendingNeutralCalibration>,
	latest_states: &HashMap<StreamId, Arc<StreamState>>,
) {
	let Some(calibration) = pending.as_mut() else {
		return;
	};
	if !push_neutral_sample(calibration, latest_states) {
		return;
	}
	if calibration.valid_samples < calibration.target_samples {
		return;
	}
	let result = NeutralCalibrationResult {
		valid_samples: calibration.valid_samples,
		rotations: summarize_neutral_rotations(&calibration.rotations, calibration.pose),
	};
	if let Some(response_tx) = calibration.response_tx.take() {
		let _ = response_tx.send(Ok(result));
	}
	*pending = None;
}

fn push_neutral_sample(calibration: &mut PendingNeutralCalibration, latest_states: &HashMap<StreamId, Arc<StreamState>>) -> bool {
	let Some(state) = latest_states.values().max_by_key(|state| state.last_event_at_ns.unwrap_or(0)) else {
		return false;
	};
	if state.root.is_none() && state.bones.is_empty() {
		return false;
	}
	let sample_at_ns = state.last_event_at_ns.unwrap_or(0);
	if calibration.last_sample_at_ns == Some(sample_at_ns) {
		return false;
	}
	if let Some(root) = state.root.as_ref() {
		calibration
			.rotations
			.entry("Root".to_string())
			.or_default()
			.push(normalize_quat_array(root.rotation));
	}
	for (name, bone) in &state.bones {
		calibration
			.rotations
			.entry(name.clone())
			.or_default()
			.push(normalize_quat_array(bone.rotation));
	}
	calibration.valid_samples += 1;
	calibration.last_sample_at_ns = Some(sample_at_ns);
	true
}

struct PendingFacePoseModel {
	target_samples: usize,
	valid_samples: usize,
	last_sample_at_ns: Option<u64>,
	nose_drop_eye_mouth: Vec<f32>,
	abs_yaw: Vec<f32>,
	abs_roll: Vec<f32>,
	confidence: Vec<f32>,
	response_tx: Option<Sender<anyhow::Result<FacePoseModelResult>>>,
}

impl PendingFacePoseModel {
	fn new(target_samples: usize, response_tx: Sender<anyhow::Result<FacePoseModelResult>>) -> Self {
		Self {
			target_samples,
			valid_samples: 0,
			last_sample_at_ns: None,
			nose_drop_eye_mouth: Vec::with_capacity(target_samples),
			abs_yaw: Vec::with_capacity(target_samples),
			abs_roll: Vec::with_capacity(target_samples),
			confidence: Vec::with_capacity(target_samples),
			response_tx: Some(response_tx),
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FaceMetricsSample {
	nose_drop_eye_mouth: f32,
	yaw: f32,
	roll: f32,
	confidence: f32,
}

fn collect_pending_face_pose_model(pending: &mut Option<PendingFacePoseModel>, latest_unmotion_frame: &Option<Arc<UNMotionFrame>>) {
	let Some(model) = pending.as_mut() else {
		return;
	};
	if !push_face_pose_model_sample(model, latest_unmotion_frame) {
		return;
	}
	if model.valid_samples < model.target_samples {
		return;
	}
	let result = summarize_face_pose_model(model);
	if let Some(response_tx) = model.response_tx.take() {
		let _ = response_tx.send(result);
	}
	*pending = None;
}

fn summarize_face_pose_model(model: &mut PendingFacePoseModel) -> anyhow::Result<FacePoseModelResult> {
	let neutral_nose_drop_eye_mouth = median_f32(&mut model.nose_drop_eye_mouth).unwrap_or(0.64);
	let median_abs_yaw = median_f32(&mut model.abs_yaw).unwrap_or(0.0);
	let median_abs_roll = median_f32(&mut model.abs_roll).unwrap_or(0.0);
	let nose_drop_mad = median_abs_deviation_f32(&model.nose_drop_eye_mouth, neutral_nose_drop_eye_mouth).unwrap_or(0.0);
	if median_abs_yaw > FACE_POSE_MODEL_MAX_MEDIAN_ABS_YAW {
		anyhow::bail!(
			"face pose model quality check failed: 顔を正面へ向けて下さい (median yaw {:.3} > {:.3})",
			median_abs_yaw,
			FACE_POSE_MODEL_MAX_MEDIAN_ABS_YAW
		);
	}
	if median_abs_roll > FACE_POSE_MODEL_MAX_MEDIAN_ABS_ROLL {
		anyhow::bail!(
			"face pose model quality check failed: 首を傾けず水平にして下さい (median roll {:.3} > {:.3})",
			median_abs_roll,
			FACE_POSE_MODEL_MAX_MEDIAN_ABS_ROLL
		);
	}
	if nose_drop_mad > FACE_POSE_MODEL_MAX_NOSE_DROP_MAD {
		anyhow::bail!(
			"face pose model quality check failed: サンプリング中の顔姿勢が安定していません (MAD {:.3} > {:.3})",
			nose_drop_mad,
			FACE_POSE_MODEL_MAX_NOSE_DROP_MAD
		);
	}
	Ok(FacePoseModelResult {
		valid_samples: model.valid_samples,
		neutral_nose_drop_eye_mouth,
		median_abs_yaw,
		median_abs_roll,
	})
}

fn push_face_pose_model_sample(model: &mut PendingFacePoseModel, latest_unmotion_frame: &Option<Arc<UNMotionFrame>>) -> bool {
	let Some(frame) = latest_unmotion_frame.as_ref() else {
		return false;
	};
	let sample_at_ns = frame.header.capture_timestamp_ns;
	if model.last_sample_at_ns == Some(sample_at_ns) {
		return false;
	}
	let Some(sample) = face_metrics_sample_from_notes(&frame.metadata.notes) else {
		return false;
	};
	if !sample.nose_drop_eye_mouth.is_finite() || !(0.25..=1.10).contains(&sample.nose_drop_eye_mouth) {
		return false;
	}
	if !sample.confidence.is_finite() || sample.confidence < FACE_POSE_MODEL_MIN_CONFIDENCE {
		return false;
	}
	if !sample.yaw.is_finite() || sample.yaw.abs() > FACE_POSE_MODEL_MAX_SAMPLE_ABS_YAW {
		return false;
	}
	if !sample.roll.is_finite() || sample.roll.abs() > FACE_POSE_MODEL_MAX_SAMPLE_ABS_ROLL {
		return false;
	}
	model.nose_drop_eye_mouth.push(sample.nose_drop_eye_mouth);
	model.abs_yaw.push(sample.yaw.abs());
	model.abs_roll.push(sample.roll.abs());
	model.confidence.push(sample.confidence);
	model.valid_samples += 1;
	model.last_sample_at_ns = Some(sample_at_ns);
	true
}

fn face_metrics_sample_from_notes(notes: &[String]) -> Option<FaceMetricsSample> {
	let note = notes.iter().rev().find(|note| note.starts_with("mediapipe.face_metrics "))?;
	Some(FaceMetricsSample {
		nose_drop_eye_mouth: parse_note_f32(note, "noseDropEyeMouth=")?,
		yaw: parse_note_f32(note, "yaw=")?,
		roll: parse_note_f32(note, "roll=")?,
		confidence: parse_note_f32(note, "confidence=")?,
	})
}

fn parse_note_f32(note: &str, key: &str) -> Option<f32> {
	let start = note.find(key)? + key.len();
	let rest = &note[start..];
	let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
	rest[..end].parse().ok()
}

fn median_f32(values: &mut [f32]) -> Option<f32> {
	if values.is_empty() {
		return None;
	}
	values.sort_by(|a, b| a.total_cmp(b));
	let mid = values.len() / 2;
	if values.len() % 2 == 0 {
		Some((values[mid - 1] + values[mid]) * 0.5)
	} else {
		Some(values[mid])
	}
}

fn median_abs_deviation_f32(values: &[f32], median: f32) -> Option<f32> {
	let mut deviations = values.iter().map(|value| (value - median).abs()).collect::<Vec<_>>();
	median_f32(&mut deviations)
}

fn summarize_neutral_rotations(samples: &BTreeMap<String, Vec<[f32; 4]>>, pose: NeutralCalibrationPose) -> BTreeMap<String, [f32; 4]> {
	samples
		.iter()
		.filter_map(|(key, values)| {
			if !neutral_calibration_key_enabled(key) {
				return None;
			}
			let actual = average_quaternions(values)?;
			let target = neutral_calibration_target_rotation(key, pose)?;
			Some((key.clone(), neutral_calibration_offset_for_key(key, pose, actual, target)))
		})
		.collect()
}

fn neutral_calibration_key_enabled(key: &str) -> bool {
	// Limb rotation は pose output であり、安定した neutral offset ではない。
	// 保存する neutral calibration は global/head など安定参照に限定する。
	matches!(key, "Root" | "Head")
}

fn neutral_calibration_target_rotation(key: &str, pose: NeutralCalibrationPose) -> Option<[f32; 4]> {
	match (pose, key) {
		(NeutralCalibrationPose::T, "LeftLowerArm") => {
			quat_from_basis([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
		}
		(NeutralCalibrationPose::T, "RightLowerArm") => {
			quat_from_basis([1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
		}
		(NeutralCalibrationPose::I, "LeftUpperArm") => Some(quat_from_to([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0])),
		(NeutralCalibrationPose::I, "RightUpperArm") => Some(quat_from_to([1.0, 0.0, 0.0], [0.0, -1.0, 0.0])),
		(NeutralCalibrationPose::U, "LeftUpperArm") => Some(quat_from_to([-1.0, 0.0, 0.0], normalize_vec3([-0.82, 0.57, 0.0])?)),
		(NeutralCalibrationPose::U, "RightUpperArm") => Some(quat_from_to([1.0, 0.0, 0.0], normalize_vec3([0.82, 0.57, 0.0])?)),
		(NeutralCalibrationPose::U, "LeftLowerArm") => u_pose_lower_arm_target(false),
		(NeutralCalibrationPose::U, "RightLowerArm") => u_pose_lower_arm_target(true),
		(NeutralCalibrationPose::U, "LeftHand") => u_pose_hand_target(false),
		(NeutralCalibrationPose::U, "RightHand") => u_pose_hand_target(true),
		_ => Some([0.0, 0.0, 0.0, 1.0]),
	}
}

fn neutral_calibration_offset_for_key(key: &str, pose: NeutralCalibrationPose, actual: [f32; 4], target: [f32; 4]) -> [f32; 4] {
	if matches!(pose, NeutralCalibrationPose::U | NeutralCalibrationPose::I) && neutral_calibration_preserve_live_rotation_for_key(key) {
		return [0.0, 0.0, 0.0, 1.0];
	}
	let offset = neutral_calibration_offset(actual, target);
	if matches!(pose, NeutralCalibrationPose::U | NeutralCalibrationPose::I)
		&& let Some(axis) = neutral_calibration_limb_twist_axis(key)
	{
		return constrain_quat_to_axis_twist(offset, axis);
	}
	offset
}

fn neutral_calibration_offset(actual: [f32; 4], target: [f32; 4]) -> [f32; 4] {
	normalize_quat_array(quat_mul_array(quat_inverse_array(target), actual))
}

fn neutral_calibration_preserve_live_rotation_for_key(key: &str) -> bool {
	matches!(key, "LeftShoulder" | "LeftUpperArm" | "RightShoulder" | "RightUpperArm")
}

fn neutral_calibration_limb_twist_axis(key: &str) -> Option<[f32; 3]> {
	match key {
		"LeftLowerArm" | "LeftHand" => Some([-1.0, 0.0, 0.0]),
		"RightLowerArm" | "RightHand" => Some([1.0, 0.0, 0.0]),
		_ => None,
	}
}

fn constrain_quat_to_axis_twist(rotation: [f32; 4], axis: [f32; 3]) -> [f32; 4] {
	let rotation = normalize_quat_array(rotation);
	let axis = normalize_vec3(axis).unwrap_or([1.0, 0.0, 0.0]);
	let projected = [
		axis[0] * dot3([rotation[0], rotation[1], rotation[2]], axis),
		axis[1] * dot3([rotation[0], rotation[1], rotation[2]], axis),
		axis[2] * dot3([rotation[0], rotation[1], rotation[2]], axis),
		rotation[3],
	];
	normalize_quat_array(projected)
}

fn u_pose_upper_arm_axis(right: bool) -> [f32; 3] {
	if right {
		normalize_vec3([0.82, 0.57, 0.0]).unwrap()
	} else {
		normalize_vec3([-0.82, 0.57, 0.0]).unwrap()
	}
}

fn u_pose_lower_arm_axis(right: bool) -> [f32; 3] {
	if right {
		normalize_vec3([-0.707, 0.707, 0.0]).unwrap()
	} else {
		normalize_vec3([0.707, 0.707, 0.0]).unwrap()
	}
}

fn u_pose_rest_axis(right: bool) -> [f32; 3] {
	if right { [1.0, 0.0, 0.0] } else { [-1.0, 0.0, 0.0] }
}

fn u_pose_lower_arm_target(right: bool) -> Option<[f32; 4]> {
	let rest_axis = u_pose_rest_axis(right);
	let upper_global = quat_from_to(rest_axis, u_pose_upper_arm_axis(right));
	let lower_global = quat_from_basis(rest_axis, [0.0, -1.0, 0.0], u_pose_lower_arm_axis(right), [0.0, 0.0, 1.0])?;
	Some(normalize_quat_array(quat_mul_array(quat_inverse_array(upper_global), lower_global)))
}

fn u_pose_hand_target(right: bool) -> Option<[f32; 4]> {
	let rest_axis = u_pose_rest_axis(right);
	let lower_global = quat_from_basis(rest_axis, [0.0, -1.0, 0.0], u_pose_lower_arm_axis(right), [0.0, 0.0, 1.0])?;
	let hand_global = quat_from_basis(rest_axis, [0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0])?;
	Some(normalize_quat_array(quat_mul_array(quat_inverse_array(lower_global), hand_global)))
}

fn average_quaternions(values: &[[f32; 4]]) -> Option<[f32; 4]> {
	let first = *values.first()?;
	let mut sum = [0.0_f32; 4];
	for value in values {
		let mut q = normalize_quat_array(*value);
		if quat_array_dot(first, q) < 0.0 {
			q = [-q[0], -q[1], -q[2], -q[3]];
		}
		for i in 0..4 {
			sum[i] += q[i];
		}
	}
	Some(normalize_quat_array(sum))
}

fn normalize_quat_array(q: [f32; 4]) -> [f32; 4] {
	let len_sq = q.iter().map(|v| v * v).sum::<f32>();
	if !len_sq.is_finite() || len_sq < 1e-12 {
		[0.0, 0.0, 0.0, 1.0]
	} else {
		let inv_len = 1.0 / len_sq.sqrt();
		[q[0] * inv_len, q[1] * inv_len, q[2] * inv_len, q[3] * inv_len]
	}
}

fn quat_array_dot(a: [f32; 4], b: [f32; 4]) -> f32 {
	a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

fn quat_inverse_array(q: [f32; 4]) -> [f32; 4] {
	let q = normalize_quat_array(q);
	[-q[0], -q[1], -q[2], q[3]]
}

fn quat_mul_array(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
	[
		a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
		a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
		a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
		a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
	]
}

fn quat_from_to(from: [f32; 3], to: [f32; 3]) -> [f32; 4] {
	let Some(from) = normalize_vec3(from) else {
		return [0.0, 0.0, 0.0, 1.0];
	};
	let Some(to) = normalize_vec3(to) else {
		return [0.0, 0.0, 0.0, 1.0];
	};
	let dot = dot3(from, to).clamp(-1.0, 1.0);
	if dot > 0.999_999 {
		return [0.0, 0.0, 0.0, 1.0];
	}
	if dot < -0.999_999 {
		let axis = if from[0].abs() < 0.9 {
			normalize_vec3(cross3(from, [1.0, 0.0, 0.0])).unwrap_or([0.0, 1.0, 0.0])
		} else {
			normalize_vec3(cross3(from, [0.0, 1.0, 0.0])).unwrap_or([0.0, 0.0, 1.0])
		};
		return [axis[0], axis[1], axis[2], 0.0];
	}
	let axis = cross3(from, to);
	normalize_quat_array([axis[0], axis[1], axis[2], 1.0 + dot])
}

fn quat_from_basis(from_primary: [f32; 3], from_secondary: [f32; 3], to_primary: [f32; 3], to_secondary: [f32; 3]) -> Option<[f32; 4]> {
	let from_primary = normalize_vec3(from_primary)?;
	let from_secondary = project_onto_plane(from_secondary, from_primary)?;
	let from_third = normalize_vec3(cross3(from_primary, from_secondary))?;
	let to_primary = normalize_vec3(to_primary)?;
	let to_secondary = project_onto_plane(to_secondary, to_primary)?;
	let to_third = normalize_vec3(cross3(to_primary, to_secondary))?;
	let from_matrix = [
		[from_primary[0], from_secondary[0], from_third[0]],
		[from_primary[1], from_secondary[1], from_third[1]],
		[from_primary[2], from_secondary[2], from_third[2]],
	];
	let to_matrix = [
		[to_primary[0], to_secondary[0], to_third[0]],
		[to_primary[1], to_secondary[1], to_third[1]],
		[to_primary[2], to_secondary[2], to_third[2]],
	];
	let from_transpose = transpose3(from_matrix);
	let rotation = multiply3(to_matrix, from_transpose);
	Some(quat_from_rotation_matrix(rotation))
}

fn project_onto_plane(v: [f32; 3], normal: [f32; 3]) -> Option<[f32; 3]> {
	normalize_vec3([
		v[0] - (normal[0] * dot3(v, normal)),
		v[1] - (normal[1] * dot3(v, normal)),
		v[2] - (normal[2] * dot3(v, normal)),
	])
}

fn normalize_vec3(v: [f32; 3]) -> Option<[f32; 3]> {
	let len_sq = dot3(v, v);
	if !len_sq.is_finite() || len_sq < 1e-12 {
		return None;
	}
	let inv_len = 1.0 / len_sq.sqrt();
	Some([v[0] * inv_len, v[1] * inv_len, v[2] * inv_len])
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
	a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
	[a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn transpose3(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
	[
		[m[0][0], m[1][0], m[2][0]],
		[m[0][1], m[1][1], m[2][1]],
		[m[0][2], m[1][2], m[2][2]],
	]
}

fn multiply3(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
	let mut out = [[0.0_f32; 3]; 3];
	for row in 0..3 {
		for col in 0..3 {
			out[row][col] = (0..3).map(|i| a[row][i] * b[i][col]).sum();
		}
	}
	out
}

fn quat_from_rotation_matrix(m: [[f32; 3]; 3]) -> [f32; 4] {
	let trace = m[0][0] + m[1][1] + m[2][2];
	if trace > 0.0 {
		let s = (trace + 1.0).sqrt() * 2.0;
		return normalize_quat_array([(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s]);
	}
	if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
		let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
		return normalize_quat_array([0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s]);
	}
	if m[1][1] > m[2][2] {
		let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
		return normalize_quat_array([(m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s]);
	}
	let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
	normalize_quat_array([(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s])
}

#[allow(clippy::too_many_arguments)]
fn refresh_latest_motion_frames(
	frame_streams: &[LatestMotionFrameStreamWorkerHandle],
	latest_states: &mut HashMap<StreamId, Arc<StreamState>>,
	latest_unmotion_frame: &mut Option<Arc<un_motion_frame::UNMotionFrame>>,
	packet_count: &mut u64,
	health: &mut String,
	active_profile_id: &str,
	frame_count: u64,
	config: &CoreRuntimeConfig,
	output_telemetry: &OutputTelemetry,
	status_callback: &StatusCallback,
) -> bool {
	refresh_latest_motion_frames_from_loaded(
		frame_streams.iter().filter_map(|handle| handle.slot.load()),
		latest_states,
		latest_unmotion_frame,
		packet_count,
		health,
		active_profile_id,
		frame_count,
		config,
		output_telemetry,
		status_callback,
	)
}

#[allow(clippy::too_many_arguments)]
fn refresh_latest_motion_frames_from_loaded(
	latest_frames: impl Iterator<Item = Arc<LatestMotionFrame>>,
	latest_states: &mut HashMap<StreamId, Arc<StreamState>>,
	latest_unmotion_frame: &mut Option<Arc<un_motion_frame::UNMotionFrame>>,
	packet_count: &mut u64,
	health: &mut String,
	active_profile_id: &str,
	frame_count: u64,
	config: &CoreRuntimeConfig,
	output_telemetry: &OutputTelemetry,
	status_callback: &StatusCallback,
) -> bool {
	let mut received = false;
	for latest in latest_frames {
		let previous = latest_states.get(&latest.stream_id).and_then(|state| state.last_event_at_ns);
		if previous == latest.state.last_event_at_ns {
			continue;
		}
		received = true;
		*packet_count = packet_count.saturating_add(1);
		*health = "streaming".to_string();
		*latest_unmotion_frame = Some(latest.frame.clone());
		latest_states.insert(latest.stream_id.clone(), latest.state.clone());
		status_callback(CoreRuntimeStatusUpdate {
			active_profile_id: active_profile_id.to_string(),
			running: true,
			health: health.clone(),
			frame_count,
			packet_count: *packet_count,
			runtime_snapshot: Some(runtime_snapshot(config, latest_states, RuntimeState::Running, output_telemetry)),
		});
	}
	received
}

fn select_latest_unsent_frame(
	frame_streams: &[LatestMotionFrameStreamWorkerHandle],
	last_output_sequences: &mut HashMap<StreamId, u64>,
) -> Option<Arc<un_motion_frame::UNMotionFrame>> {
	select_latest_unsent_frame_from_loaded(frame_streams.iter().filter_map(|handle| handle.slot.load()), last_output_sequences)
}

fn select_latest_unsent_frame_from_loaded(
	latest_frames: impl Iterator<Item = Arc<LatestMotionFrame>>,
	last_output_sequences: &mut HashMap<StreamId, u64>,
) -> Option<Arc<un_motion_frame::UNMotionFrame>> {
	let latest = latest_frames
		.filter(|latest| last_output_sequences.get(&latest.stream_id).copied() != Some(latest.frame.header.sequence))
		.max_by_key(|latest| latest.frame.header.capture_timestamp_ns)?;
	last_output_sequences.insert(latest.stream_id.clone(), latest.frame.header.sequence);
	Some(latest.frame.clone())
}

fn runtime_snapshot(
	config: &CoreRuntimeConfig,
	latest_states: &HashMap<StreamId, Arc<StreamState>>,
	state: RuntimeState,
	output_telemetry: &OutputTelemetry,
) -> RuntimeSnapshot {
	let generated_at_ns = now_unix_ns();
	RuntimeSnapshot {
		state,
		active_profile_id: Some(ProfileId::new(config.active_profile_id.clone())),
		generated_at_ns,
		streams: latest_states
			.values()
			.map(|state| state.snapshot_at(generated_at_ns, STALE_AFTER_NS))
			.collect(),
		output_telemetry: output_telemetry.clone(),
	}
}

fn drain_output_messages(output_rx: &Receiver<VmcOutputEvent>, health: &mut String, output_telemetry: &mut OutputTelemetry) {
	loop {
		match output_rx.try_recv() {
			Ok(VmcOutputEvent::Sent { datagrams, packets, .. }) => {
				if let Some(vmc) = output_telemetry.vmc.as_mut() {
					vmc.sent_datagrams = vmc.sent_datagrams.saturating_add(datagrams);
					vmc.sent_packets = vmc.sent_packets.saturating_add(packets);
				}
			}
			Ok(VmcOutputEvent::Error { message, .. }) => {
				*health = format!("VMC output failed: {message}");
				if let Some(vmc) = output_telemetry.vmc.as_mut() {
					vmc.error_count = vmc.error_count.saturating_add(1);
					vmc.last_error = Some(message);
				}
			}
			Ok(VmcOutputEvent::Stopped { .. }) => {}
			Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
		}
	}
}

fn drain_zenoh_output_messages(output_rx: &Receiver<ZenohOutputEvent>, health: &mut String, output_telemetry: &mut OutputTelemetry) {
	loop {
		match output_rx.try_recv() {
			Ok(ZenohOutputEvent::Sent { frames, .. }) => {
				if let Some(zenoh) = output_telemetry.zenoh.as_mut() {
					zenoh.sent_frames = zenoh.sent_frames.saturating_add(frames);
				}
			}
			Ok(ZenohOutputEvent::Error { message }) => {
				*health = format!("Zenoh output failed: {message}");
				if let Some(zenoh) = output_telemetry.zenoh.as_mut() {
					zenoh.error_count = zenoh.error_count.saturating_add(1);
					zenoh.last_error = Some(message);
				}
			}
			Ok(ZenohOutputEvent::Stopped { .. }) => {}
			Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
		}
	}
}

fn drain_vrc_osc_output_messages(output_rx: &Receiver<VrcOscOutputEvent>, health: &mut String, output_telemetry: &mut OutputTelemetry) {
	loop {
		match output_rx.try_recv() {
			Ok(VrcOscOutputEvent::Sent {
				datagrams,
				packets,
				vrchat_detected,
				..
			}) => {
				if let Some(vrc_osc) = output_telemetry.vrc_osc.as_mut() {
					vrc_osc.vrchat_detected = vrchat_detected;
					vrc_osc.sent_datagrams = vrc_osc.sent_datagrams.saturating_add(datagrams);
					vrc_osc.sent_packets = vrc_osc.sent_packets.saturating_add(packets);
				}
			}
			Ok(VrcOscOutputEvent::Skipped {
				process_gate_blocked,
				vrchat_detected,
				..
			}) => {
				if let Some(vrc_osc) = output_telemetry.vrc_osc.as_mut() {
					vrc_osc.vrchat_detected = vrchat_detected;
					vrc_osc.skipped_frames = vrc_osc.skipped_frames.saturating_add(1);
					if process_gate_blocked {
						vrc_osc.process_gate_blocked_frames = vrc_osc.process_gate_blocked_frames.saturating_add(1);
					}
				}
			}
			Ok(VrcOscOutputEvent::Error { message, .. }) => {
				*health = format!("VRC OSC output failed: {message}");
				if let Some(vrc_osc) = output_telemetry.vrc_osc.as_mut() {
					vrc_osc.error_count = vrc_osc.error_count.saturating_add(1);
					vrc_osc.last_error = Some(message);
				}
			}
			Ok(VrcOscOutputEvent::Stopped { .. }) => {}
			Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
		}
	}
}

struct CoreRuntimeWorkers {
	frame_streams: Vec<LatestMotionFrameStreamWorkerHandle>,
	frame_stream_errors: Vec<(StreamId, String)>,
	vmc_output: Option<VmcOutputWorkerHandle>,
	output_event_rx: Receiver<VmcOutputEvent>,
	vrc_osc_output: Option<VrcOscOutputWorkerHandle>,
	vrc_osc_event_rx: Receiver<VrcOscOutputEvent>,
	zenoh_output: Option<ZenohOutputWorkerHandle>,
	zenoh_event_rx: Receiver<ZenohOutputEvent>,
}

impl CoreRuntimeWorkers {
	/// 起動済み output worker から `OutputTelemetry` の初期値を組み立てる。
	/// 起動成功した stage のみ `Some(...)` で表現し、未設定の stage は `None` のまま。
	/// 「送信先 / トピックが何か」を Supervisor から目視するために使う。
	fn initial_output_telemetry(&self) -> OutputTelemetry {
		let mut telemetry = OutputTelemetry {
			zenoh: self.zenoh_output.as_ref().map(|handle| ZenohOutputTelemetry {
				base_key_expr: Some(handle.base_key_expr.clone()),
				..ZenohOutputTelemetry::default()
			}),
			vmc: self.vmc_output.as_ref().map(|handle| VmcOutputTelemetry {
				target_addr: Some(handle.target_addr.to_string()),
				..VmcOutputTelemetry::default()
			}),
			vrc_osc: self.vrc_osc_output.as_ref().map(|handle| VrcOscOutputTelemetry {
				target_addr: Some(handle.target_addr.to_string()),
				..VrcOscOutputTelemetry::default()
			}),
			sources: Vec::new(),
		};
		self.refresh_source_telemetry(&mut telemetry);
		telemetry
	}

	/// 各 `MotionFrameStreamWorkerHandle` が持つ `SourceTelemetryHandle`
	/// (Arc<SourceStageAtomics>) を `load(Relaxed)` で読み、`OutputTelemetry.sources` をその場で組み替える。
	///
	/// この関数は Capturer 側 runtime loop のメインスレッドから `runtime_snapshot`
	/// を発行する直前に呼ばれる。worker thread は同じ Atomics に
	/// `fetch_add(Relaxed)` で書き込むだけなので競合は起きず、ロックフリー。
	fn refresh_source_telemetry(&self, telemetry: &mut OutputTelemetry) {
		telemetry.sources.clear();
		for handle in &self.frame_streams {
			if let Some(stage) = &handle.telemetry {
				telemetry.sources.push(stage.snapshot_with_stream(handle.stream_id.0.clone()));
			}
		}
	}
}

impl CoreRuntimeWorkers {
	fn start(config: &CoreRuntimeConfig) -> anyhow::Result<Self> {
		let mut frame_streams = Vec::new();
		let mut frame_stream_errors = Vec::new();
		for stream in &config.frame_streams {
			match open_motion_frame_source(stream) {
				Ok(Some(source)) => {
					frame_streams.push(spawn_latest_motion_frame_stream_worker(
						MotionFrameStreamConfig::new(stream.stream_id.clone()).with_stale_after_ns(STALE_AFTER_NS),
						source,
						FLOW_IDLE_SLEEP,
					)?);
				}
				Ok(None) => {}
				Err(error) => {
					tracing::warn!(
						target: "un_motion_core::runtime_host",
						stream_id = %stream.stream_id.0,
						profile_stream_id = %stream.profile_stream_id,
						error = %error,
						"failed to open Motion frame stream source",
					);
					frame_stream_errors.push((stream.stream_id.clone(), error.to_string()));
				}
			}
		}

		let (output_tx, output_event_rx) = mpsc::channel();
		let vmc_output = config
			.vmc_output
			.as_ref()
			.map(|output| {
				spawn_vmc_output_worker(
					VmcOutputConfig::new(output.target_addr).with_modifier(output.modifier.clone()),
					output_tx,
				)
			})
			.transpose()?;

		let (vrc_osc_tx, vrc_osc_event_rx) = mpsc::channel();
		let vrc_osc_output = config
			.vrc_osc_output
			.as_ref()
			.map(|output| {
				spawn_vrc_osc_output_worker(
					VrcOscOutputConfig::new(output.target_addr)
						.with_parameter_prefix(output.parameter_prefix.clone())
						.with_process_gate(output.send_only_when_vrchat_running, output.process_poll_interval)
						.with_modifier(output.modifier.clone()),
					vrc_osc_tx,
				)
			})
			.transpose()?;

		let (zenoh_tx, zenoh_event_rx) = mpsc::channel();
		let zenoh_output = match config.zenoh_output.as_ref() {
			Some(zenoh) => {
				let mut zenoh_config = ZenohOutputConfig::new(zenoh.base_key_expr.clone())
					.with_topic_mode(zenoh.topic_mode.to_zenoh_mode())
					.with_modifier(zenoh.modifier.clone());
				if let Some(stream_id) = &zenoh.stream_id {
					zenoh_config = zenoh_config.with_stream_id(stream_id.clone());
				}
				if let Some(producer) = &zenoh.producer {
					zenoh_config = zenoh_config.with_producer(producer.clone());
				}
				let expected_dt_ns = (1_000_000_000_u64).checked_div(config.fps as u64).unwrap_or(0);
				if expected_dt_ns > 0 {
					zenoh_config = zenoh_config.with_expected_dt_ns(expected_dt_ns);
				}
				match spawn_zenoh_output_worker(zenoh_config, zenoh_tx) {
					Ok(handle) => Some(handle),
					Err(error) => {
						return Err(error.context("failed to start Zenoh output worker"));
					}
				}
			}
			None => None,
		};

		Ok(Self {
			frame_streams,
			frame_stream_errors,
			vmc_output,
			output_event_rx,
			vrc_osc_output,
			vrc_osc_event_rx,
			zenoh_output,
			zenoh_event_rx,
		})
	}

	fn stop(self) {
		for stream in self.frame_streams {
			let _ = stream.join();
		}
		if let Some(output) = self.vmc_output {
			let _ = output.join();
		}
		if let Some(output) = self.vrc_osc_output {
			let _ = output.join();
		}
		if let Some(output) = self.zenoh_output {
			let _ = output.join();
		}
	}
}

fn default_native_input_component() -> &'static str {
	if cfg!(windows) { "webcam-directshow" } else { "webcam-nokhwa" }
}

fn normalize_mediapipe_engine_id(value: &str) -> &str {
	match value.to_ascii_lowercase().replace('_', "-").as_str() {
		"media-pipe-native" | "mediapipe-native" => "mediapipe-native",
		_ => value,
	}
}

fn normalize_media_pipe_running_mode(value: &str) -> String {
	match value.to_ascii_lowercase().replace('_', "-").as_str() {
		"live" | "livestream" | "live-stream" => "live-stream".to_string(),
		"image" => "image".to_string(),
		"video" => "video".to_string(),
		_ => "live-stream".to_string(),
	}
}

fn parse_resolution(value: &str) -> Option<(u32, u32)> {
	let (width, height) = value.split_once('x')?;
	Some((width.parse().ok()?, height.parse().ok()?))
}

fn core_media_pipe_post_process_settings(runtime: Option<&ProfileRuntimeSettings>) -> CoreMediaPipePostProcessSettings {
	let modifier = runtime.and_then(|runtime| runtime.modifier.as_ref());
	let mut config = CoreMediaPipePostProcessSettings::default();
	apply_modifier_settings(&mut config, modifier);
	config.post_process_rules = core_unmotion_post_process_rules_config(modifier.and_then(|modifier| modifier.post_process_rules.as_ref()));
	config.face_pose_model = modifier.and_then(|modifier| modifier.face_pose_model.as_ref()).and_then(|model| {
		let enabled = model.enabled.unwrap_or(false);
		let neutral = model.neutral_nose_drop_eye_mouth?;
		enabled.then_some(CoreFacePoseModelConfig {
			enabled,
			neutral_nose_drop_eye_mouth: neutral,
		})
	});
	// Modifier (`modifier_config_from_runtime`) を single-source-of-truth にするため、
	// Engine 側 (MediaPipe Native の
	// post-process) に渡す `*_enabled` は all-on で固定する。Profile の
	// `modifier.*_enabled` は Capturer 出力段の Modifier だけが読む形に揃え、
	// VMC 出力 / Zenoh 出力どちらでも同じ Profile 設定で同じ bone subset が
	// 効くようにする。Engine 内部の処理量最適化ヒントとしての役割は将来再設計する。
	config.head_enabled = true;
	config.face_enabled = true;
	config.hands_enabled = true;
	config.arms_ik_enabled = true;
	config.torso_enabled = true;
	config.legs_enabled = true;
	config.feet_enabled = true;
	// Engine 側 (MediaPipe signal-level) の mirror を無効化し、bone-transform level の
	// `MirrorStage` に処理を完全移管。
	// Profile の `modifier.mirror_mode` は `modifier_config_from_runtime` 経由で
	// Modifier 側に届くので、ここでは常に `"normal"` (passthrough) に上書きする。
	// 二重 mirror を避けるための一元化措置。
	config.mirror_mode = "normal".to_string();
	config
}

fn apply_modifier_settings(config: &mut CoreMediaPipePostProcessSettings, modifier: Option<&ProfileModifierSettings>) {
	let Some(modifier) = modifier else {
		return;
	};
	if let Some(value) = modifier.head_enabled {
		config.head_enabled = value;
	}
	if let Some(value) = modifier.face_enabled {
		config.face_enabled = value;
	}
	if let Some(value) = modifier.hands_enabled {
		config.hands_enabled = value;
	}
	if let Some(value) = modifier.arms_ik_enabled {
		config.arms_ik_enabled = value;
	}
	if let Some(value) = modifier.torso_enabled {
		config.torso_enabled = value;
	}
	if let Some(value) = modifier.legs_enabled {
		config.legs_enabled = value;
	}
	if let Some(value) = modifier.feet_enabled {
		config.feet_enabled = value;
	}
	if let Some(value) = modifier.camera_diagonal_view_angle_deg {
		config.camera_diagonal_view_angle_deg = value;
	}
	if let Some(value) = modifier.min_landmark_confidence {
		config.min_landmark_confidence = value;
	}
	if let Some(value) = modifier.eye_open_bias {
		config.eye_open_bias = value.clamp(0.0, 1.0);
	}
	if let Some(value) = modifier.mirror_mode.clone() {
		config.mirror_mode = value;
	}
}

fn core_unmotion_post_process_rules_config(rules: Option<&ProfileMediaPipeAdvancedSettings>) -> CoreUnmotionPostProcessRulesConfig {
	let mut config = CoreUnmotionPostProcessRulesConfig::default();
	let Some(rules) = rules else {
		return config;
	};
	if let Some(value) = rules.anatomical_constraints {
		config.anatomical_constraints = value;
	}
	if let Some(value) = rules.hold_lost_landmarks {
		config.hold_lost_landmarks = value;
	}
	if let Some(value) = rules.ease_recovery {
		config.ease_recovery = value;
	}
	if let Some(value) = rules.limit_rotation_jumps {
		config.limit_rotation_jumps = value;
	}
	if let Some(value) = rules.head_source_switch_blend {
		config.head_source_switch_blend = value;
	}
	if let Some(value) = rules.lost_signal_behavior.clone() {
		config.lost_signal_behavior = value;
	}
	if let Some(value) = rules.lost_signal_rest_pose_blend {
		config.lost_signal_rest_pose_blend = value.clamp(0.0, 1.0);
	}
	if let Some(value) = rules.lost_signal_hold_seconds {
		config.lost_signal_hold_seconds = value.clamp(0.0, 30.0);
	}
	if let Some(value) = rules.lost_signal_head_behavior.clone() {
		config.lost_signal_head_behavior = value;
	}
	if let Some(value) = rules.lost_signal_head_rest_pose_blend {
		config.lost_signal_head_rest_pose_blend = value.clamp(0.0, 1.0);
	}
	if let Some(value) = rules.lost_signal_head_hold_seconds {
		config.lost_signal_head_hold_seconds = value.clamp(0.0, 30.0);
	}
	if let Some(value) = rules.lost_signal_hands_behavior.clone() {
		config.lost_signal_hands_behavior = value;
	}
	if let Some(value) = rules.lost_signal_hands_rest_pose_blend {
		config.lost_signal_hands_rest_pose_blend = value.clamp(0.0, 1.0);
	}
	if let Some(value) = rules.lost_signal_hands_hold_seconds {
		config.lost_signal_hands_hold_seconds = value.clamp(0.0, 30.0);
	}
	if let Some(value) = rules.lost_signal_arms_behavior.clone() {
		config.lost_signal_arms_behavior = value;
	}
	if let Some(value) = rules.lost_signal_arms_rest_pose_blend {
		config.lost_signal_arms_rest_pose_blend = value.clamp(0.0, 1.0);
	}
	if let Some(value) = rules.lost_signal_arms_hold_seconds {
		config.lost_signal_arms_hold_seconds = value.clamp(0.0, 30.0);
	}
	if let Some(value) = rules.lost_signal_recovery_seconds {
		config.lost_signal_recovery_seconds = value.clamp(0.0, 5.0);
	}
	if let Some(value) = rules.head_from_pose {
		config.head_from_pose = value;
	}
	if let Some(value) = rules.head_from_face_matrix {
		config.head_from_face_matrix = value;
	}
	if let Some(value) = rules.head_reconcile {
		config.head_reconcile = value;
	}
	if let Some(value) = rules.neutral_eye_fallback {
		config.neutral_eye_fallback = value;
	}
	if let Some(value) = rules.hand_camera_target {
		config.hand_camera_target = value;
	}
	if let Some(value) = rules.hand_orientation {
		config.hand_orientation = value;
	}
	if let Some(value) = rules.finger_derived {
		config.finger_derived = value;
	}
	if let Some(value) = rules.arm_from_pose {
		config.arm_from_pose = value;
	}
	if let Some(value) = rules.arm_ik_from_hands {
		config.arm_ik_from_hands = value;
	}
	if let Some(value) = rules.crossed_hand_heuristic {
		config.crossed_hand_heuristic = value;
	}
	if let Some(value) = rules.coordinate_correction {
		config.coordinate_correction = value;
	}
	if let Some(value) = rules.final_clamp {
		config.final_clamp = value;
	}
	config
}

fn unmotion_runtime_stream_id(profile_stream_id: &str) -> StreamId {
	StreamId::new(format!("unmotion:{profile_stream_id}"))
}

fn now_unix_ns() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_nanos() as u64)
		.unwrap_or_default()
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_frame::{ExpressionSample, FaceMotion, SampleState, TrackingState, UNMotionFrame};
	use un_motion_profile_schema::{CoreProfileDocument, CoreProfileDocumentProfile};
	use un_motion_runtime::{LatestMotionFrame, LatestMotionFrameSlot, StreamHealth, stream_state_from_unmotion_frame};

	fn profile(runtime_selection: Option<ProfileRuntimeSettings>) -> CoreProfileDocumentProfile {
		CoreProfileDocumentProfile {
			id: "waidayo".to_string(),
			name: "Waidayo".to_string(),
			created_at: String::new(),
			note: String::new(),
			icon_path: None,
			group: String::new(),
			default_source_enabled: false,
			default_source_label: "UNMotion Default".to_string(),
			runtime_selection,
			pipeline_components: None,
		}
	}

	fn test_frame_stream_config(profile_stream_id: &str) -> CoreMotionFrameStreamConfig {
		core_frame_stream_config(
			profile_stream_id.to_string(),
			StreamId::new(format!("unmotion:{profile_stream_id}")),
			None,
			None,
		)
	}

	fn pending_face_pose_model(samples: &[(f32, f32, f32, f32)]) -> PendingFacePoseModel {
		let (tx, _rx) = mpsc::channel();
		let mut model = PendingFacePoseModel::new(samples.len().max(1), tx);
		for (nose_drop_eye_mouth, yaw, roll, confidence) in samples {
			model.nose_drop_eye_mouth.push(*nose_drop_eye_mouth);
			model.abs_yaw.push(yaw.abs());
			model.abs_roll.push(roll.abs());
			model.confidence.push(*confidence);
			model.valid_samples += 1;
		}
		model
	}

	#[test]
	fn face_pose_model_quality_accepts_stable_frontal_samples() {
		let mut model = pending_face_pose_model(&[
			(0.632, 0.04, 0.03, 0.92),
			(0.638, 0.06, 0.02, 0.91),
			(0.635, 0.05, 0.04, 0.93),
			(0.636, 0.03, 0.03, 0.90),
		]);

		let result = summarize_face_pose_model(&mut model).expect("stable frontal model");

		assert_eq!(result.valid_samples, 4);
		assert!((result.neutral_nose_drop_eye_mouth - 0.6355).abs() < 0.0001);
		assert!((result.median_abs_yaw - 0.045).abs() < 0.0001);
		assert!((result.median_abs_roll - 0.03).abs() < 0.0001);
	}

	#[test]
	fn face_pose_model_quality_rejects_non_frontal_samples() {
		let mut model = pending_face_pose_model(&[
			(0.632, 0.28, 0.03, 0.92),
			(0.638, 0.30, 0.02, 0.91),
			(0.635, 0.26, 0.04, 0.93),
			(0.636, 0.27, 0.03, 0.90),
		]);

		let error = summarize_face_pose_model(&mut model).expect_err("non-frontal model should fail");

		assert!(error.to_string().contains("顔を正面"));
	}

	#[test]
	fn face_pose_model_quality_rejects_unstable_samples() {
		let mut model = pending_face_pose_model(&[
			(0.52, 0.04, 0.03, 0.92),
			(0.63, 0.06, 0.02, 0.91),
			(0.75, 0.05, 0.04, 0.93),
			(0.64, 0.03, 0.03, 0.90),
		]);

		let error = summarize_face_pose_model(&mut model).expect_err("unstable model should fail");

		assert!(error.to_string().contains("安定していません"));
	}

	#[test]
	fn config_leaves_output_disabled_when_profile_output_is_off() {
		let document = CoreProfileDocument {
			selected_profile_id: "waidayo".to_string(),
			profiles: vec![profile(Some(ProfileRuntimeSettings {
				vmc_enabled: Some(false),
				..Default::default()
			}))],
			profile_sources: Vec::new(),
			next_profile_index: 2,
			next_source_index: 2,
		};

		let config = core_runtime_config_from_document(&document).expect("config");

		assert_eq!(config.fps, FOLLOW_INPUT_OUTPUT_FPS_CAP);
		assert!(config.vmc_output.is_none());
	}

	#[test]
	fn follow_input_uses_high_output_cadence_cap() {
		let mut unmotion_profile = profile(Some(ProfileRuntimeSettings {
			engine: Some("vmc".to_string()),
			vmc_receive_listen_addr: Some("127.0.0.1:39540".to_string()),
			..Default::default()
		}));
		unmotion_profile.default_source_enabled = true;
		let document = CoreProfileDocument {
			selected_profile_id: "waidayo".to_string(),
			profiles: vec![unmotion_profile],
			profile_sources: Vec::new(),
			next_profile_index: 2,
			next_source_index: 2,
		};

		let config = core_runtime_config_from_document(&document).expect("config");

		assert_eq!(config.fps, FOLLOW_INPUT_OUTPUT_FPS_CAP);
		assert_eq!(config.frame_streams[0].input_fps, DEFAULT_FPS);
	}

	#[test]
	fn config_reads_unmotion_pipeline_selection() {
		let mut unmotion_profile = profile(Some(ProfileRuntimeSettings {
			engine: Some("mediapipe-native".to_string()),
			device: Some("dshow0:Camera".to_string()),
			resolution: Some("800x600".to_string()),
			media_pipe_running_mode: Some("video".to_string()),
			media_pipe_holistic_enabled: Some(false),
			media_pipe_delegate: Some("cpu".to_string()),
			media_pipe_num_threads: Some(2),
			media_pipe_holistic_flow_limiter_enabled: Some(false),
			media_pipe_holistic_flow_limiter_max_in_flight: Some(3),
			media_pipe_holistic_flow_limiter_max_in_queue: Some(0),
			modifier: Some(ProfileModifierSettings {
				hands_enabled: Some(true),
				camera_diagonal_view_angle_deg: Some(65.0),
				min_landmark_confidence: Some(0.7),
				eye_open_bias: Some(0.8),
				mirror_mode: Some("mirror-output".to_string()),
				post_process_rules: Some(ProfileMediaPipeAdvancedSettings {
					anatomical_constraints: Some(false),
					finger_derived: Some(false),
					..Default::default()
				}),
				..Default::default()
			}),
			..Default::default()
		}));
		unmotion_profile.default_source_enabled = true;
		unmotion_profile.pipeline_components = Some(ProfilePipelineComponents {
			input: Some("file-image".to_string()),
			engine: Some("media-pipe-native".to_string()),
			post_process: Some("none".to_string()),
			input_path: Some("fixture.png".to_string()),
			input_fps: Some(12),
			input_width: Some(640),
			input_height: Some(360),
			input_pixel_format: Some("YUY2".to_string()),
			input_repeat: Some(true),
			input_ffmpeg_path: Some("tools/ffmpeg.exe".to_string()),
			input_denoise_mode: Some("temporal-iir".to_string()),
			input_denoise_temporal_iir_hz: Some(8.0),
			input_resize_enabled: Some(true),
			input_resize_axis: Some("height".to_string()),
			input_resize_pad_color: Some("112233ff".to_string()),
			..Default::default()
		});
		let document = CoreProfileDocument {
			selected_profile_id: "waidayo".to_string(),
			profiles: vec![unmotion_profile],
			profile_sources: Vec::new(),
			next_profile_index: 2,
			next_source_index: 2,
		};

		let config = core_runtime_config_from_document(&document).expect("config");

		assert_eq!(config.frame_streams[0].runtime_engine, "mediapipe-native");
		assert_eq!(config.frame_streams[0].device_id, "dshow0:Camera");
		assert_eq!(config.frame_streams[0].input_component, "file-image");
		assert_eq!(config.frame_streams[0].input_path.as_deref(), Some("fixture.png"));
		assert_eq!(config.frame_streams[0].input_fps, 12);
		assert_eq!(config.fps, FOLLOW_INPUT_OUTPUT_FPS_CAP);
		assert_eq!(config.frame_streams[0].input_width, Some(640));
		assert_eq!(config.frame_streams[0].input_height, Some(360));
		assert_eq!(config.frame_streams[0].input_pixel_format.as_deref(), Some("YUY2"));
		assert!(config.frame_streams[0].input_repeat);
		assert_eq!(config.frame_streams[0].input_ffmpeg_path.as_deref(), Some("tools/ffmpeg.exe"));
		assert_eq!(config.frame_streams[0].input_denoise_mode, "temporal-iir");
		assert_eq!(config.frame_streams[0].input_denoise_temporal_iir_hz, 8.0);
		assert_eq!(
			config.frame_streams[0].input_resize.as_ref().map(|resize| resize.reference),
			Some(360)
		);
		assert_eq!(
			config.frame_streams[0]
				.input_resize
				.as_ref()
				.map(|resize| resize.pad_color.as_str()),
			Some("112233ff")
		);
		assert_eq!(config.frame_streams[0].media_pipe_running_mode, "video");
		assert!(!config.frame_streams[0].media_pipe_holistic_enabled);
		assert_eq!(config.frame_streams[0].media_pipe_delegate.as_deref(), Some("cpu"));
		assert_eq!(config.frame_streams[0].media_pipe_num_threads, Some(2));
		assert!(!config.frame_streams[0].media_pipe_holistic_flow_limiter_enabled);
		assert_eq!(config.frame_streams[0].media_pipe_holistic_flow_limiter_max_in_flight, 3);
		assert_eq!(config.frame_streams[0].media_pipe_holistic_flow_limiter_max_in_queue, 0);
		assert_eq!(config.frame_streams[0].post_process_component, "none");
		assert!(config.frame_streams[0].media_pipe_post_process.hands_enabled);
		assert_eq!(config.frame_streams[0].media_pipe_post_process.camera_diagonal_view_angle_deg, 65.0);
		assert_eq!(config.frame_streams[0].media_pipe_post_process.min_landmark_confidence, 0.7);
		assert_eq!(config.frame_streams[0].media_pipe_post_process.eye_open_bias, 0.8);
		// Engine 側 mirror_mode は常に "normal" に固定。Profile の "mirror-output" 指定は
		// Modifier 側 (zenoh_output.modifier.mirror.mode /
		// vmc_output.modifier.mirror.mode) で MirrorMode::MirrorOutput として
		// 反映される。本テストは profile に Zenoh/VMC output が無いため、
		// `modifier_config_from_runtime` を直接呼んで Modifier 側マッピングを確認する。
		assert_eq!(config.frame_streams[0].media_pipe_post_process.mirror_mode, "normal");
		assert!(
			!config.frame_streams[0]
				.media_pipe_post_process
				.post_process_rules
				.anatomical_constraints
		);
		assert!(!config.frame_streams[0].media_pipe_post_process.post_process_rules.finger_derived);
		let runtime = document.profiles[0].runtime_selection.as_ref();
		let modifier = modifier_config_from_runtime(runtime);
		assert_eq!(modifier.mirror.mode, un_motion_runtime::MirrorMode::MirrorOutput);
	}

	#[test]
	fn explicit_runtime_fps_overrides_input_fps_for_output_cadence() {
		let mut unmotion_profile = profile(Some(ProfileRuntimeSettings {
			fps: Some(90),
			engine: Some("mediapipe-native".to_string()),
			..Default::default()
		}));
		unmotion_profile.default_source_enabled = true;
		unmotion_profile.pipeline_components = Some(ProfilePipelineComponents {
			input_fps: Some(60),
			..Default::default()
		});
		let document = CoreProfileDocument {
			selected_profile_id: "waidayo".to_string(),
			profiles: vec![unmotion_profile],
			profile_sources: Vec::new(),
			next_profile_index: 2,
			next_source_index: 2,
		};

		let config = core_runtime_config_from_document(&document).expect("config");

		assert_eq!(config.frame_streams[0].input_fps, 60);
		assert_eq!(config.fps, 90);
	}

	#[test]
	fn file_image_uses_explicit_output_fps_as_repeat_clock() {
		let mut unmotion_profile = profile(Some(ProfileRuntimeSettings {
			fps: Some(60),
			engine: Some("mediapipe-native".to_string()),
			..Default::default()
		}));
		unmotion_profile.default_source_enabled = true;
		unmotion_profile.pipeline_components = Some(ProfilePipelineComponents {
			input: Some("file-image".to_string()),
			input_fps: Some(30),
			input_repeat: Some(true),
			..Default::default()
		});
		let document = CoreProfileDocument {
			selected_profile_id: "dev-y".to_string(),
			profiles: vec![unmotion_profile],
			profile_sources: Vec::new(),
			next_profile_index: 2,
			next_source_index: 2,
		};

		let config = core_runtime_config_from_document(&document).expect("config");

		assert_eq!(config.frame_streams[0].input_fps, 60);
		assert_eq!(config.fps, 60);
	}

	/// `runtime_selection.modifier.smoothing_preset` (TOML 上は `smoothingPreset`) 文字列を
	/// `SmoothingPreset` enum に正しくマップする。
	#[test]
	fn modifier_config_maps_smoothing_preset_strings() {
		use un_motion_runtime::SmoothingPreset;

		fn modifier_for(preset: &str) -> un_motion_runtime::ModifierConfig {
			let settings = ProfileRuntimeSettings {
				modifier: Some(ProfileModifierSettings {
					smoothing_preset: Some(preset.to_string()),
					..Default::default()
				}),
				..Default::default()
			};
			modifier_config_from_runtime(Some(&settings))
		}

		assert_eq!(modifier_for("off").smoothing.preset, SmoothingPreset::Off);
		assert_eq!(modifier_for("low").smoothing.preset, SmoothingPreset::Low);
		assert_eq!(modifier_for("medium").smoothing.preset, SmoothingPreset::Medium);
		assert_eq!(modifier_for("high").smoothing.preset, SmoothingPreset::High);
		assert_eq!(modifier_for("adaptive").smoothing.preset, SmoothingPreset::Adaptive);
		// 大文字小文字を区別しない。
		assert_eq!(modifier_for("ADAPTIVE").smoothing.preset, SmoothingPreset::Adaptive);
		assert_eq!(modifier_for("Medium").smoothing.preset, SmoothingPreset::Medium);
		// unknown value は panic せず Off (pass-through) に倒す。
		assert_eq!(modifier_for("bogus").smoothing.preset, SmoothingPreset::Off);
	}

	#[test]
	fn modifier_config_maps_adaptive_smoothing_parameters() {
		use un_motion_runtime::SmoothingPreset;

		let settings = ProfileRuntimeSettings {
			modifier: Some(ProfileModifierSettings {
				smoothing_preset: Some("adaptive".to_string()),
				adaptive_min_cutoff_hz: Some(0.25),
				adaptive_beta: Some(0.12),
				adaptive_derivative_cutoff_hz: Some(1.7),
				..Default::default()
			}),
			..Default::default()
		};
		let modifier = modifier_config_from_runtime(Some(&settings));

		assert_eq!(modifier.smoothing.preset, SmoothingPreset::Adaptive);
		assert!((modifier.smoothing.adaptive_min_cutoff_hz - 0.25).abs() < f32::EPSILON);
		assert!((modifier.smoothing.adaptive_beta - 0.12).abs() < f32::EPSILON);
		assert!((modifier.smoothing.adaptive_derivative_cutoff_hz - 1.7).abs() < f32::EPSILON);
	}

	#[test]
	fn modifier_config_maps_explicit_composable_smoothing_parameters() {
		let settings = ProfileRuntimeSettings {
			modifier: Some(ProfileModifierSettings {
				smoothing_preset: Some("off".to_string()),
				smoothing_ema_enabled: Some(true),
				smoothing_ema_alpha: Some(0.35),
				smoothing_one_euro_enabled: Some(true),
				smoothing_confidence_adaptive_cutoff: Some(true),
				adaptive_min_cutoff_hz: Some(0.4),
				adaptive_beta: Some(0.11),
				adaptive_derivative_cutoff_hz: Some(1.4),
				..Default::default()
			}),
			..Default::default()
		};
		let modifier = modifier_config_from_runtime(Some(&settings));

		assert!(modifier.smoothing.ema_enabled);
		assert!((modifier.smoothing.ema_alpha - 0.35).abs() < f32::EPSILON);
		assert!(modifier.smoothing.one_euro_enabled);
		assert!(modifier.smoothing.confidence_adaptive_cutoff_enabled);
		assert!((modifier.smoothing.adaptive_min_cutoff_hz - 0.4).abs() < f32::EPSILON);
		assert!((modifier.smoothing.adaptive_beta - 0.11).abs() < f32::EPSILON);
		assert!((modifier.smoothing.adaptive_derivative_cutoff_hz - 1.4).abs() < f32::EPSILON);
	}

	/// `runtime_selection.modifier` が `None` のときは `ModifierConfig::default()`
	/// が返り、smoothing は Off (pass-through)。
	#[test]
	fn modifier_config_defaults_to_off_when_modifier_missing() {
		use un_motion_runtime::SmoothingPreset;
		let modifier = modifier_config_from_runtime(None);
		assert_eq!(modifier.smoothing.preset, SmoothingPreset::Off);
	}

	#[test]
	fn initial_runtime_snapshot_lists_configured_streams_before_signal() {
		let config = CoreRuntimeConfig {
			active_profile_id: "waidayo".to_string(),
			fps: 90,
			vmc_output: None,
			vrc_osc_output: None,
			zenoh_output: None,
			frame_streams: vec![test_frame_stream_config("mediapipe-main")],
		};
		let (status_tx, status_rx) = mpsc::channel();
		let host = CoreRuntimeHost::spawn(
			config,
			Arc::new(move |update| {
				let _ = status_tx.send(update);
			}),
		)
		.expect("runtime host");

		let update = status_rx.recv_timeout(Duration::from_secs(1)).expect("initial status");
		host.join().expect("join host");
		let snapshot = update.runtime_snapshot.expect("snapshot");

		assert!(
			snapshot
				.streams
				.iter()
				.any(|stream| stream.stream_id == StreamId::new("unmotion:mediapipe-main") && stream.health == StreamHealth::NoSignal)
		);
	}

	#[test]
	fn runtime_host_refreshes_latest_frame_slot_into_snapshot() {
		let config = CoreRuntimeConfig {
			active_profile_id: "unmotion-profile".to_string(),
			fps: 90,
			vmc_output: None,
			vrc_osc_output: None,
			zenoh_output: None,
			frame_streams: vec![test_frame_stream_config("mediapipe-main")],
		};
		let mut latest_states = initial_stream_states(&config);
		let (status_tx, status_rx) = mpsc::channel();
		let status_callback: StatusCallback = Arc::new(move |update| {
			let _ = status_tx.send(update);
		});
		let mut frame = UNMotionFrame::new(7);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "jawOpen".to_string(),
				value: 0.7,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});
		let observed_at_ns = now_unix_ns();
		let state = stream_state_from_unmotion_frame(StreamId::new("unmotion:mediapipe-main"), &frame, observed_at_ns);
		let slot = LatestMotionFrameSlot::new();
		slot.store(LatestMotionFrame {
			stream_id: StreamId::new("unmotion:mediapipe-main"),
			frame: Arc::new(frame),
			snapshot: state.snapshot_at(observed_at_ns, STALE_AFTER_NS),
			state: Arc::new(state),
		});
		let mut packet_count = 0;
		let mut health = String::new();

		let output_telemetry = OutputTelemetry::default();
		let mut latest_unmotion_frame = None;
		let received = refresh_latest_motion_frames_from_loaded(
			std::iter::once(slot.load().expect("latest frame")),
			&mut latest_states,
			&mut latest_unmotion_frame,
			&mut packet_count,
			&mut health,
			"unmotion-profile",
			0,
			&config,
			&output_telemetry,
			&status_callback,
		);

		assert!(received);
		assert_eq!(packet_count, 1);
		assert_eq!(health, "streaming");
		assert_eq!(latest_states[&StreamId::new("unmotion:mediapipe-main")].blendshapes["jawOpen"], 0.7);
		let update = status_rx.recv_timeout(Duration::from_secs(1)).expect("status");
		let snapshot = update.runtime_snapshot.expect("snapshot");
		assert!(
			snapshot
				.streams
				.iter()
				.any(|stream| stream.stream_id == StreamId::new("unmotion:mediapipe-main") && stream.health == StreamHealth::Live)
		);
	}

	#[test]
	fn runtime_host_selects_only_latest_unsent_frame() {
		let slot = LatestMotionFrameSlot::new();
		for (sequence, jaw_open) in [(1, 0.1), (2, 0.9)] {
			let mut frame = UNMotionFrame::new(sequence);
			frame.header.capture_timestamp_ns = sequence;
			frame.face = Some(FaceMotion {
				tracking_state: TrackingState::Valid,
				confidence: 1.0,
				head: None,
				expressions: vec![ExpressionSample {
					name: "jawOpen".to_string(),
					value: jaw_open,
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				}],
			});
			let observed_at_ns = now_unix_ns();
			let state = stream_state_from_unmotion_frame(StreamId::new("unmotion:vmc-main"), &frame, observed_at_ns);
			slot.store(LatestMotionFrame {
				stream_id: StreamId::new("unmotion:vmc-main"),
				frame: Arc::new(frame),
				snapshot: state.snapshot_at(observed_at_ns, STALE_AFTER_NS),
				state: Arc::new(state),
			});
		}

		let mut last_output_sequences = HashMap::new();
		let sent = select_latest_unsent_frame_from_loaded(std::iter::once(slot.load().expect("latest")), &mut last_output_sequences)
			.expect("selected frame");

		assert_eq!(sent.header.sequence, 2);
		assert!(
			select_latest_unsent_frame_from_loaded(std::iter::once(slot.load().expect("latest")), &mut last_output_sequences).is_none()
		);
	}

	/// VMC output worker から飛んでくる `Sent`/`Error` event を
	/// `OutputTelemetry::vmc` に累積できるか検証する。
	#[test]
	fn drain_output_messages_accumulates_vmc_sent_and_error() {
		let (tx, rx) = mpsc::channel::<VmcOutputEvent>();
		let target_addr: SocketAddr = "127.0.0.1:39539".parse().expect("addr");
		let mut telemetry = OutputTelemetry {
			vmc: Some(VmcOutputTelemetry {
				target_addr: Some(target_addr.to_string()),
				..VmcOutputTelemetry::default()
			}),
			..OutputTelemetry::default()
		};
		let mut health = String::new();

		tx.send(VmcOutputEvent::Sent {
			target_addr,
			datagrams: 1,
			packets: 4,
		})
		.expect("send 1");
		tx.send(VmcOutputEvent::Sent {
			target_addr,
			datagrams: 1,
			packets: 5,
		})
		.expect("send 2");
		tx.send(VmcOutputEvent::Error {
			target_addr,
			message: "network unreachable".to_string(),
		})
		.expect("send err");

		drain_output_messages(&rx, &mut health, &mut telemetry);

		let vmc = telemetry.vmc.expect("vmc telemetry");
		assert_eq!(vmc.sent_datagrams, 2);
		assert_eq!(vmc.sent_packets, 9);
		assert_eq!(vmc.error_count, 1);
		assert_eq!(vmc.last_error.as_deref(), Some("network unreachable"));
		assert!(health.starts_with("VMC output failed:"));
	}

	/// Zenoh output worker から飛んでくる `Sent`/`Error` event を
	/// `OutputTelemetry::zenoh` に累積できるか検証する。
	#[test]
	fn drain_zenoh_output_messages_accumulates_zenoh_sent_and_error() {
		let (tx, rx) = mpsc::channel::<ZenohOutputEvent>();
		let mut telemetry = OutputTelemetry {
			zenoh: Some(ZenohOutputTelemetry {
				base_key_expr: Some("unmotion/v1/test".to_string()),
				..ZenohOutputTelemetry::default()
			}),
			..OutputTelemetry::default()
		};
		let mut health = String::new();

		tx.send(ZenohOutputEvent::Sent {
			key_expr: "unmotion/v1/test/A".to_string(),
			frames: 1,
		})
		.expect("send 1");
		tx.send(ZenohOutputEvent::Sent {
			key_expr: "unmotion/v1/test/B".to_string(),
			frames: 1,
		})
		.expect("send 2");
		tx.send(ZenohOutputEvent::Error {
			message: "session closed".to_string(),
		})
		.expect("send err");

		drain_zenoh_output_messages(&rx, &mut health, &mut telemetry);

		let zenoh = telemetry.zenoh.expect("zenoh telemetry");
		assert_eq!(zenoh.sent_frames, 2);
		assert_eq!(zenoh.error_count, 1);
		assert_eq!(zenoh.last_error.as_deref(), Some("session closed"));
		assert!(health.starts_with("Zenoh output failed:"));
	}

	#[test]
	fn drain_vrc_osc_output_messages_accumulates_sent_skipped_and_error() {
		let (tx, rx) = mpsc::channel::<VrcOscOutputEvent>();
		let target_addr: SocketAddr = "127.0.0.1:9000".parse().expect("addr");
		let mut telemetry = OutputTelemetry {
			vrc_osc: Some(VrcOscOutputTelemetry {
				target_addr: Some(target_addr.to_string()),
				..VrcOscOutputTelemetry::default()
			}),
			..OutputTelemetry::default()
		};
		let mut health = String::new();

		tx.send(VrcOscOutputEvent::Sent {
			target_addr,
			datagrams: 1,
			packets: 3,
			vrchat_detected: true,
		})
		.expect("send sent");
		tx.send(VrcOscOutputEvent::Skipped {
			target_addr,
			process_gate_blocked: true,
			vrchat_detected: false,
		})
		.expect("send skipped");
		tx.send(VrcOscOutputEvent::Error {
			target_addr,
			message: "send failed".to_string(),
		})
		.expect("send err");

		drain_vrc_osc_output_messages(&rx, &mut health, &mut telemetry);

		let vrc_osc = telemetry.vrc_osc.expect("vrc osc telemetry");
		assert_eq!(vrc_osc.sent_datagrams, 1);
		assert_eq!(vrc_osc.sent_packets, 3);
		assert_eq!(vrc_osc.skipped_frames, 1);
		assert_eq!(vrc_osc.process_gate_blocked_frames, 1);
		assert_eq!(vrc_osc.error_count, 1);
		assert_eq!(vrc_osc.last_error.as_deref(), Some("send failed"));
		assert!(health.starts_with("VRC OSC output failed:"));
	}

	/// `RuntimeSnapshot` を構築するときに `output_telemetry` が forwarding されていることを
	/// 確認するスモークテスト。Supervisor から GUI まで届くために必須。
	#[test]
	fn runtime_snapshot_forwards_output_telemetry() {
		let config = CoreRuntimeConfig {
			active_profile_id: "p".to_string(),
			fps: 30,
			frame_streams: Vec::new(),
			vmc_output: None,
			vrc_osc_output: None,
			zenoh_output: None,
		};
		let telemetry = OutputTelemetry {
			zenoh: Some(ZenohOutputTelemetry {
				base_key_expr: Some("unmotion/v1/test".to_string()),
				sent_frames: 42,
				..ZenohOutputTelemetry::default()
			}),
			vmc: Some(VmcOutputTelemetry {
				target_addr: Some("127.0.0.1:39539".to_string()),
				sent_datagrams: 10,
				sent_packets: 30,
				..VmcOutputTelemetry::default()
			}),
			vrc_osc: Some(VrcOscOutputTelemetry {
				target_addr: Some("127.0.0.1:9000".to_string()),
				sent_datagrams: 4,
				sent_packets: 12,
				..VrcOscOutputTelemetry::default()
			}),
			sources: Vec::new(),
		};

		let snapshot = runtime_snapshot(&config, &HashMap::new(), RuntimeState::Running, &telemetry);

		let zenoh = snapshot.output_telemetry.zenoh.expect("zenoh telemetry");
		assert_eq!(zenoh.sent_frames, 42);
		assert_eq!(zenoh.base_key_expr.as_deref(), Some("unmotion/v1/test"));
		let vmc = snapshot.output_telemetry.vmc.expect("vmc telemetry");
		assert_eq!(vmc.sent_datagrams, 10);
		assert_eq!(vmc.sent_packets, 30);
		assert_eq!(vmc.target_addr.as_deref(), Some("127.0.0.1:39539"));
		let vrc_osc = snapshot.output_telemetry.vrc_osc.expect("vrc osc telemetry");
		assert_eq!(vrc_osc.sent_datagrams, 4);
		assert_eq!(vrc_osc.sent_packets, 12);
		assert_eq!(vrc_osc.target_addr.as_deref(), Some("127.0.0.1:9000"));
	}

	#[test]
	fn neutral_calibration_summary_keeps_head_baseline() {
		let captured = [0.0, (std::f32::consts::FRAC_PI_4).sin(), 0.0, (std::f32::consts::FRAC_PI_4).cos()];
		let mut samples = BTreeMap::new();
		samples.insert("Head".to_string(), vec![captured]);

		let summary = summarize_neutral_rotations(&samples, NeutralCalibrationPose::U);
		let stored = summary["Head"];

		assert_quat_array_near(stored, captured, 1e-5);
	}

	#[test]
	fn neutral_calibration_summary_ignores_t_pose_limb_baselines() {
		let mut samples = BTreeMap::new();
		samples.insert("LeftUpperArm".to_string(), vec![[0.2, 0.1, 0.0, 0.97]]);
		samples.insert("LeftLowerArm".to_string(), vec![[0.7, 0.1, 0.0, 0.7]]);
		samples.insert("LeftHand".to_string(), vec![[0.1, 0.2, 0.3, 0.9]]);
		samples.insert("RightUpperArm".to_string(), vec![[0.2, -0.1, 0.0, 0.97]]);
		samples.insert("RightLowerArm".to_string(), vec![[0.7, -0.1, 0.0, 0.7]]);
		samples.insert("RightHand".to_string(), vec![[0.1, -0.2, -0.3, 0.9]]);

		let summary = summarize_neutral_rotations(&samples, NeutralCalibrationPose::T);

		assert!(summary.is_empty());
		assert!(!summary.contains_key("LeftUpperArm"));
		assert!(!summary.contains_key("RightUpperArm"));
		assert!(!summary.contains_key("LeftLowerArm"));
		assert!(!summary.contains_key("LeftHand"));
		assert!(!summary.contains_key("RightLowerArm"));
		assert!(!summary.contains_key("RightHand"));
	}

	#[test]
	fn neutral_calibration_t_pose_target_documents_wrist_front_forearm_twist_without_saving_limb_offset() {
		let actual = [0.0, 0.0, 0.0, 1.0];
		let mut samples = BTreeMap::new();
		samples.insert("RightLowerArm".to_string(), vec![actual]);

		let summary = summarize_neutral_rotations(&samples, NeutralCalibrationPose::T);
		assert!(!summary.contains_key("RightLowerArm"));

		let target =
			neutral_calibration_target_rotation("RightLowerArm", NeutralCalibrationPose::T).expect("T-wrist-front lower arm target");
		let axis = quat_rotate_vec3(target, [1.0, 0.0, 0.0]);
		let palm = quat_rotate_vec3(target, [0.0, -1.0, 0.0]);
		assert_vec3_near(axis, [1.0, 0.0, 0.0], 1e-5);
		assert_vec3_near(palm, [0.0, 0.0, 1.0], 1e-5);
	}

	#[test]
	fn neutral_calibration_summary_keeps_i_pose_upper_arm_live() {
		let actual = quat_from_to([-1.0, 0.0, 0.0], [-0.2, -0.98, 0.0]);
		let mut samples = BTreeMap::new();
		samples.insert("LeftUpperArm".to_string(), vec![actual]);

		let summary = summarize_neutral_rotations(&samples, NeutralCalibrationPose::I);
		assert!(!summary.contains_key("LeftUpperArm"));
	}

	#[test]
	fn neutral_calibration_summary_keeps_u_pose_upper_arm_live() {
		let actual = quat_from_to([1.0, 0.0, 0.0], [0.30, 0.95, 0.0]);
		let mut samples = BTreeMap::new();
		samples.insert("RightUpperArm".to_string(), vec![actual]);

		let summary = summarize_neutral_rotations(&samples, NeutralCalibrationPose::U);
		assert!(!summary.contains_key("RightUpperArm"));
	}

	#[test]
	fn neutral_calibration_u_pose_limb_target_does_not_create_saved_lower_arm_offset() {
		let actual = quat_from_basis([1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.30, 0.95, 0.0], [0.0, 0.0, 1.0]).unwrap();
		let mut samples = BTreeMap::new();
		samples.insert("RightLowerArm".to_string(), vec![actual]);

		let summary = summarize_neutral_rotations(&samples, NeutralCalibrationPose::U);
		assert!(!summary.contains_key("RightLowerArm"));
		assert!(neutral_calibration_target_rotation("RightLowerArm", NeutralCalibrationPose::U).is_some());
	}

	#[test]
	fn neutral_calibration_u_pose_upper_arm_offset_preserves_elbow_bend_plane() {
		let actual = quat_from_basis(
			[1.0, 0.0, 0.0],
			[0.0, -1.0, 0.0],
			normalize_vec3([0.30, 0.95, 0.0]).unwrap(),
			[0.0, 0.0, 1.0],
		)
		.unwrap();
		let mut samples = BTreeMap::new();
		samples.insert("RightUpperArm".to_string(), vec![actual]);

		let summary = summarize_neutral_rotations(&samples, NeutralCalibrationPose::U);
		let upper_t = [0.0, 0.0, 0.0, 1.0];
		let lower_bent_up = quat_from_to([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
		let corrected_upper = normalize_quat_array(quat_mul_array(
			upper_t,
			quat_inverse_array(summary.get("RightUpperArm").copied().unwrap_or([0.0, 0.0, 0.0, 1.0])),
		));
		let forearm_axis = quat_rotate_vec3(
			normalize_quat_array(quat_mul_array(corrected_upper, lower_bent_up)),
			[1.0, 0.0, 0.0],
		);

		assert_vec3_near(forearm_axis, [0.0, 1.0, 0.0], 1e-5);
	}

	#[test]
	fn neutral_calibration_u_pose_targets_bunny_ear_arm_shape() {
		let right_upper = neutral_calibration_target_rotation("RightUpperArm", NeutralCalibrationPose::U).unwrap();
		let right_lower = neutral_calibration_target_rotation("RightLowerArm", NeutralCalibrationPose::U).unwrap();
		let right_hand = neutral_calibration_target_rotation("RightHand", NeutralCalibrationPose::U).unwrap();
		let right_upper_axis = quat_rotate_vec3(right_upper, [1.0, 0.0, 0.0]);
		let right_lower_global = normalize_quat_array(quat_mul_array(right_upper, right_lower));
		let right_lower_axis = quat_rotate_vec3(right_lower_global, [1.0, 0.0, 0.0]);
		let right_lower_palm = quat_rotate_vec3(right_lower_global, [0.0, -1.0, 0.0]);
		let right_hand_global = normalize_quat_array(quat_mul_array(right_lower_global, right_hand));
		let right_hand_axis = quat_rotate_vec3(right_hand_global, [1.0, 0.0, 0.0]);
		let right_hand_palm = quat_rotate_vec3(right_hand_global, [0.0, -1.0, 0.0]);

		assert_vec3_near(right_upper_axis, normalize_vec3([0.82, 0.57, 0.0]).unwrap(), 1e-5);
		assert_vec3_near(right_lower_axis, normalize_vec3([-0.707, 0.707, 0.0]).unwrap(), 1e-5);
		assert_vec3_near(right_lower_palm, [0.0, 0.0, 1.0], 1e-5);
		assert_vec3_near(right_hand_axis, [0.0, 1.0, 0.0], 1e-5);
		assert_vec3_near(right_hand_palm, [0.0, 0.0, 1.0], 1e-5);
	}

	fn assert_quat_array_near(actual: [f32; 4], expected: [f32; 4], epsilon: f32) {
		let same = actual
			.iter()
			.zip(expected.iter())
			.all(|(actual, expected)| (actual - expected).abs() <= epsilon);
		let negated = actual
			.iter()
			.zip(expected.iter())
			.all(|(actual, expected)| (actual + expected).abs() <= epsilon);
		assert!(same || negated, "actual={actual:?} expected={expected:?}");
	}

	fn quat_rotate_vec3(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
		let qv = [v[0], v[1], v[2], 0.0];
		let rotated = quat_mul_array(quat_mul_array(q, qv), quat_inverse_array(q));
		[rotated[0], rotated[1], rotated[2]]
	}

	fn assert_vec3_near(actual: [f32; 3], expected: [f32; 3], epsilon: f32) {
		assert!(
			actual
				.iter()
				.zip(expected.iter())
				.all(|(actual, expected)| (actual - expected).abs() <= epsilon),
			"actual={actual:?} expected={expected:?}"
		);
	}
}
