use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use rosc::{OscMessage, OscPacket, OscType};
use serde::Serialize;
use serde_json::json;
use un_motion_engine_mediapipe_native::{NativeMediaPipeEngineConfig, NativeMediaPipeImageEngine};
use un_motion_engine_mediapipe_post_process::{MediaPipePostProcessConfig, MediaPipePostProcessRules, MediaPipePostProcessor};
use un_motion_input_file_image::FileImageInputSource;
use un_motion_interfaces::ImageInputSource;
use un_motion_mediapipe_native::{
	NativeFace, NativeMediaPipeOptions, NativePose, RUNNING_MODE_IMAGE, RUNNING_MODE_LIVE_STREAM, RUNNING_MODE_VIDEO,
	resolve_media_pipe_root, resolve_native_dir,
};
use un_motion_output_vmc::vmc_packets_for_frame;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineProbeSummary {
	image: String,
	dll: String,
	running_mode: String,
	holistic: bool,
	rule_preset: String,
	head_source: String,
	eye_open_bias: f32,
	rules: RuleSummary,
	width: u32,
	height: u32,
	output_sequence: u64,
	source_state: String,
	source_confidence: f32,
	signal_count: usize,
	signals: Vec<SignalSummary>,
	body: BodySummary,
	left_hand: HandSummary,
	right_hand: HandSummary,
	face_metrics: Option<FaceMetricSummary>,
	notes: Vec<String>,
	output_vmc: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleSummary {
	head_from_pose: bool,
	head_from_face_matrix: bool,
	head_reconcile: bool,
	neutral_eye_fallback: bool,
	hand_camera_target: bool,
	hand_orientation: bool,
	finger_derived: bool,
	arm_from_pose: bool,
	arm_ik_from_hands: bool,
	crossed_hand_heuristic: bool,
	coordinate_correction: bool,
	final_clamp: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignalSummary {
	name: String,
	value: f32,
	confidence: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BodySummary {
	present: bool,
	bone_count: usize,
	bones: Vec<BoneSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BoneSummary {
	bone: String,
	rotation: QuatSummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandSummary {
	present: bool,
	wrist_present: bool,
	finger_count: usize,
	joint_count: usize,
	fingers: Vec<FingerSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FingerSummary {
	finger: String,
	joint_rotations: Vec<QuatSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuatSummary {
	x: f32,
	y: f32,
	z: f32,
	w: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FaceMetricSummary {
	nose_drop_eye_chin: f32,
	nose_drop_eye_mouth: f32,
	mouth_drop_eye_chin: f32,
	face_width: f32,
	eye_width: f32,
	face_height: f32,
}

struct Args {
	image: PathBuf,
	media_pipe_root: PathBuf,
	dll: Option<PathBuf>,
	running_mode: String,
	holistic: bool,
	rule_preset: String,
	head_source: HeadSourceMode,
	camera_diagonal_view_angle_deg: f32,
	eye_open_bias: f32,
	output_vmc: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadSourceMode {
	All,
	Face,
	Pose,
}

fn main() -> anyhow::Result<()> {
	let args = Args::parse()?;
	let original_cwd = env::current_dir().context("failed to read current directory")?;
	let image_path = args
		.image
		.canonicalize()
		.with_context(|| format!("failed to resolve image {}", args.image.display()))?;
	let media_pipe_input = absolutize(&original_cwd, &args.media_pipe_root)
		.canonicalize()
		.with_context(|| format!("failed to resolve media pipe root {}", args.media_pipe_root.display()))?;
	let media_pipe_root = resolve_media_pipe_root(&media_pipe_input)?;
	let native_dir = resolve_native_dir(&media_pipe_input, &media_pipe_root);
	let dll_path = args.dll.unwrap_or_else(|| native_dir.join("un-motion-mediapipe.dll"));
	let dll_path = absolutize(&original_cwd, &dll_path)
		.canonicalize()
		.with_context(|| format!("failed to resolve DLL {}", dll_path.display()))?;
	let output_vmc = args.output_vmc.as_ref().map(|path| absolutize(&original_cwd, path));
	configure_native_env(&media_pipe_root)?;

	let mut input = FileImageInputSource::open_once(&image_path)?;
	let image = input.next_image_frame()?.context("file image input emitted no frame")?;
	let mut rules = rules_for_preset(&args.rule_preset)?;
	apply_head_source_rules(&mut rules, args.head_source);
	let mut engine = NativeMediaPipeImageEngine::open_dll(
		&dll_path,
		NativeMediaPipeEngineConfig {
			options: native_options(&args.running_mode, args.holistic)?,
			include_gestures: false,
		},
	)?;
	let mut native = engine.process_rgb_frame(&image)?;
	apply_head_source_native_mask(&mut native, args.head_source);
	let mut post_processor = MediaPipePostProcessor::new(MediaPipePostProcessConfig {
		input_width: image.width,
		input_height: image.height,
		camera_diagonal_view_angle_deg: args.camera_diagonal_view_angle_deg,
		eye_open_bias: args.eye_open_bias,
		source_id: "probe:mediapipe-native".to_string(),
		display_name: "MediaPipe Native Probe".to_string(),
		rules: rules.clone(),
		..MediaPipePostProcessConfig::default()
	});
	let frame = post_processor.process_native_output(&image, &native);
	if let Some(path) = &output_vmc {
		let messages = flatten_messages(vmc_packets_for_frame(&frame));
		write_messages_jsonl(path, &messages, frame.header.capture_timestamp_ns as f64 / 1_000_000.0)?;
	}
	let source = frame.sources.first();
	let summary = PipelineProbeSummary {
		image: image_path.display().to_string(),
		dll: dll_path.display().to_string(),
		running_mode: args.running_mode,
		holistic: args.holistic,
		rule_preset: args.rule_preset,
		head_source: args.head_source.as_str().to_string(),
		eye_open_bias: args.eye_open_bias,
		rules: RuleSummary::from(&rules),
		width: image.width,
		height: image.height,
		output_sequence: frame.header.sequence,
		source_state: source
			.map(|source| format!("{:?}", source.state))
			.unwrap_or_else(|| "None".to_string()),
		source_confidence: source.map(|source| source.confidence).unwrap_or_default(),
		signal_count: frame.signals.len(),
		signals: frame
			.signals
			.iter()
			.map(|signal| SignalSummary {
				name: signal.name.clone(),
				value: match signal.value {
					un_motion_frame::MotionSignalValue::Scalar(value) => value,
					_ => 0.0,
				},
				confidence: signal.confidence,
			})
			.collect(),
		body: BodySummary::from(frame.body.as_ref()),
		left_hand: HandSummary::from(frame.left_hand.as_ref()),
		right_hand: HandSummary::from(frame.right_hand.as_ref()),
		face_metrics: face_metric_summary(primary_face(&native)),
		notes: frame.metadata.notes,
		output_vmc: output_vmc.as_ref().map(|path| path.display().to_string()),
	};
	println!("{}", serde_json::to_string_pretty(&summary)?);
	Ok(())
}

impl Args {
	fn parse() -> anyhow::Result<Self> {
		let mut image = None;
		let mut media_pipe_root = PathBuf::from(".");
		let mut dll = None;
		let mut running_mode = "image".to_string();
		let mut holistic = false;
		let mut rule_preset = "stable".to_string();
		let mut head_source = HeadSourceMode::All;
		let mut camera_diagonal_view_angle_deg = 70.0_f32;
		let mut eye_open_bias = 0.5_f32;
		let mut output_vmc = None;
		let mut args = env::args().skip(1);
		while let Some(arg) = args.next() {
			match arg.as_str() {
				"--image" => image = args.next().map(PathBuf::from),
				"--media-pipe-root" => media_pipe_root = args.next().map(PathBuf::from).context("missing --media-pipe-root value")?,
				"--dll" => dll = args.next().map(PathBuf::from),
				"--running-mode" => running_mode = args.next().context("missing --running-mode value")?,
				"--holistic" => holistic = true,
				"--rule-preset" => rule_preset = args.next().context("missing --rule-preset value")?,
				"--head-source" => {
					let value = args.next().context("missing --head-source value")?;
					head_source = HeadSourceMode::parse(&value)?;
				}
				"--output-vmc" => output_vmc = args.next().map(PathBuf::from),
				"--fov" | "--camera-diagonal-view-angle-deg" => {
					let value = args.next().context("missing --fov value")?;
					camera_diagonal_view_angle_deg = value.parse().with_context(|| format!("invalid --fov {value}"))?;
				}
				"--eye-open-bias" => {
					let value = args.next().context("missing --eye-open-bias value")?;
					eye_open_bias = value.parse().with_context(|| format!("invalid --eye-open-bias {value}"))?;
				}
				"--help" | "-h" => {
					print_usage();
					std::process::exit(0);
				}
				_ => bail!("unexpected argument: {arg}"),
			}
		}
		let image = image.context("missing --image path")?;
		Ok(Self {
			image,
			media_pipe_root,
			dll,
			running_mode,
			holistic,
			rule_preset,
			head_source,
			camera_diagonal_view_angle_deg: camera_diagonal_view_angle_deg.clamp(30.0, 170.0),
			eye_open_bias: eye_open_bias.clamp(0.0, 1.0),
			output_vmc,
		})
	}
}

fn print_usage() {
	eprintln!(
		"usage: un-motion-native-image-pipeline-probe --image path [--media-pipe-root .] [--dll path] [--running-mode image|video|live-stream] [--holistic] [--rule-preset stable|diagnostic-minimal|vmc-compare] [--head-source all|face|pose] [--fov degrees] [--eye-open-bias 0..1] [--output-vmc out.jsonl]"
	);
}

impl HeadSourceMode {
	fn parse(value: &str) -> anyhow::Result<Self> {
		match value {
			"all" => Ok(Self::All),
			"face" | "face-matrix" => Ok(Self::Face),
			"pose" | "pose-world" => Ok(Self::Pose),
			_ => bail!("unsupported --head-source {value}; expected all, face, or pose"),
		}
	}

	fn as_str(self) -> &'static str {
		match self {
			Self::All => "all",
			Self::Face => "face",
			Self::Pose => "pose",
		}
	}
}

fn apply_head_source_rules(rules: &mut MediaPipePostProcessRules, mode: HeadSourceMode) {
	match mode {
		HeadSourceMode::All => {}
		HeadSourceMode::Face => {
			rules.head_from_pose = false;
			rules.head_reconcile = false;
		}
		HeadSourceMode::Pose => {
			rules.head_from_face_matrix = false;
			rules.head_reconcile = false;
			rules.neutral_eye_fallback = false;
		}
	}
}

fn apply_head_source_native_mask(native: &mut un_motion_mediapipe_native::NativeMediaPipeOutput, mode: HeadSourceMode) {
	match mode {
		HeadSourceMode::All => {}
		HeadSourceMode::Face => {
			native.pose = NativePose::default();
			native.holistic.pose = NativePose::default();
		}
		HeadSourceMode::Pose => {
			native.face = NativeFace::default();
			native.holistic.face = NativeFace::default();
		}
	}
}

fn primary_face(native: &un_motion_mediapipe_native::NativeMediaPipeOutput) -> &NativeFace {
	if native.holistic.face.landmark_count > 0 {
		&native.holistic.face
	} else {
		&native.face
	}
}

fn face_metric_summary(face: &NativeFace) -> Option<FaceMetricSummary> {
	if face.landmark_count < 478 {
		return None;
	}
	let nose = face.landmarks[1];
	let chin = face.landmarks[152];
	let left_eye_outer = face.landmarks[33];
	let right_eye_outer = face.landmarks[263];
	let left_face = face.landmarks[234];
	let right_face = face.landmarks[454];
	let left_mouth = face.landmarks[61];
	let right_mouth = face.landmarks[291];
	let eye_mid_y = (left_eye_outer.y + right_eye_outer.y) * 0.5;
	let mouth_mid_y = (left_mouth.y + right_mouth.y) * 0.5;
	let face_height = (chin.y - eye_mid_y).abs().max(1e-5);
	let eye_mouth_height = (mouth_mid_y - eye_mid_y).abs().max(1e-5);
	let eye_width = (right_eye_outer.x - left_eye_outer.x).abs().max(1e-5);
	let face_width = (right_face.x - left_face.x).abs().max(eye_width).max(1e-5);
	Some(FaceMetricSummary {
		nose_drop_eye_chin: rounded((nose.y - eye_mid_y) / face_height),
		nose_drop_eye_mouth: rounded((nose.y - eye_mid_y) / eye_mouth_height),
		mouth_drop_eye_chin: rounded((mouth_mid_y - eye_mid_y) / face_height),
		face_width: rounded(face_width),
		eye_width: rounded(eye_width),
		face_height: rounded(face_height),
	})
}

fn rounded(value: f32) -> f32 {
	(value * 1000.0).round() / 1000.0
}

fn rules_for_preset(name: &str) -> anyhow::Result<MediaPipePostProcessRules> {
	let mut rules = MediaPipePostProcessRules::default();
	match name {
		"stable" | "vmc-compare" => {}
		"diagnostic-minimal" => {
			rules.head_reconcile = false;
			rules.neutral_eye_fallback = false;
			rules.finger_derived = false;
			rules.arm_ik_from_hands = false;
			rules.crossed_hand_heuristic = false;
			rules.coordinate_correction = false;
		}
		other => bail!("unsupported rule preset: {other}"),
	}
	Ok(rules)
}

impl From<&MediaPipePostProcessRules> for RuleSummary {
	fn from(rules: &MediaPipePostProcessRules) -> Self {
		Self {
			head_from_pose: rules.head_from_pose,
			head_from_face_matrix: rules.head_from_face_matrix,
			head_reconcile: rules.head_reconcile,
			neutral_eye_fallback: rules.neutral_eye_fallback,
			hand_camera_target: rules.hand_camera_target,
			hand_orientation: rules.hand_orientation,
			finger_derived: rules.finger_derived,
			arm_from_pose: rules.arm_from_pose,
			arm_ik_from_hands: rules.arm_ik_from_hands,
			crossed_hand_heuristic: rules.crossed_hand_heuristic,
			coordinate_correction: rules.coordinate_correction,
			final_clamp: rules.final_clamp,
		}
	}
}

impl BodySummary {
	fn from(body: Option<&un_motion_frame::BodyMotion>) -> Self {
		let Some(humanoid) = body.and_then(|body| body.humanoid.as_ref()) else {
			return Self {
				present: false,
				bone_count: 0,
				bones: Vec::new(),
			};
		};
		let bones = humanoid
			.bones
			.iter()
			.map(|bone| {
				let rotation = bone.transform.rotation.unwrap_or(un_motion_frame::Quatf {
					x: 0.0,
					y: 0.0,
					z: 0.0,
					w: 1.0,
				});
				BoneSummary {
					bone: format!("{:?}", bone.bone),
					rotation: QuatSummary {
						x: rotation.x,
						y: rotation.y,
						z: rotation.z,
						w: rotation.w,
					},
				}
			})
			.collect::<Vec<_>>();
		Self {
			present: true,
			bone_count: bones.len(),
			bones,
		}
	}
}

impl HandSummary {
	fn from(hand: Option<&un_motion_frame::HandMotion>) -> Self {
		let Some(hand) = hand else {
			return Self {
				present: false,
				wrist_present: false,
				finger_count: 0,
				joint_count: 0,
				fingers: Vec::new(),
			};
		};
		Self {
			present: true,
			wrist_present: hand.wrist.is_some(),
			finger_count: hand.fingers.len(),
			joint_count: hand.fingers.iter().map(|finger| finger.joints.len()).sum(),
			fingers: hand
				.fingers
				.iter()
				.map(|finger| FingerSummary {
					finger: format!("{:?}", finger.finger),
					joint_rotations: finger
						.joints
						.iter()
						.map(|joint| {
							let rotation = joint.rotation.unwrap_or(un_motion_frame::Quatf {
								x: 0.0,
								y: 0.0,
								z: 0.0,
								w: 1.0,
							});
							QuatSummary {
								x: rotation.x,
								y: rotation.y,
								z: rotation.z,
								w: rotation.w,
							}
						})
						.collect(),
				})
				.collect(),
		}
	}
}

fn native_options(mode: &str, holistic: bool) -> anyhow::Result<NativeMediaPipeOptions> {
	let mut options = match mode.to_ascii_lowercase().as_str() {
		"image" => NativeMediaPipeOptions {
			running_mode: RUNNING_MODE_IMAGE,
			..NativeMediaPipeOptions::desktop_video()
		},
		"video" => NativeMediaPipeOptions {
			running_mode: RUNNING_MODE_VIDEO,
			..NativeMediaPipeOptions::desktop_video()
		},
		"live" | "live-stream" | "live_stream" | "livestream" => NativeMediaPipeOptions {
			running_mode: RUNNING_MODE_LIVE_STREAM,
			..NativeMediaPipeOptions::desktop_video()
		},
		other => bail!("unsupported running mode: {other}"),
	};
	if holistic {
		options.enable_pose = 1;
		options.enable_hands = 1;
		options.enable_face = 1;
		options.enable_gestures = 0;
		options.enable_holistic = 1;
	}
	Ok(options)
}

fn configure_native_env(media_pipe_root: &Path) -> anyhow::Result<()> {
	let pose_model = media_pipe_root.join("models/pose_landmarker_lite.task");
	let hand_model = media_pipe_root.join("models/hand_landmarker.task");
	let face_model = media_pipe_root.join("models/face_landmarker.task");
	let holistic_model = media_pipe_root.join("models/holistic_landmarker.task");
	for path in [&pose_model, &hand_model, &face_model, &holistic_model] {
		if !path.exists() {
			bail!("missing MediaPipe model: {}", path.display());
		}
	}
	unsafe {
		env::set_var("UN_MOTION_MEDIAPIPE_MODEL", pose_model);
		env::set_var("UN_MOTION_MEDIAPIPE_HAND_MODEL", hand_model);
		env::set_var("UN_MOTION_MEDIAPIPE_FACE_MODEL", face_model);
		env::set_var("UN_MOTION_MEDIAPIPE_HOLISTIC_MODEL", holistic_model);
		env::set_var("UN_MOTION_MEDIAPIPE_QUIET", "1");
		env::set_var("UN_MOTION_MEDIAPIPE_LOG_LEVEL", "3");
		env::set_var("TF_CPP_MIN_LOG_LEVEL", "3");
		env::set_var("GLOG_minloglevel", "2");
	}
	Ok(())
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
	if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

fn flatten_messages(packets: Vec<OscPacket>) -> Vec<OscMessage> {
	let mut messages = Vec::new();
	for packet in packets {
		flatten_packet(packet, &mut messages);
	}
	messages
}

fn flatten_packet(packet: OscPacket, messages: &mut Vec<OscMessage>) {
	match packet {
		OscPacket::Message(message) => messages.push(message),
		OscPacket::Bundle(bundle) => {
			for packet in bundle.content {
				flatten_packet(packet, messages);
			}
		}
	}
}

fn write_messages_jsonl(path: &Path, messages: &[OscMessage], capture_timestamp_ms: f64) -> anyhow::Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)?;
	}
	let mut writer = BufWriter::new(File::create(path).with_context(|| format!("failed to create {}", path.display()))?);
	for message in messages {
		let entry = json!({
			"addr": message.addr,
			"args": message.args.iter().map(vmc_record_arg).collect::<Vec<_>>(),
			"sourceAddr": "offline:native-image-pipeline",
			"timestampMs": capture_timestamp_ms,
		});
		writeln!(writer, "{}", serde_json::to_string(&entry)?)?;
	}
	Ok(())
}

fn vmc_record_arg(arg: &OscType) -> serde_json::Value {
	match arg {
		OscType::String(value) => json!({ "type": "string", "value": value }),
		OscType::Float(value) => json!({ "type": "float", "value": value }),
		OscType::Int(value) => json!({ "type": "int", "value": value }),
		OscType::Double(value) => json!({ "type": "double", "value": value }),
		OscType::Long(value) => json!({ "type": "long", "value": value }),
		OscType::Bool(value) => json!({ "type": "bool", "value": value }),
		OscType::Nil => json!({ "type": "nil", "value": null }),
		_ => json!({ "type": "other", "value": format!("{arg:?}") }),
	}
}
