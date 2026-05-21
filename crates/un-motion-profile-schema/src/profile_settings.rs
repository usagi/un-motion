//! Strongly-typed runtime / pipeline settings stored per profile.
//!
//! These structs are persisted as TOML inside `profiles/<id>.toml` and exchanged
//! over the core HTTP API as JSON. Both paths share the same camelCase shape so
//! a profile authored by hand in TOML (`runtimeSelection.fps = 60`) and one
//! produced by a GUI client carry identical semantics. Each field is an
//! `Option<T>` so callers can leave the runtime to fall back to its built-in
//! defaults rather than having to specify every value up-front.
//!
//! Adding a new tunable knob means extending these structs and reading the new
//! field in `runtime_host` (or wherever the value is consumed). The Svelte
//! supervisor is expected to update settings via a path + value command and
//! does not need to know the full schema in advance.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRuntimeSettings {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fps: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vmc_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vmc_target_addr: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zenoh_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zenoh_key_expr: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zenoh_topic_mode: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zenoh_stream_id: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zenoh_producer: Option<String>,
	/// Engine Type 識別子。GUI の "Engine" dropdown で選択される一級項目。
	/// 認識される値: `"mediapipe-native"` / `"vmc"` / `"ifacialmocap"`。
	/// 未指定時は default (`mediapipe-native`) として扱う。
	///
	/// `runtime_selection.engine` が source of truth。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub engine: Option<String>,
	/// **MediaPipe Webcam 専用**: DirectShow / nokhwa デバイス識別子
	/// (例: `"dshow0:Cam Link 4K"`)。Engine Type が `mediapipe-*` 以外のときは
	/// 参照されない。VMC / iFacialMocap engine の listen address は
	/// `vmc_receive_listen_addr` / `ifacialmocap_receive_listen_addr` に分離した。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub device: Option<String>,
	/// **MediaPipe Webcam 専用**: キャプチャ解像度 (例: `"1280x720"`)。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resolution: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_pipe_running_mode: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_pipe_holistic_enabled: Option<bool>,
	/// Native MediaPipe の推論 delegate。認識される値:
	/// `"xnnpack"` / `"cpu"`。未指定時は runtime 既定値を使う。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_pipe_delegate: Option<String>,
	/// Native MediaPipe delegate の thread 数。未指定時は runtime 既定値。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_pipe_num_threads: Option<u32>,
	/// Native MediaPipe Holistic graph の FlowLimiter を有効化する。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_pipe_holistic_flow_limiter_enabled: Option<bool>,
	/// Native MediaPipe Holistic graph の FlowLimiter max_in_flight。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_pipe_holistic_flow_limiter_max_in_flight: Option<u32>,
	/// Native MediaPipe Holistic graph の FlowLimiter max_in_queue。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_pipe_holistic_flow_limiter_max_in_queue: Option<u32>,
	/// **VMC 受信 Engine 専用** (Engine Type = `"vmc"` のとき): VMC OSC listen
	/// アドレス (例: `"0.0.0.0:39539"`)。未指定なら既定値 `0.0.0.0:39539` (VMC
	/// 標準ポート: VSeeFace / Waidayo / mocopi 送信器の既定 port) を使う。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vmc_receive_listen_addr: Option<String>,
	/// **iFacialMocap 受信 Engine 専用** (Engine Type = `"ifacialmocap"` のとき):
	/// iFacialMocap OSC listen アドレス (例: `"0.0.0.0:49983"`)。未指定なら
	/// 既定値 `0.0.0.0:49983` を使う。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ifacialmocap_receive_listen_addr: Option<String>,
	/// Engine 非依存の出力段 Modifier 設定。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub modifier: Option<ProfileModifierSettings>,
}

