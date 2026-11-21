import { mockIPC } from "@tauri-apps/api/mocks";
import enLocaleToml from "../src-tauri/locales/en-US.toml?raw";
import jaLocaleToml from "../src-tauri/locales/ja-JP.toml?raw";
import newProfileDefaults from "./lib/newProfileDefaults.json";

type LocaleBundle = {
	locale: string;
	messages: Record<string, string>;
};

type ProfileDetail = {
	id: string;
	revision?: string;
	deletionRevision?: string;
	name: string;
	note: string;
	path: string;
	iconPath: string | null;
	group: string;
	runtime: Record<string, unknown>;
	pipeline: Record<string, unknown>;
	// Raw preview-only values retained across clone/archive; derived display values are separate.
	previewSavedFields?: Record<string, unknown>;
	previewFacePoseModel?: Record<string, unknown>;
};

const localeBundles: Record<string, LocaleBundle> = {
	"ja-JP": { locale: "ja-JP", messages: parseFlatTomlMessages(jaLocaleToml) },
	"en-US": { locale: "en-US", messages: parseFlatTomlMessages(enLocaleToml) },
};

let profiles: ProfileDetail[] = [
	{
		id: "mediapipe-native",
		name: "MediaPipe Native",
		note: "Webcam to UNMF/Z and VMC/UDP",
		path: "profiles/mediapipe-native.toml",
		iconPath: null,
		group: "Main",
		runtime: {
			fps: 60,
			vmcEnabled: true,
			vmcTargetAddr: "127.0.0.1:39539",
			vrcOscEnabled: false,
			vrcOscTargetAddr: "127.0.0.1:9000",
			vrcOscSendOnlyWhenVrchatRunning: true,
			vrcOscProcessPollIntervalSecs: 10,
			vrcOscParameterPrefix: null,
			zenohEnabled: true,
			zenohKeyExpr: "un-motion/frame",
			zenohTopicMode: "frame",
			zenohStreamId: null,
			zenohProducer: "un-motion-capturer",
			zenohLanEnabled: false,
			zenohLanPort: 47447,
			zenohLanConnectAddress: null,
			engine: "mediapipe-native",
			device: "cam0:Preview Camera",
			resolution: "1280x720",
			mediaPipeRunningMode: "live_stream",
			mediaPipeHolisticEnabled: true,
			vmcReceiveListenAddr: "0.0.0.0:39539",
			ifacialmocapReceiveListenAddr: "0.0.0.0:49983",
			modifierHeadEnabled: true,
			modifierFaceEnabled: true,
			modifierHandsEnabled: true,
			modifierArmsIkEnabled: true,
			modifierTorsoEnabled: true,
			modifierLegsEnabled: false,
			modifierFeetEnabled: false,
			modifierMirrorMode: "normal",
			modifierSmoothingPreset: "medium",
			modifierSmoothingEmaEnabled: true,
			modifierSmoothingEmaAlpha: 0.45,
			modifierSmoothingOneEuroEnabled: false,
			modifierSmoothingConfidenceAdaptiveCutoff: false,
			modifierAdaptiveMinCutoffHz: 0.35,
			modifierAdaptiveBeta: 0.08,
			modifierAdaptiveDerivativeCutoffHz: 1.0,
			modifierHoldLostLandmarks: true,
			modifierEaseRecovery: true,
			modifierLimitRotationJumps: true,
			modifierHeadSourceSwitchBlend: true,
			modifierLostSignalBehavior: "rest-pose",
			modifierLostSignalRestPoseBlend: 0.3,
			modifierLostSignalHoldSeconds: 8.2,
			modifierLostSignalHeadBehavior: "hold",
			modifierLostSignalHeadRestPoseBlend: 0.3,
			modifierLostSignalHeadHoldSeconds: 8.2,
			modifierLostSignalHandsBehavior: "rest-pose",
			modifierLostSignalHandsRestPoseBlend: 0.3,
			modifierLostSignalHandsHoldSeconds: 8.2,
			modifierLostSignalArmsBehavior: "rest-pose",
			modifierLostSignalArmsRestPoseBlend: 0.3,
			modifierLostSignalArmsHoldSeconds: 8.2,
			modifierLostSignalRecoverySeconds: 0.25,
		},
		pipeline: {
			input: "webcam-mediafoundation",
			postProcess: "mediapipe-holistic-v1",
			inputPath: null,
			inputFps: 60,
			inputWidth: null,
			inputHeight: null,
			inputDenoiseMode: "off",
			inputRepeat: false,
			inputFfmpegPath: null,
			inputResizeEnabled: false,
			inputResizeAxis: null,
			inputResizeReference: null,
			inputResizeWidth: null,
			inputResizeHeight: null,
			inputResizePreserveAspect: true,
			inputResizePadColor: null,
		},
	},
	{
		id: "vmc-receive",
		name: "VMC Receive",
		note: "OSC/UDP receive, publish as UNMotionFrame",
		path: "profiles/vmc-receive.toml",
		iconPath: null,
		group: "Inputs",
		runtime: {
			engine: "vmc",
			fps: 60,
			vmcEnabled: false,
			vmcTargetAddr: "127.0.0.1:39539",
			vrcOscEnabled: false,
			vrcOscTargetAddr: "127.0.0.1:9000",
			vrcOscSendOnlyWhenVrchatRunning: true,
			vrcOscProcessPollIntervalSecs: 10,
			vrcOscParameterPrefix: null,
			zenohEnabled: true,
			zenohKeyExpr: "un-motion/frame",
			zenohTopicMode: "by-stream-id",
			zenohStreamId: "vmc-receive",
			zenohProducer: "un-motion-capturer",
			zenohLanEnabled: true,
			zenohLanPort: 47447,
			zenohLanConnectAddress: "192.168.1.20:47447",
			vmcReceiveListenAddr: "0.0.0.0:39539",
			ifacialmocapReceiveListenAddr: "0.0.0.0:49983",
			modifierHeadEnabled: true,
			modifierFaceEnabled: true,
			modifierHandsEnabled: true,
			modifierArmsIkEnabled: true,
			modifierTorsoEnabled: true,
			modifierLegsEnabled: true,
			modifierFeetEnabled: true,
			modifierMirrorMode: "normal",
			modifierSmoothingPreset: "off",
			modifierSmoothingEmaEnabled: false,
			modifierSmoothingEmaAlpha: 0.45,
			modifierSmoothingOneEuroEnabled: false,
			modifierSmoothingConfidenceAdaptiveCutoff: false,
			modifierAdaptiveMinCutoffHz: 0.35,
			modifierAdaptiveBeta: 0.08,
			modifierAdaptiveDerivativeCutoffHz: 1.0,
			modifierHoldLostLandmarks: true,
			modifierEaseRecovery: true,
			modifierLimitRotationJumps: true,
			modifierHeadSourceSwitchBlend: true,
			modifierLostSignalBehavior: "hold",
			modifierLostSignalRestPoseBlend: 0.3,
			modifierLostSignalHoldSeconds: 8.2,
			modifierLostSignalHeadBehavior: "hold",
			modifierLostSignalHeadRestPoseBlend: 0.3,
			modifierLostSignalHeadHoldSeconds: 8.2,
			modifierLostSignalHandsBehavior: "hold",
			modifierLostSignalHandsRestPoseBlend: 0.3,
			modifierLostSignalHandsHoldSeconds: 8.2,
			modifierLostSignalArmsBehavior: "hold",
			modifierLostSignalArmsRestPoseBlend: 0.3,
			modifierLostSignalArmsHoldSeconds: 8.2,
			modifierLostSignalRecoverySeconds: 0.25,
		},
		pipeline: {
			engine: "vmc",
			input: "vmc",
			postProcess: null,
			inputPath: null,
			inputFps: null,
			inputWidth: null,
			inputHeight: null,
			inputDenoiseMode: "off",
			inputRepeat: false,
		},
	},
];

