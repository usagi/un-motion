//! profile ごとに保存する strongly-typed runtime / pipeline settings。
//!
//! これらの struct は `profiles/<id>.toml` 内では TOML、core HTTP API 越しでは
//! JSON として扱われる。どちらも同じ camelCase shape を共有するため、TOML を
//! 手書きした profile (`runtimeSelection.fps = 60`) と GUI client が生成した
//! profile は同じ意味を持つ。各 field は `Option<T>` とし、呼び出し側がすべての
//! 値を明示しなくても runtime built-in defaults へ fallback できる。
//!
//! 新しい tunable knob を追加するときは、この struct を拡張し、`runtime_host` など
//! 実際に値を消費する場所で読む。Svelte supervisor は path + value command で
//! settings を更新する想定で、schema 全体を事前に知る必要はない。

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
	/// 上半身の前後折れ抑制。`1.0` は pass-through、`0.0` は
	/// Spine/Chest/UpperChest の local X-axis torso pitch を取り除く。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub torso_pitch_scale: Option<f32>,
	/// 実演者から取得した Neutral pose calibration。有効時は runtime が安定した
	/// root/head 基準回転を outgoing motion から差し引く。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_calibration_enabled: Option<bool>,
	/// Bone/root key -> neutral quaternion `[x, y, z, w]`。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_calibration_rotations: Option<BTreeMap<String, [f32; 4]>>,
	/// `neutralCalibrationRotations` 取得時の pose kind。
	/// 既知の値は `"U"`、`"T"`、`"I"`。`"T"` は T-wrist-front を表す。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_calibration_pose: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub camera_diagonal_view_angle_deg: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub min_landmark_confidence: Option<f32>,
	/// ユーザー向けの瞼開き補正。`0.5` が中立で、低いほど瞼を重く、
	/// 高いほど通常開眼を広めに扱う。
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
	/// 開発プレビュー: 汎用 modifier stage の前で Head の仰俯角を補正するための、
	/// 演者固有 face landmark model。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub face_pose_model: Option<ProfileFacePoseModelSettings>,
}

/// Engine (現状 MediaPipe Native) 内部の advanced post-process 規則。
/// 一般ユーザーが触る想定ではない (UI 非露出)。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileMediaPipeAdvancedSettings {
	/// MediaPipe 由来 humanoid motion に解剖学的な妥当性制約を適用する。
	/// ROM clamp、人体として不自然な jump 抑制、復帰 / source switch damping の
	/// 上位スイッチ。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub anatomical_constraints: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub hold_lost_landmarks: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ease_recovery: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub limit_rotation_jumps: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head_source_switch_blend: Option<bool>,
	/// source landmark が消えた部位の signal loss policy。
	/// 既知の値は `rest-pose`、`hold`、`drop`。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_behavior: Option<String>,
	/// `lostSignalBehavior` が `rest-pose` のときの T-wrist-front pose (0.0) と
	/// I-pose (1.0) の blend。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lost_signal_rest_pose_blend: Option<f32>,
	/// `lostSignalBehavior` が `hold` のときの保持秒数。
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
	/// 正面 neutral 時の `(nose.y - eye_mid.y) / (mouth_mid.y - eye_mid.y)`。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub neutral_nose_drop_eye_mouth: Option<f32>,
	/// model 生成時に使った live valid sample 数。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sample_count: Option<u32>,
	/// sampling 中に観測した yaw 絶対値の中央値。粗い品質フラグ用。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub median_abs_yaw: Option<f32>,
	/// sampling 中に観測した roll 絶対値の中央値。粗い品質フラグ用。
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
	/// MediaPipe input preprocessing。既知の値は `"off"`、`"temporal-iir"`。
	/// native MediaPipe inference 直前の RGB image に適用する。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_denoise_mode: Option<String>,
	/// `inputDenoiseMode = "temporal-iir"` の cutoff frequency。
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_denoise_temporal_iir_hz: Option<f32>,
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
		components.input_denoise_mode = Some("temporal-iir".to_string());
		components.input_denoise_temporal_iir_hz = Some(8.0);
		components.input_resize_enabled = Some(true);
		components.input_resize_axis = Some("width".to_string());

		let text = toml::to_string_pretty(&components).expect("serialize");
		assert!(text.contains("engine = \"mediapipe-native\""));
		assert!(text.contains("inputDenoiseMode = \"temporal-iir\""));
		assert!(text.contains("inputDenoiseTemporalIirHz = 8.0"));
		assert!(text.contains("inputResizeEnabled = true"));

		let parsed: ProfilePipelineComponents = toml::from_str(&text).expect("parse");
		assert_eq!(parsed, components);
	}
}