/// Capturer の出力段で適用される Modifier 設定 (bone subset + smoothing + mirror +
/// calibration)。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileModifierSettings {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub face_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub hands_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub arms_ik_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub torso_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub legs_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub feet_enabled: Option<bool>,
	/// Neutral pose calibration captured from a live performer. When enabled,
	/// runtime subtracts stable root/head baseline rotations from outgoing motion.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_calibration_enabled: Option<bool>,
	/// Bone/root key -> neutral quaternion `[x, y, z, w]`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_calibration_rotations: Option<BTreeMap<String, [f32; 4]>>,
	/// Pose kind used when `neutralCalibrationRotations` was captured.
	/// Known values: `"U"`, `"T"`, `"I"`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_calibration_pose: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub camera_diagonal_view_angle_deg: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub min_landmark_confidence: Option<f32>,
	/// User-facing eyelid openness adjustment. `0.5` is neutral, lower values
	/// make eyelids heavier, higher values keep normally open eyes wider.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub eye_open_bias: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub mirror_mode: Option<String>,
	/// Smoothing stage preset (`"off"` / `"low"` / `"medium"` / `"high"` /
	/// `"adaptive"`). 大文字小文字は ASCII で比較される (case-insensitive)。
	/// 未指定は `"off"` 扱い = pass-through。
	///
	/// runtime 側 enum (`un_motion_runtime::SmoothingPreset`):
	/// - `Off`      — Stage を pipeline から除外。
	/// - `Low`      — fixed-α = 0.70 (反応優先)。
	/// - `Medium`   — fixed-α = 0.45。
	/// - `High`     — fixed-α = 0.25 (jitter 強抑制)。
	/// - `Adaptive` — One-Euro Filter (`adaptiveMinCutoffHz=0.35Hz, adaptiveBeta=0.08`)。
	///
	/// TOML 側のキーは serde `rename_all = "camelCase"` により `smoothingPreset`。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub smoothing_preset: Option<String>,
	/// EMA smoothing を明示的に有効化する。指定時は `smoothingPreset` より優先され、
	/// `smoothingOneEuroEnabled` と併用できる。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub smoothing_ema_enabled: Option<bool>,
	/// EMA の現在値重み α。1.0 は pass-through、低いほど強い平滑化。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub smoothing_ema_alpha: Option<f32>,
	/// One-Euro Filter smoothing を明示的に有効化する。EMA と併用できる。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub smoothing_one_euro_enabled: Option<bool>,
	/// One-Euro Filter の min cutoff をサンプル confidence に応じて下げる。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub smoothing_confidence_adaptive_cutoff: Option<bool>,
	/// One-Euro Filter の静止時カットオフ周波数。低いほど静止 jitter を強く抑える。
	/// `smoothingPreset = "adaptive"` のときのみ runtime に反映される。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub adaptive_min_cutoff_hz: Option<f32>,
	/// One-Euro Filter の速度依存ゲイン。高いほど速い動きで追従性が上がる。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub adaptive_beta: Option<f32>,
	/// One-Euro Filter の速度推定自体を平滑化するカットオフ周波数。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub adaptive_derivative_cutoff_hz: Option<f32>,
	/// Engine (現状は MediaPipe Native) 固有の advanced チューニング。
	/// 一般ユーザー向け GUI には露出させず、profile TOML を直接編集する上級者
	/// 向けに残してある。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub post_process_rules: Option<ProfileMediaPipeAdvancedSettings>,
	/// Developer preview: performer-specific face landmark model used to correct
	/// head pose pitch before the generic modifier stages.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub face_pose_model: Option<ProfileFacePoseModelSettings>,
}