let activeProfileId = profiles[0].id;
const archivedProfiles = new Map<string, ProfileDetail>();
let nextArchiveId = 1;

function duplicatePreviewProfile(profileId: string): ProfileDetail {
	const original = findProfile(profileId);
	let index = 1;
	let id: string;
	let name: string;
	do {
		id = `${original.id}-copy-${index}`;
		name = `${original.name} copy${index === 1 ? "" : ` ${index}`}`;
		index += 1;
	} while (profiles.some((entry) => entry.id === id || entry.name === name || entry.path === `profiles/${id}.toml`));
	const copy = { ...structuredClone(original), id, name, path: `profiles/${id}.toml` };
	profiles.push(copy);
	return structuredClone(copy);
}

function createPreviewProfile(name: unknown): ProfileDetail {
	if (typeof name !== "string") throw new Error("Expected profile name");
	const label = name.trim() || "New Profile";
	const base = label.replace(/[ _-]/g, "-").replace(/[^a-zA-Z0-9-]/g, "").toLowerCase().replace(/^-+|-+$/g, "") || "profile";
	let id = base, index = 2;
	while (profiles.some((profile) => profile.id === id)) id = `${base}-${index++}`;
	const profile: ProfileDetail = { id, name: label, note: "", path: `profiles/${id}.toml`, iconPath: null, group: "", ...structuredClone(newProfileDefaults) };
	profiles.push(profile);
	selectedProfileId = id;
	return structuredClone(profile);
}

// Direct ProfilePipelineView fields; denoise uses separate native normalization.
const previewPipelineFields: Record<string, { key: string; kind: string }> = {
	"pipeline_components.input": { key: "input", kind: "opt_string" },
	"pipeline_components.post_process": { key: "postProcess", kind: "opt_string" },
	"pipeline_components.input_path": { key: "inputPath", kind: "opt_string" },
	"pipeline_components.input_fps": { key: "inputFps", kind: "opt_u32" },
	"pipeline_components.input_width": { key: "inputWidth", kind: "opt_u32" },
	"pipeline_components.input_height": { key: "inputHeight", kind: "opt_u32" },
	"pipeline_components.input_pixel_format": { key: "inputPixelFormat", kind: "opt_string" },
	"pipeline_components.input_repeat": { key: "inputRepeat", kind: "opt_bool" },
	"pipeline_components.input_ffmpeg_path": { key: "inputFfmpegPath", kind: "opt_string" },
	"pipeline_components.input_resize_enabled": { key: "inputResizeEnabled", kind: "opt_bool" },
	"pipeline_components.input_resize_axis": { key: "inputResizeAxis", kind: "opt_string" },
	"pipeline_components.input_resize_reference": { key: "inputResizeReference", kind: "opt_u32" },
	"pipeline_components.input_resize_width": { key: "inputResizeWidth", kind: "opt_u32" },
	"pipeline_components.input_resize_height": { key: "inputResizeHeight", kind: "opt_u32" },
	"pipeline_components.input_resize_preserve_aspect": { key: "inputResizePreserveAspect", kind: "opt_bool" },
	"pipeline_components.input_resize_pad_color": { key: "inputResizePadColor", kind: "opt_string" },
};