/// Engine (現状 MediaPipe Native) 内部の advanced post-process 規則。
/// 一般ユーザーが触る想定ではない (UI 非露出)。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileMediaPipeAdvancedSettings {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub hold_lost_landmarks: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ease_recovery: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub limit_rotation_jumps: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head_source_switch_blend: Option<bool>,
	/// Signal loss policy for body parts whose source landmarks disappear.
	/// Known values: `rest-pose`, `hold`, `drop`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_behavior: Option<String>,
	/// Blend between T-pose (0.0) and I-pose (1.0) when `lostSignalBehavior`
	/// is `rest-pose`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_rest_pose_blend: Option<f32>,
	/// Hold timeout in seconds when `lostSignalBehavior` is `hold`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_hold_seconds: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_head_behavior: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_head_rest_pose_blend: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_head_hold_seconds: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_hands_behavior: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_hands_rest_pose_blend: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_hands_hold_seconds: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_arms_behavior: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_arms_rest_pose_blend: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_arms_hold_seconds: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_recovery_seconds: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head_from_pose: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head_from_face_matrix: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head_reconcile: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_eye_fallback: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub hand_camera_target: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub hand_orientation: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub finger_derived: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub arm_from_pose: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub arm_ik_from_hands: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub crossed_hand_heuristic: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub coordinate_correction: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub final_clamp: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileFacePoseModelSettings {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub enabled: Option<bool>,
	/// Neutral frontal value of `(nose.y - eye_mid.y) / (mouth_mid.y - eye_mid.y)`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_nose_drop_eye_mouth: Option<f32>,
	/// Number of live valid samples used when this model was generated.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sample_count: Option<u32>,
	/// Median absolute yaw observed while sampling. Useful as a rough quality flag.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub median_abs_yaw: Option<f32>,
	/// Median absolute roll observed while sampling. Useful as a rough quality flag.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub median_abs_roll: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePipelineComponents {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub engine: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub post_process: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_path: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_fps: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_width: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_height: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_pixel_format: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_repeat: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_ffmpeg_path: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_resize_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_resize_axis: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_resize_reference: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_resize_width: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_resize_height: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_resize_preserve_aspect: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_resize_pad_color: Option<String>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn runtime_settings_roundtrip_through_toml() {
		let mut settings = ProfileRuntimeSettings::default();
		settings.fps = Some(90);
		settings.zenoh_enabled = Some(true);
		settings.zenoh_key_expr = Some("un-motion/frame".to_string());
		settings.media_pipe_delegate = Some("xnnpack".to_string());
		settings.media_pipe_num_threads = Some(2);
		settings.media_pipe_holistic_flow_limiter_enabled = Some(true);
		settings.media_pipe_holistic_flow_limiter_max_in_flight = Some(2);
		settings.media_pipe_holistic_flow_limiter_max_in_queue = Some(1);
		settings.modifier = Some(ProfileModifierSettings {
			hands_enabled: Some(true),
			eye_open_bias: Some(0.75),
			post_process_rules: Some(ProfileMediaPipeAdvancedSettings {
				finger_derived: Some(false),
				..Default::default()
			}),
			..Default::default()
		});

		let text = toml::to_string_pretty(&settings).expect("serialize");
		assert!(text.contains("fps = 90"));
		assert!(text.contains("zenohEnabled = true"));
		assert!(text.contains("mediaPipeDelegate = \"xnnpack\""));
		assert!(text.contains("mediaPipeNumThreads = 2"));
		assert!(text.contains("mediaPipeHolisticFlowLimiterMaxInFlight = 2"));
		assert!(text.contains("[modifier]"));
		assert!(text.contains("eyeOpenBias = 0.75"));
		assert!(text.contains("[modifier.postProcessRules]"));

		let parsed: ProfileRuntimeSettings = toml::from_str(&text).expect("parse toml");
		assert_eq!(parsed.fps, Some(90));
		assert_eq!(parsed.zenoh_enabled, Some(true));
		assert_eq!(parsed.media_pipe_delegate.as_deref(), Some("xnnpack"));
		assert_eq!(parsed.media_pipe_num_threads, Some(2));
		assert_eq!(parsed.media_pipe_holistic_flow_limiter_enabled, Some(true));
		assert_eq!(parsed.media_pipe_holistic_flow_limiter_max_in_flight, Some(2));
		assert_eq!(parsed.media_pipe_holistic_flow_limiter_max_in_queue, Some(1));
		assert_eq!(parsed.modifier.as_ref().and_then(|modifier| modifier.hands_enabled), Some(true));
		assert_eq!(parsed.modifier.as_ref().and_then(|modifier| modifier.eye_open_bias), Some(0.75));
		assert_eq!(
			parsed
				.modifier
				.as_ref()
				.and_then(|modifier| modifier.post_process_rules.as_ref())
				.and_then(|rules| rules.finger_derived),
			Some(false)
		);
	}

	#[test]
	fn runtime_settings_roundtrip_through_json() {
		let settings = ProfileRuntimeSettings {
			fps: Some(60),
			vmc_enabled: Some(true),
			vmc_target_addr: Some("127.0.0.1:39539".to_string()),
			..Default::default()
		};
		let json = serde_json::to_string(&settings).expect("serialize json");
		assert!(json.contains("\"fps\":60"));
		assert!(json.contains("\"vmcEnabled\":true"));
		let parsed: ProfileRuntimeSettings = serde_json::from_str(&json).expect("parse json");
		assert_eq!(parsed, settings);
	}

	#[test]
	fn vmc_receive_listen_addr_roundtrips() {
		let settings = ProfileRuntimeSettings {
			engine: Some("vmc".to_string()),
			vmc_receive_listen_addr: Some("0.0.0.0:39539".to_string()),
			..Default::default()
		};
		let text = toml::to_string_pretty(&settings).expect("serialize");
		assert!(text.contains("vmcReceiveListenAddr = \"0.0.0.0:39539\""));
		let parsed: ProfileRuntimeSettings = toml::from_str(&text).expect("parse toml");
		assert_eq!(parsed.vmc_receive_listen_addr.as_deref(), Some("0.0.0.0:39539"));
		assert_eq!(parsed.engine.as_deref(), Some("vmc"));
		// `device` (MediaPipe webcam 専用フィールド) は VMC engine では未使用なので None。
		assert!(parsed.device.is_none());
	}

	#[test]
	fn pipeline_components_roundtrip_through_toml() {
		let mut components = ProfilePipelineComponents::default();
		components.engine = Some("mediapipe-native".to_string());
		components.input = Some("webcam-directshow".to_string());
		components.input_fps = Some(30);
		components.input_resize_enabled = Some(true);
		components.input_resize_axis = Some("width".to_string());

		let text = toml::to_string_pretty(&components).expect("serialize");
		assert!(text.contains("engine = \"mediapipe-native\""));
		assert!(text.contains("inputResizeEnabled = true"));

		let parsed: ProfilePipelineComponents = toml::from_str(&text).expect("parse");
		assert_eq!(parsed, components);
	}
}