const previewRuntimeFields: Record<string, { key: string; kind: string; minimum?: number; maximum?: number }> = {
	"runtime_selection.fps": { key: "fps", kind: "opt_u32" },
	"runtime_selection.vmc_enabled": { key: "vmcEnabled", kind: "opt_bool" },
	"runtime_selection.vmc_target_addr": { key: "vmcTargetAddr", kind: "opt_string" },
	"runtime_selection.vrc_osc_enabled": { key: "vrcOscEnabled", kind: "opt_bool" },
	"runtime_selection.vrc_osc_target_addr": { key: "vrcOscTargetAddr", kind: "opt_string" },
	"runtime_selection.vrc_osc_send_only_when_vrchat_running": { key: "vrcOscSendOnlyWhenVrchatRunning", kind: "opt_bool" },
	"runtime_selection.vrc_osc_process_poll_interval_secs": { key: "vrcOscProcessPollIntervalSecs", kind: "opt_u64" },
	"runtime_selection.vrc_osc_parameter_prefix": { key: "vrcOscParameterPrefix", kind: "opt_string" },
	"runtime_selection.zenoh_enabled": { key: "zenohEnabled", kind: "opt_bool" },
	"runtime_selection.zenoh_key_expr": { key: "zenohKeyExpr", kind: "opt_string" },
	"runtime_selection.zenoh_topic_mode": { key: "zenohTopicMode", kind: "opt_string" },
	"runtime_selection.zenoh_stream_id": { key: "zenohStreamId", kind: "opt_string" },
	"runtime_selection.zenoh_producer": { key: "zenohProducer", kind: "opt_string" },
	"runtime_selection.device": { key: "device", kind: "opt_string" },
	"runtime_selection.resolution": { key: "resolution", kind: "opt_string" },
	"runtime_selection.media_pipe_running_mode": { key: "mediaPipeRunningMode", kind: "opt_string" },
	"runtime_selection.media_pipe_holistic_enabled": { key: "mediaPipeHolisticEnabled", kind: "opt_bool" },
	"runtime_selection.media_pipe_delegate": { key: "mediaPipeDelegate", kind: "opt_string" },
	"runtime_selection.media_pipe_num_threads": { key: "mediaPipeNumThreads", kind: "opt_u32", minimum: 1 },
	"runtime_selection.media_pipe_holistic_flow_limiter_enabled": { key: "mediaPipeHolisticFlowLimiterEnabled", kind: "opt_bool" },
	"runtime_selection.media_pipe_holistic_flow_limiter_max_in_flight": { key: "mediaPipeHolisticFlowLimiterMaxInFlight", kind: "opt_u32", minimum: 1 },
	"runtime_selection.media_pipe_holistic_flow_limiter_max_in_queue": { key: "mediaPipeHolisticFlowLimiterMaxInQueue", kind: "opt_u32" },
	"runtime_selection.vmc_receive_listen_addr": { key: "vmcReceiveListenAddr", kind: "opt_string" },
	"runtime_selection.ifacialmocap_receive_listen_addr": { key: "ifacialmocapReceiveListenAddr", kind: "opt_string" },
	"runtime_selection.modifier.head_enabled": { key: "modifierHeadEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.face_enabled": { key: "modifierFaceEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.hands_enabled": { key: "modifierHandsEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.arms_ik_enabled": { key: "modifierArmsIkEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.torso_enabled": { key: "modifierTorsoEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.legs_enabled": { key: "modifierLegsEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.feet_enabled": { key: "modifierFeetEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.torso_pitch_scale": { key: "modifierTorsoPitchScale", kind: "opt_f32", maximum: 1 },
	"runtime_selection.modifier.mirror_mode": { key: "modifierMirrorMode", kind: "opt_string" },
	"runtime_selection.modifier.eye_open_bias": { key: "modifierEyeOpenBias", kind: "opt_f32", maximum: 1 },
	"runtime_selection.modifier.smoothing_preset": { key: "modifierSmoothingPreset", kind: "opt_string" },
	"runtime_selection.modifier.smoothing_ema_enabled": { key: "modifierSmoothingEmaEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.smoothing_ema_alpha": { key: "modifierSmoothingEmaAlpha", kind: "opt_f32" },
	"runtime_selection.modifier.smoothing_one_euro_enabled": { key: "modifierSmoothingOneEuroEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.smoothing_confidence_adaptive_cutoff": { key: "modifierSmoothingConfidenceAdaptiveCutoff", kind: "opt_bool" },
	"runtime_selection.modifier.adaptive_min_cutoff_hz": { key: "modifierAdaptiveMinCutoffHz", kind: "opt_f32" },
	"runtime_selection.modifier.adaptive_beta": { key: "modifierAdaptiveBeta", kind: "opt_f32" },
	"runtime_selection.modifier.adaptive_derivative_cutoff_hz": { key: "modifierAdaptiveDerivativeCutoffHz", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.anatomical_constraints": { key: "modifierAnatomicalConstraints", kind: "opt_bool" },
	"runtime_selection.modifier.post_process_rules.hold_lost_landmarks": { key: "modifierHoldLostLandmarks", kind: "opt_bool" },
	"runtime_selection.modifier.post_process_rules.ease_recovery": { key: "modifierEaseRecovery", kind: "opt_bool" },
	"runtime_selection.modifier.post_process_rules.limit_rotation_jumps": { key: "modifierLimitRotationJumps", kind: "opt_bool" },
	"runtime_selection.modifier.post_process_rules.head_source_switch_blend": { key: "modifierHeadSourceSwitchBlend", kind: "opt_bool" },
	"runtime_selection.modifier.post_process_rules.head_from_face_matrix": { key: "modifierHeadFromFaceMatrix", kind: "opt_bool" },
	"runtime_selection.modifier.post_process_rules.lost_signal_behavior": { key: "modifierLostSignalBehavior", kind: "opt_string" },
	"runtime_selection.modifier.post_process_rules.lost_signal_rest_pose_blend": { key: "modifierLostSignalRestPoseBlend", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_hold_seconds": { key: "modifierLostSignalHoldSeconds", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_head_behavior": { key: "modifierLostSignalHeadBehavior", kind: "opt_string" },
	"runtime_selection.modifier.post_process_rules.lost_signal_head_rest_pose_blend": { key: "modifierLostSignalHeadRestPoseBlend", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_head_hold_seconds": { key: "modifierLostSignalHeadHoldSeconds", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_hands_behavior": { key: "modifierLostSignalHandsBehavior", kind: "opt_string" },
	"runtime_selection.modifier.post_process_rules.lost_signal_hands_rest_pose_blend": { key: "modifierLostSignalHandsRestPoseBlend", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_hands_hold_seconds": { key: "modifierLostSignalHandsHoldSeconds", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_arms_behavior": { key: "modifierLostSignalArmsBehavior", kind: "opt_string" },
	"runtime_selection.modifier.post_process_rules.lost_signal_arms_rest_pose_blend": { key: "modifierLostSignalArmsRestPoseBlend", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_arms_hold_seconds": { key: "modifierLostSignalArmsHoldSeconds", kind: "opt_f32" },
	"runtime_selection.modifier.post_process_rules.lost_signal_recovery_seconds": { key: "modifierLostSignalRecoverySeconds", kind: "opt_f32" },
};

const previewFaceFields: Record<string, { key: string | null; kind: string }> = {
	"runtime_selection.modifier.face_pose_model.enabled": { key: "modifierFacePoseModelEnabled", kind: "opt_bool" },
	"runtime_selection.modifier.face_pose_model.neutral_nose_drop_eye_mouth": { key: "modifierFacePoseModelNeutralNoseDropEyeMouth", kind: "opt_f32" },
	"runtime_selection.modifier.face_pose_model.sample_count": { key: "modifierFacePoseModelSampleCount", kind: "opt_u32" },
	"runtime_selection.modifier.face_pose_model.median_abs_yaw": { key: null, kind: "opt_f32" },
	"runtime_selection.modifier.face_pose_model.median_abs_roll": { key: null, kind: "opt_f32" },
};

function previewRevisionDetail(profile: ProfileDetail): ProfileDetail {
	// Opaque content token for the isolated mock, not the native SHA256 algorithm.
	return { ...structuredClone(profile), revision: JSON.stringify(profile), deletionRevision: `delete:${JSON.stringify(profile)}` };
}

function updatePreviewProfile(profileId: string, updates: unknown, expectedRevision?: unknown): ProfileDetail {
	if (!Array.isArray(updates)) throw new Error("Expected profile updates");
	const current = findProfile(profileId);
	if (expectedRevision != null && expectedRevision !== JSON.stringify(current)) throw new Error("profile_conflict: saved profile changed");
	const next = structuredClone(current);
	const pipelineTouched = updates.some((update) => typeof update?.field === "string" && update.field.startsWith("pipeline_components."));
	if (pipelineTouched) {
		next.previewSavedFields ??= {
			"pipeline_components.input_denoise_mode": next.pipeline.inputDenoiseMode ?? null,
			"pipeline_components.input_denoise_temporal_iir_hz": next.pipeline.inputDenoiseTemporalIirHz ?? null,
		};
	}
	for (const update of updates) {
		if (!update || typeof update.field !== "string") throw new Error("Invalid profile update");
		const field: string = update.field;
		const value: unknown = update.value;
		if (field === "name" || field === "note" || field === "group") {
			if (typeof value !== "string") throw new Error("Expected text value");
			next[field] = field === "note" ? value : value.trim();
		} else if (Object.hasOwn(previewFaceFields, field)) {
			const spec = previewFaceFields[field];
			let normalized = value;
			if (value !== null) {
				if (spec.kind === "opt_bool") {
					if (typeof value !== "boolean") throw new Error("Expected boolean or null");
				} else if (spec.kind === "opt_u32") {
					if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > 4294967295) throw new Error("Expected u32 or null");
				} else {
					if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 3.4028234663852886e38) throw new Error("Expected nonnegative finite f32 or null");
					normalized = Math.fround(value);
				}
			}
			next.previewFacePoseModel ??= Object.fromEntries(Object.entries(previewFaceFields).map(([path, entry]) => [path, entry.key ? next.runtime[entry.key] ?? null : null]));
			if (field === "runtime_selection.modifier.face_pose_model.enabled" && value === false) {
				next.previewFacePoseModel = {};
			} else {
				next.previewFacePoseModel[field] = normalized;
			}
			for (const [path, entry] of Object.entries(previewFaceFields)) {
				if (entry.key) next.runtime[entry.key] = next.previewFacePoseModel[path] ?? null;
			}
		} else if (field === "icon_path") {
			if (value !== null && typeof value !== "string") throw new Error("Expected icon path or null");
			next.iconPath = typeof value === "string" ? value.trim() || null : null;
		} else if (field === "runtime_selection.engine") {
			if (value !== null && typeof value !== "string") throw new Error("Expected engine or null");
			const normalize = (v: unknown) => v === "media-pipe-native" || v === "mediapipe-native" ? "mediapipe-native" : v === "vmc" || v === "ifacialmocap" ? v : null;
			next.runtime.engine = normalize(typeof value === "string" ? value.trim() : null) ?? normalize(next.pipeline.engine) ?? "mediapipe-native";
		} else if (field === "pipeline_components.input_denoise_mode" || field === "pipeline_components.input_denoise_temporal_iir_hz") {
			next.previewSavedFields ??= {
				"pipeline_components.input_denoise_mode": next.pipeline.inputDenoiseMode ?? null,
				"pipeline_components.input_denoise_temporal_iir_hz": next.pipeline.inputDenoiseTemporalIirHz ?? null,
			};
			if (field === "pipeline_components.input_denoise_mode") {
				if (value !== null && typeof value !== "string") throw new Error("Expected denoise mode or null");
				next.previewSavedFields[field] = typeof value === "string" ? value.trim() || null : null;
			} else {
				if (value !== null && (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 3.4028234663852886e38)) throw new Error("Expected nonnegative finite f32 or null");
				next.previewSavedFields[field] = typeof value === "number" ? Math.fround(value) : null;
			}
			const mode = String(next.previewSavedFields["pipeline_components.input_denoise_mode"] ?? "off").trim().toLowerCase();
			const legacy = /^temporal[-_]iir[-_](10|8|6|4|2)hz$/.exec(mode);
			next.pipeline.inputDenoiseMode = ["on", "true", "temporal-iir", "temporal_iir"].includes(mode) || legacy ? "temporal-iir" : "off";
			next.pipeline.inputDenoiseTemporalIirHz = next.previewSavedFields["pipeline_components.input_denoise_temporal_iir_hz"] ?? (legacy ? Number(legacy[1]) : null);
		 } else if (Object.hasOwn(previewRuntimeFields, field)) {
            const spec = previewRuntimeFields[field];
            let normalized = value;
            if (value !== null) {
                if (spec.kind === "opt_string") {
                    if (typeof value !== "string") throw new Error("Expected string or null");
                    normalized = value.trim() || null;
                } else if (spec.kind === "opt_bool") {
                    if (typeof value !== "boolean") throw new Error("Expected boolean or null");
                } else if (spec.kind === "opt_u32" || spec.kind === "opt_u64") {
                    const maximum = spec.kind === "opt_u32" ? 4294967295 : Number.MAX_SAFE_INTEGER;
                    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 || value > maximum) throw new Error("Expected supported unsigned integer or null");
                    normalized = Math.max(spec.minimum ?? 0, value);
                } else if (spec.kind === "opt_f32") {
                    if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 3.4028234663852886e38) throw new Error("Expected nonnegative finite f32 or null");
                    normalized = Math.min(spec.maximum ?? Infinity, Math.fround(value));
                } else { throw new Error("Unsupported runtime parser"); }
            }
            next.runtime[spec.key] = normalized;
        } else if (Object.hasOwn(previewPipelineFields, field)) {
            const spec = previewPipelineFields[field];
            let normalized = value;
            if (value !== null) {
                if (spec.kind === "opt_string") {
                    if (typeof value !== "string") throw new Error("Expected string or null");
                    normalized = value.trim() || null;
                } else if (spec.kind === "opt_bool") {
                    if (typeof value !== "boolean") throw new Error("Expected boolean or null");
                } else if (spec.kind === "opt_u32") {
                    if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > 4294967295) throw new Error("Expected u32 or null");
                } else { throw new Error("Unsupported pipeline parser"); }
            }
            next.pipeline[spec.key] = normalized;
        } else if (field === "runtime_selection.zenoh_lan_enabled") {
			if (value !== null && typeof value !== "boolean") throw new Error("Expected boolean value");
			next.runtime.zenohLanEnabled = value;
		} else if (field === "runtime_selection.zenoh_lan_port") {
			if (value !== null && (typeof value !== "number" || !Number.isInteger(value) || value < 1 || value > 65535)) throw new Error("Expected port in 1..65535");
			next.runtime.zenohLanPort = value;
		} else {
			throw new Error(`dev IPC mock: unsupported field ${field}`);
		}
	}
	if (pipelineTouched) {
		const raw = next.previewSavedFields!;
		const rawMode = raw["pipeline_components.input_denoise_mode"];
		const rawHz = raw["pipeline_components.input_denoise_temporal_iir_hz"];
		const present = next.pipeline.engine != null || Object.values(previewPipelineFields).some((spec) => next.pipeline[spec.key] != null) || rawMode != null || rawHz != null;
		const mode = String(rawMode ?? "off").trim().toLowerCase();
		const legacy = /^temporal[-_]iir[-_](10|8|6|4|2)hz$/.exec(mode);
		next.pipeline.inputDenoiseMode = present ? (["on", "true", "temporal-iir", "temporal_iir"].includes(mode) || legacy ? "temporal-iir" : "off") : null;
		next.pipeline.inputDenoiseTemporalIirHz = present ? rawHz ?? (legacy ? Number(legacy[1]) : null) : null;
	}
	// This address is simulated, never a claim about native network connectivity.
	next.runtime.zenohLanConnectAddress = next.runtime.zenohLanEnabled ? `192.168.1.20:${Number(next.runtime.zenohLanPort ?? 47447)}` : null;
	profiles = profiles.map((profile) => profile.id === profileId ? next : profile);
	return previewRevisionDetail(next);
}

function archivePreviewProfile(profileId: string): void {
	const profile = findProfile(profileId);
	if (previewCapturers.some((entry) => entry.profileId === profileId && ["starting", "running", "stopping"].includes(entry.state))) {
		throw new Error(`profile is in use by a running capturer: ${profileId}`);
	}
	archivedProfiles.set(`preview-${nextArchiveId++}`, structuredClone(profile));
	profiles = profiles.filter((entry) => entry.id !== profileId);
	if (selectedProfileId === profileId) selectedProfileId = profiles[0]?.id ?? "";
}

function restorePreviewProfile(archiveId: string): { selectedProfileId: string } {
	const profile = archivedProfiles.get(archiveId);
	if (!profile) throw new Error(`archive not found: ${archiveId}`);
	if (profiles.some((entry) => entry.id === profile.id || entry.path === profile.path)) throw new Error(`profile already exists: ${profile.id}`);
	profiles.push(structuredClone(profile));
	if (!selectedProfileId) selectedProfileId = profile.id;
	return { selectedProfileId };
}
// Browser fixture only. Keep the editor selection independent of running captures.
// sessionStorage models a page reload, not native disk persistence or app restart.
const selectedProfileStorageKey = "un-motion-dev-preview-selected-profile";
let selectedProfileId = profiles[0].id;
try {
	const stored = sessionStorage.getItem(selectedProfileStorageKey);
	if (stored !== null && (stored === "" || profiles.some((profile) => profile.id === stored))) selectedProfileId = stored;
} catch { /* A browser without storage can still run the in-memory preview. */ }
// Browser-only simulation: never starts a native process or opens an input device.
let previewNextId = 1;
const previewCapturers: {
	id: number;
	name: string;
	state: string;
	pid: null;
	bindAddr: null;
	profileId: string;
	uptimeSecs: number;
	lastStderr: null;
	stderrTail: string[];
	exitCode: number | null;
}[] = [];

let appSettings = {
	stopCapturersOnExit: true,
	systemTrayEnabled: false,
	minimizeToTray: true,
	closeToTrayWhileRunning: true,
	startMinimizedToTray: false,
	jumpToCapturersOnQuickLaunch: false,
	themeMode: "system",
	consoleWindowX: null,
	consoleWindowY: null,
	consoleWindowWidth: null,
	consoleWindowHeight: null,
	apiWorkerThreads: 2,
	locale: "ja-JP",
};

export function installDevIpcMock(): void {
	if (hasTauriRuntime()) return;

	mockIPC((cmd, payload) => {
		const args = (payload ?? {}) as Record<string, unknown>;
		switch (cmd) {
			case "i18n_available_locales":
				return Object.keys(localeBundles);
			case "i18n_get_svelte_bundle": {
				const tag = String(args.locale ?? "ja-JP");
				return localeBundles[tag] ?? localeBundles["ja-JP"];
			}
			case "i18n_resolve_default_locale":
				return appSettings.locale;
			case "app_version":
				return "dev-preview";
			case "get_app_settings":
				return appSettings;
			case "sync_app_settings":
				appSettings = { ...appSettings, ...((args.settings as Record<string, unknown>) ?? {}) };
				return appSettings;
			case "list_capturers":
				return structuredClone(previewCapturers);
			case "list_profiles":
				return profiles.map((profile) => ({
					id: profile.id,
					name: profile.name,
					note: profile.note,
					iconPath: profile.iconPath,
					group: profile.group,
					engine: profile.runtime.engine,
				}));
			case "reorder_profiles": {
				const expected = args.expectedIds as string[] | undefined;
				if (!expected || expected.length !== profiles.length || profiles.some((profile, index) => profile.id !== expected[index])) throw new Error("Profile list changed. Reload the saved order before rearranging again.");
				const ids = (args.profileIds as string[]) ?? [];
				if (new Set(ids).size !== ids.length || ids.length !== profiles.length || profiles.some(profile => !ids.includes(profile.id))) {
					throw new Error("Profile list changed or contains duplicate IDs. Reload the saved order before rearranging again.");
				}
				profiles = ids
					.map((id) => profiles.find((profile) => profile.id === id))
					.filter((profile): profile is ProfileDetail => Boolean(profile))
					.concat(profiles.filter((profile) => !ids.includes(profile.id)));
				return profiles.map((profile) => ({
					id: profile.id,
					name: profile.name,
					note: profile.note,
					iconPath: profile.iconPath,
					group: profile.group,
					engine: profile.runtime.engine,
				}));
			}
			case "active_profile_id":
				return activeProfileId;
			case "selected_profile_id":
				return selectedProfileId;
			case "set_selected_profile_id": {
				if (args.profileId != null && typeof args.profileId !== "string") throw new Error("Invalid profile id");
				const normalized = ((args.profileId as string | null | undefined) ?? "").trim();
				if (normalized && !profiles.some((profile) => profile.id === normalized)) throw new Error(`profile not found: ${normalized}`);
				sessionStorage.setItem(selectedProfileStorageKey, normalized);
				selectedProfileId = normalized;
				return null;
			}
			case "set_active_profile":
				activeProfileId = String(args.profileId ?? activeProfileId);
				return activeProfileId;
			case "get_profile_detail":
				return previewRevisionDetail(findProfile(args.profileId));
			case "update_profile_field":
				return updatePreviewProfile(String(args.profileId), [{ field: args.field, value: args.value }], args.expectedRevision);
			case "update_profile_fields":
				return updatePreviewProfile(String(args.profileId), args.updates, args.expectedRevision);
			case "create_profile":
				return previewRevisionDetail(createPreviewProfile(args.name));
			case "duplicate_profile":
				return previewRevisionDetail(duplicatePreviewProfile(String(args.profileId)));
			case "delete_profile":
				if (args.expectedRevision != null && args.expectedRevision !== (String(args.expectedRevision).startsWith('delete:') ? 'delete:' : '') + JSON.stringify(findProfile(args.profileId))) throw new Error("profile_conflict: saved profile changed before deletion");
				archivePreviewProfile(String(args.profileId));
				return null;
			case "list_deleted_profiles":
				return [...archivedProfiles].map(([archive_id, profile]) => ({ archive_id, profile_id: profile.id, name: profile.name }));
			case "restore_deleted_profile":
				return restorePreviewProfile(String(args.archiveId));

			case "open_external_url":
					throw new Error("隔離プレビューではOSへの操作やファイル出力は実行していません。製品版で確認してください。 / Preview: native OS or file operation was not performed.");
			case "save_supervisor_logs":
					throw new Error("隔離プレビューではOSへの操作やファイル出力は実行していません。製品版で確認してください。 / Preview: native OS or file operation was not performed.");
			case "pick_file_path":
				return null;
			case "reveal_profiles_dir":
					throw new Error("隔離プレビューではOSへの操作やファイル出力は実行していません。製品版で確認してください。 / Preview: native OS or file operation was not performed.");
			case "reveal_supervisor_logs_dir":
					throw new Error("隔離プレビューではOSへの操作やファイル出力は実行していません。製品版で確認してください。 / Preview: native OS or file operation was not performed.");
				return null;
			case "launch_capturer": {
				const profile = findProfile(args.profileId);
				const existing = previewCapturers.find((entry) => entry.profileId === profile.id && ["starting", "running", "stopping"].includes(entry.state));
				if (existing) return structuredClone(existing);
				const instance = {
					id: previewNextId++,
					name: profile.name,
					state: "running",
					pid: null,
					bindAddr: null,
					profileId: profile.id,
					uptimeSecs: 0,
					lastStderr: null,
					stderrTail: [] as string[],
					exitCode: null,
				};
				previewCapturers.push(instance);
				return structuredClone(instance);
			}
			case "stop_capturer":
			case "stop_all_capturers": {
				for (const instance of previewCapturers) {
					if (cmd === "stop_all_capturers" || instance.id === Number(args.id)) {
						instance.state = "exited";
						instance.exitCode = 0;
					}
				}
				return null;
			}
			case "capturer_runtime_status": {
				const instance = previewCapturers.find((entry) => entry.id === Number(args.id));
				if (!instance) throw new Error(`dev IPC mock: capturer not found ${args.id}`);
				// Match the native DTO even without a real capture process. Do not
				// invent healthy telemetry or a motion snapshot for the preview.
				return {
					id: instance.id,
					state: instance.state,
					pid: instance.pid,
					bindAddr: instance.bindAddr,
					uptimeSecs: instance.uptimeSecs,
					healthy: false,
					note: "隔離プレビュー: 実Capturerには接続していません。 / Preview: no real Capturer connection.",
					snapshot: null,
				};
			}
			case "enumerate_webcams":
				return [
					{ id: "cam0:Preview Camera", label: "Preview Camera" },
					{ id: "cam1:Virtual Camera", label: "Virtual Camera" },
				];
			case "enumerate_webcam_formats":
				return [
					{ width: 1280, height: 720, fps: 60, pixelFormat: "NV12", label: "1280x720 @ 60 NV12" },
					{ width: 1920, height: 1080, fps: 30, pixelFormat: "MJPG", label: "1920x1080 @ 30 MJPG" },
				];
			default:
				throw new Error(`dev IPC mock: unsupported command ${cmd}`);
		}
	});
}

function findProfile(profileId: unknown): ProfileDetail {
	const id = String(profileId ?? activeProfileId);
	const profile = profiles.find((profile) => profile.id === id);
	if (!profile) throw new Error(`dev IPC mock: profile not found ${id}`);
	return profile;
}

function hasTauriRuntime(): boolean {
	return typeof window !== "undefined" && Boolean((window as any).__TAURI_INTERNALS__);
}

function parseFlatTomlMessages(text: string): Record<string, string> {
	const messages: Record<string, string> = {};
	let section = "";

	for (const rawLine of text.split(/\r?\n/)) {
		const line = rawLine.trim();
		if (!line || line.startsWith("#") || line.startsWith("_")) continue;
		const sectionMatch = line.match(/^\[([^\]]+)\]$/);
		if (sectionMatch) {
			section = sectionMatch[1] ?? "";
			continue;
		}
		const pairMatch = line.match(/^([A-Za-z0-9_]+)\s*=\s*("(?:\\.|[^"\\])*")\s*$/);
		if (!pairMatch) continue;
		const key = pairMatch[1];
		const value = JSON.parse(pairMatch[2]) as string;
		messages[section ? `${section}.${key}` : key] = value.replaceAll("%{", "{");
	}

	return messages;
}
