use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use image::ImageReader;
use rosc::{OscMessage, OscPacket, OscType};
use serde::Serialize;
use serde_json::json;
use un_motion_frame::{
	CoordinateSpace, LengthUnit, MotionSignal, MotionSignalValue, MotionSourceInfo, MotionSourceKind, SampleState, TimestampBasis,
	TrackingState, UNMotionFrame,
};
use un_motion_mediapipe_native::{
	HAND_LANDMARK_COUNT, NativeFace, NativeGesture, NativeGestures, NativeHand, NativeHands, NativeHolistic, NativeMediaPipeOptions,
	NativeMediaPipeOutput, NativeMediaPipeRuntime, NativePose, RUNNING_MODE_IMAGE, RUNNING_MODE_LIVE_STREAM, RUNNING_MODE_VIDEO,
	RgbImageRef, resolve_media_pipe_root, resolve_native_dir,
};
use un_motion_output_vmc::vmc_packets_for_frame;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeSummary {
	image: String,
	dll: String,
	return_code: i32,
	result_timestamp_ms: Option<i64>,
	width: u32,
	height: u32,
	pose_landmarks: u32,
	pose_world_landmarks: u32,
	pose_confidence: f32,
	pose_segmentation_mask: Option<MaskSummary>,
	hand_count: u32,
	hands: Vec<HandSummary>,
	face: FaceSummary,
	gestures: GestureSummary,
	holistic: HolisticSummary,
	output_web_pose: Option<String>,
	output_vmc: Option<String>,
	running_mode: String,
	repeat: u32,
	timing: ProbeTimingSummary,
	stability: Option<StabilitySummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeTimingSummary {
	elapsed_ms: f32,
	avg_frame_ms: f32,
	effective_fps: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StabilitySummary {
	frames: u32,
	return_codes: Vec<i32>,
	pose_max_delta: f32,
	pose_world_max_delta: f32,
	hand_max_delta: f32,
	hand_world_max_delta: f32,
	face_max_delta: f32,
	blendshape_max_delta: f32,
	adjacent_max_delta: StabilityDeltaSummary,
	tail_max_delta: StabilityDeltaSummary,
	hand_count_range: [u32; 2],
	face_landmark_count_range: [u32; 2],
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StabilityDeltaSummary {
	pose: f32,
	pose_world: f32,
	hand: f32,
	hand_world: f32,
	face: f32,
	blendshape: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandSummary {
	side: String,
	landmarks: u32,
	world_landmarks: u32,
	confidence: f32,
	handedness_score: f32,
	index_curl: f32,
	index_mcp_curl: f32,
	index_pip_curl: f32,
	index_dip_curl: f32,
	sibling_fold: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MaskSummary {
	width: u32,
	height: u32,
	mean: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FaceSummary {
	landmarks: u32,
	confidence: f32,
	matrix_rows: u32,
	matrix_cols: u32,
	blendshape_count: u32,
	top_blendshapes: Vec<BlendshapeSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BlendshapeSummary {
	name: String,
	score: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GestureSummary {
	count: u32,
	gestures: Vec<SingleGestureSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SingleGestureSummary {
	side: String,
	score: f32,
	categories: Vec<BlendshapeSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HolisticSummary {
	pose_landmarks: u32,
	pose_world_landmarks: u32,
	left_hand_landmarks: u32,
	left_hand_world_landmarks: u32,
	right_hand_landmarks: u32,
	right_hand_world_landmarks: u32,
	face_landmarks: u32,
	face_blendshape_count: u32,
	pose_segmentation_mask: Option<MaskSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebPoseFrame {
	sequence: u64,
	capture_timestamp_ms: f64,
	confidence: f32,
	signals: Vec<WebPoseSignal>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebPoseSignal {
	name: String,
	value: f32,
	confidence: f32,
}

#[derive(Clone, Copy)]
struct Point3 {
	x: f32,
	y: f32,
	z: f32,
}

fn main() -> anyhow::Result<()> {
	let args = Args::parse()?;
	let original_cwd = env::current_dir().context("failed to read current directory")?;
	let image_path = args
		.image
		.canonicalize()
		.with_context(|| format!("failed to resolve image {}", args.image.display()))?;
	let output_web_pose = args.output_web_pose.as_ref().map(|path| absolutize(&original_cwd, path));
	let output_vmc = args.output_vmc.as_ref().map(|path| absolutize(&original_cwd, path));
	let media_pipe_input = args.media_pipe_root.canonicalize().with_context(|| {
		format!(
			"failed to resolve media pipe root {}; use --media-pipe-root",
			args.media_pipe_root.display()
		)
	})?;
	let media_pipe_root = resolve_media_pipe_root(&media_pipe_input)?;
	let native_dir = resolve_native_dir(&media_pipe_input, &media_pipe_root);
	let dll_path = args.dll.clone().unwrap_or_else(|| native_dir.join("un-motion-mediapipe.dll"));
	let dll_path = dll_path
		.canonicalize()
		.with_context(|| format!("failed to resolve DLL {}", dll_path.display()))?;
	let pose_model = media_pipe_root.join("models/pose_landmarker_lite.task");
	let hand_model = media_pipe_root.join("models/hand_landmarker.task");
	let face_model = media_pipe_root.join("models/face_landmarker.task");
	let gesture_model = media_pipe_root.join("models/gesture_recognizer.task");
	let holistic_model = media_pipe_root.join("models/holistic_landmarker.task");
	if !pose_model.exists() {
		bail!("missing pose model: {}", pose_model.display());
	}
	if !hand_model.exists() {
		bail!("missing hand model: {}", hand_model.display());
	}
	if !face_model.exists() {
		bail!("missing face model: {}", face_model.display());
	}
	if !gesture_model.exists() {
		bail!("missing gesture model: {}", gesture_model.display());
	}
	if !holistic_model.exists() {
		bail!("missing holistic model: {}", holistic_model.display());
	}

	// Keep native default model paths working for backends that resolve models from cwd.
	env::set_current_dir(&media_pipe_root).with_context(|| format!("failed to chdir {}", media_pipe_root.display()))?;
	unsafe {
		env::set_var("UN_MOTION_MEDIAPIPE_MODEL", &pose_model);
		env::set_var("UN_MOTION_MEDIAPIPE_HAND_MODEL", &hand_model);
		env::set_var("UN_MOTION_MEDIAPIPE_FACE_MODEL", &face_model);
		env::set_var("UN_MOTION_MEDIAPIPE_GESTURE_MODEL", &gesture_model);
		env::set_var("UN_MOTION_MEDIAPIPE_HOLISTIC_MODEL", &holistic_model);
	}

	let image = load_rgb_image(&image_path)?;
	let native_options = probe_options(args.running_mode, args.holistic)?;
	let mut native = NativeMediaPipeRuntime::open_with_options(&dll_path, native_options)?;
	let (output, timing, stability) = run_probe_frames(&mut native, &image, args.running_mode, args.repeat, args.holistic)?;
	let return_code = output.return_code;
	let pose = output.pose;
	let hands = output.hands;
	let face = output.face;
	let gestures = output.gestures;
	let holistic = output.holistic;

	let timestamp_ms = now_unix_ns() as f64 / 1_000_000.0;
	let web_pose = native_to_web_pose(args.sequence, timestamp_ms, &pose, &hands);
	if let Some(path) = &output_web_pose {
		write_json(path, &web_pose)?;
	}

	if let Some(path) = &output_vmc {
		let frame = web_pose_to_frame(&web_pose);
		let messages = flatten_messages(vmc_packets_for_frame(&frame));
		write_messages_jsonl(path, &messages, web_pose.capture_timestamp_ms)?;
	}

	let summary = ProbeSummary {
		image: image_path.display().to_string(),
		dll: dll_path.display().to_string(),
		return_code,
		result_timestamp_ms: output.result_timestamp_ms,
		width: image.width,
		height: image.height,
		pose_landmarks: pose.landmark_count,
		pose_world_landmarks: pose.world_landmark_count,
		pose_confidence: rounded(pose.confidence),
		pose_segmentation_mask: mask_summary(&pose),
		hand_count: hands.hand_count,
		hands: hand_summaries(&hands),
		face: face_summary(&face),
		gestures: gesture_summary(&gestures),
		holistic: holistic_summary(&holistic),
		output_web_pose: output_web_pose.as_ref().map(|path| path.display().to_string()),
		output_vmc: output_vmc.as_ref().map(|path| path.display().to_string()),
		running_mode: args.running_mode.to_string(),
		repeat: args.repeat,
		timing,
		stability,
	};
	println!("{}", serde_json::to_string_pretty(&summary)?);
	Ok(())
}

struct Args {
	image: PathBuf,
	media_pipe_root: PathBuf,
	dll: Option<PathBuf>,
	output_web_pose: Option<PathBuf>,
	output_vmc: Option<PathBuf>,
	sequence: u64,
	running_mode: NativeRunningModeArg,
	repeat: u32,
	holistic: bool,
}

impl Args {
	fn parse() -> anyhow::Result<Self> {
		let mut image = None;
		let mut media_pipe_root = PathBuf::from(".");
		let mut dll = None;
		let mut output_web_pose = None;
		let mut output_vmc = None;
		let mut sequence = 0_u64;
		let mut running_mode = NativeRunningModeArg::Image;
		let mut repeat = 1_u32;
		let mut holistic = false;
		let mut args = env::args().skip(1);
		while let Some(arg) = args.next() {
			match arg.as_str() {
				"--image" | "-i" => image = args.next().map(PathBuf::from),
				"--media-pipe-root" => media_pipe_root = args.next().map(PathBuf::from).context("missing --media-pipe-root value")?,
				"--dll" => dll = args.next().map(PathBuf::from),
				"--output-web-pose" => output_web_pose = args.next().map(PathBuf::from),
				"--output-vmc" => output_vmc = args.next().map(PathBuf::from),
				"--holistic" => holistic = true,
				"--running-mode" => {
					let value = args.next().context("missing --running-mode value")?;
					running_mode = NativeRunningModeArg::parse(&value)?;
				}
				"--sequence" => {
					let value = args.next().context("missing --sequence value")?;
					sequence = value.parse().with_context(|| format!("invalid --sequence {value}"))?;
				}
				"--repeat" => {
					let value = args.next().context("missing --repeat value")?;
					repeat = value.parse().with_context(|| format!("invalid --repeat {value}"))?;
					if repeat == 0 {
						bail!("--repeat must be greater than zero");
					}
				}
				"--help" | "-h" => {
					print_usage();
					std::process::exit(0);
				}
				_ if image.is_none() => image = Some(PathBuf::from(arg)),
				_ => bail!("unexpected argument: {arg}"),
			}
		}
		let Some(image) = image else {
			print_usage();
			bail!("missing --image");
		};
		Ok(Self {
			image,
			media_pipe_root,
			dll,
			output_web_pose,
			output_vmc,
			sequence,
			running_mode,
			repeat,
			holistic,
		})
	}
}

fn print_usage() {
	eprintln!(
		"usage: un-motion-native-mediapipe-probe --image image.png [--running-mode image|video|live-stream] [--holistic] [--repeat n] [--media-pipe-root .] [--output-web-pose out.json] [--output-vmc out.jsonl]"
	);
}

#[derive(Clone, Copy)]
enum NativeRunningModeArg {
	Image,
	Video,
	LiveStream,
}

impl NativeRunningModeArg {
	fn parse(value: &str) -> anyhow::Result<Self> {
		match value {
			"image" => Ok(Self::Image),
			"video" => Ok(Self::Video),
			"live-stream" | "live_stream" | "livestream" | "live" => Ok(Self::LiveStream),
			_ => bail!("invalid --running-mode {value}; expected image, video, or live-stream"),
		}
	}
}

impl std::fmt::Display for NativeRunningModeArg {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Image => f.write_str("image"),
			Self::Video => f.write_str("video"),
			Self::LiveStream => f.write_str("live-stream"),
		}
	}
}

fn probe_options(mode: NativeRunningModeArg, holistic: bool) -> anyhow::Result<NativeMediaPipeOptions> {
	let running_mode = match mode {
		NativeRunningModeArg::Image => RUNNING_MODE_IMAGE,
		NativeRunningModeArg::Video => RUNNING_MODE_VIDEO,
		NativeRunningModeArg::LiveStream => RUNNING_MODE_LIVE_STREAM,
	};
	let mut options = NativeMediaPipeOptions {
		running_mode,
		enable_gestures: 1,
		..NativeMediaPipeOptions::desktop_video()
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

fn run_probe_frames(
	native: &mut NativeMediaPipeRuntime,
	image: &RgbImageData,
	_running_mode: NativeRunningModeArg,
	repeat: u32,
	_holistic: bool,
) -> anyhow::Result<(NativeMediaPipeOutput, ProbeTimingSummary, Option<StabilitySummary>)> {
	let mut outputs = Vec::with_capacity(repeat as usize);
	let started = Instant::now();
	for frame_index in 0..repeat {
		let timestamp_ms = i64::from(frame_index);
		let output = native.process_rgb_everything_at(
			RgbImageRef {
				bytes: &image.bytes,
				width: image.width,
				height: image.height,
				stride: image.stride,
			},
			timestamp_ms,
		)?;
		outputs.push(output);
	}
	let elapsed_ms = started.elapsed().as_secs_f32() * 1000.0;
	let avg_frame_ms = elapsed_ms / repeat as f32;
	let timing = ProbeTimingSummary {
		elapsed_ms: rounded(elapsed_ms),
		avg_frame_ms: rounded(avg_frame_ms),
		effective_fps: rounded(1000.0 / avg_frame_ms.max(0.001)),
	};
	let output = outputs.last().copied().context("probe produced no frames")?;
	let stability = (repeat > 1).then(|| summarize_stability(&outputs));
	Ok((output, timing, stability))
}

fn summarize_stability(outputs: &[un_motion_mediapipe_native::NativeMediaPipeOutput]) -> StabilitySummary {
	let first = outputs[0];
	let first_pose = primary_pose(&first);
	let first_face = primary_face(&first);
	let first_hands = primary_hands(&first);
	let mut return_codes = Vec::new();
	let mut pose_max_delta = 0.0_f32;
	let mut pose_world_max_delta = 0.0_f32;
	let mut hand_max_delta = 0.0_f32;
	let mut hand_world_max_delta = 0.0_f32;
	let mut face_max_delta = 0.0_f32;
	let mut blendshape_max_delta = 0.0_f32;
	let mut hand_min = u32::MAX;
	let mut hand_max = 0_u32;
	let mut face_min = u32::MAX;
	let mut face_max = 0_u32;
	let mut adjacent = StabilityDeltaSummary::default();

	for (index, output) in outputs.iter().enumerate() {
		if !return_codes.contains(&output.return_code) {
			return_codes.push(output.return_code);
		}
		let output_pose = primary_pose(output);
		let output_face = primary_face(output);
		let output_hands = primary_hands(output);
		let hand_count = output_hands.len() as u32;
		hand_min = hand_min.min(hand_count);
		hand_max = hand_max.max(hand_count);
		face_min = face_min.min(output_face.landmark_count);
		face_max = face_max.max(output_face.landmark_count);
		pose_max_delta = pose_max_delta.max(landmark_delta(
			&first_pose.landmarks,
			&output_pose.landmarks,
			first_pose.landmark_count.min(output_pose.landmark_count) as usize,
		));
		pose_world_max_delta = pose_world_max_delta.max(landmark_delta(
			&first_pose.world_landmarks,
			&output_pose.world_landmarks,
			first_pose.world_landmark_count.min(output_pose.world_landmark_count) as usize,
		));
		for (left, right) in first_hands.iter().zip(output_hands.iter()).take(2) {
			let left = **left;
			let right = **right;
			hand_max_delta = hand_max_delta.max(landmark_delta(
				&left.landmarks,
				&right.landmarks,
				left.landmark_count.min(right.landmark_count) as usize,
			));
			hand_world_max_delta = hand_world_max_delta.max(landmark_delta(
				&left.world_landmarks,
				&right.world_landmarks,
				left.world_landmark_count.min(right.world_landmark_count) as usize,
			));
		}
		face_max_delta = face_max_delta.max(landmark_delta(
			&first_face.landmarks,
			&output_face.landmarks,
			first_face.landmark_count.min(output_face.landmark_count) as usize,
		));
		let blend_count = first_face.blendshape_count.min(output_face.blendshape_count) as usize;
		for index in 0..blend_count {
			blendshape_max_delta =
				blendshape_max_delta.max((first_face.blendshapes[index].score - output_face.blendshapes[index].score).abs());
		}
		if index > 0 {
			adjacent = adjacent.max(output_delta(&outputs[index - 1], output));
		}
	}
	let tail = if outputs.len() > 6 {
		let tail_outputs = &outputs[outputs.len() - 6..];
		let mut max_tail = StabilityDeltaSummary::default();
		for output in tail_outputs {
			max_tail = max_tail.max(output_delta(&tail_outputs[0], output));
		}
		max_tail
	} else {
		StabilityDeltaSummary::default()
	};

	StabilitySummary {
		frames: outputs.len() as u32,
		return_codes,
		pose_max_delta: rounded(pose_max_delta),
		pose_world_max_delta: rounded(pose_world_max_delta),
		hand_max_delta: rounded(hand_max_delta),
		hand_world_max_delta: rounded(hand_world_max_delta),
		face_max_delta: rounded(face_max_delta),
		blendshape_max_delta: rounded(blendshape_max_delta),
		adjacent_max_delta: adjacent.rounded(),
		tail_max_delta: tail.rounded(),
		hand_count_range: [hand_min, hand_max],
		face_landmark_count_range: [face_min, face_max],
	}
}

impl StabilityDeltaSummary {
	fn max(self, other: Self) -> Self {
		Self {
			pose: self.pose.max(other.pose),
			pose_world: self.pose_world.max(other.pose_world),
			hand: self.hand.max(other.hand),
			hand_world: self.hand_world.max(other.hand_world),
			face: self.face.max(other.face),
			blendshape: self.blendshape.max(other.blendshape),
		}
	}

	fn rounded(self) -> Self {
		Self {
			pose: rounded(self.pose),
			pose_world: rounded(self.pose_world),
			hand: rounded(self.hand),
			hand_world: rounded(self.hand_world),
			face: rounded(self.face),
			blendshape: rounded(self.blendshape),
		}
	}
}

fn output_delta(left: &NativeMediaPipeOutput, right: &NativeMediaPipeOutput) -> StabilityDeltaSummary {
	let mut hand = 0.0_f32;
	let mut hand_world = 0.0_f32;
	let left_hands = primary_hands(left);
	let right_hands = primary_hands(right);
	for (left_hand, right_hand) in left_hands.iter().zip(right_hands.iter()).take(2) {
		let left_hand = **left_hand;
		let right_hand = **right_hand;
		hand = hand.max(landmark_delta(
			&left_hand.landmarks,
			&right_hand.landmarks,
			left_hand.landmark_count.min(right_hand.landmark_count) as usize,
		));
		hand_world = hand_world.max(landmark_delta(
			&left_hand.world_landmarks,
			&right_hand.world_landmarks,
			left_hand.world_landmark_count.min(right_hand.world_landmark_count) as usize,
		));
	}
	let mut blendshape = 0.0_f32;
	let left_pose = primary_pose(left);
	let right_pose = primary_pose(right);
	let left_face = primary_face(left);
	let right_face = primary_face(right);
	let blend_count = left_face.blendshape_count.min(right_face.blendshape_count) as usize;
	for index in 0..blend_count {
		blendshape = blendshape.max((left_face.blendshapes[index].score - right_face.blendshapes[index].score).abs());
	}
	StabilityDeltaSummary {
		pose: landmark_delta(
			&left_pose.landmarks,
			&right_pose.landmarks,
			left_pose.landmark_count.min(right_pose.landmark_count) as usize,
		),
		pose_world: landmark_delta(
			&left_pose.world_landmarks,
			&right_pose.world_landmarks,
			left_pose.world_landmark_count.min(right_pose.world_landmark_count) as usize,
		),
		hand,
		hand_world,
		face: landmark_delta(
			&left_face.landmarks,
			&right_face.landmarks,
			left_face.landmark_count.min(right_face.landmark_count) as usize,
		),
		blendshape,
	}
}

fn primary_pose(output: &NativeMediaPipeOutput) -> &NativePose {
	if output.holistic.pose.landmark_count > 0 || output.holistic.pose.world_landmark_count > 0 {
		&output.holistic.pose
	} else {
		&output.pose
	}
}

fn primary_face(output: &NativeMediaPipeOutput) -> &NativeFace {
	if output.holistic.face.landmark_count > 0 || output.holistic.face.blendshape_count > 0 {
		&output.holistic.face
	} else {
		&output.face
	}
}

fn primary_hands(output: &NativeMediaPipeOutput) -> Vec<&NativeHand> {
	let holistic_has_hands = output.holistic.left_hand.landmark_count > 0 || output.holistic.right_hand.landmark_count > 0;
	if holistic_has_hands {
		let mut hands = Vec::with_capacity(2);
		if output.holistic.left_hand.landmark_count > 0 {
			hands.push(&output.holistic.left_hand);
		}
		if output.holistic.right_hand.landmark_count > 0 {
			hands.push(&output.holistic.right_hand);
		}
		hands
	} else {
		output.hands.hands.iter().take(output.hands.hand_count as usize).collect()
	}
}

fn landmark_delta<const N: usize>(
	left: &[un_motion_mediapipe_native::NativeLandmark; N],
	right: &[un_motion_mediapipe_native::NativeLandmark; N],
	count: usize,
) -> f32 {
	let mut max_delta = 0.0_f32;
	for index in 0..count {
		max_delta = max_delta.max((left[index].x - right[index].x).abs());
		max_delta = max_delta.max((left[index].y - right[index].y).abs());
		max_delta = max_delta.max((left[index].z - right[index].z).abs());
		max_delta = max_delta.max((left[index].visibility - right[index].visibility).abs());
		max_delta = max_delta.max((left[index].presence - right[index].presence).abs());
	}
	max_delta
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
	if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

struct RgbImageData {
	width: u32,
	height: u32,
	stride: u32,
	bytes: Vec<u8>,
}

fn load_rgb_image(path: &Path) -> anyhow::Result<RgbImageData> {
	let image = ImageReader::open(path)
		.with_context(|| format!("failed to open image {}", path.display()))?
		.decode()
		.with_context(|| format!("failed to decode image {}", path.display()))?
		.to_rgb8();
	let (width, height) = image.dimensions();
	Ok(RgbImageData {
		width,
		height,
		stride: width * 3,
		bytes: image.into_raw(),
	})
}

fn native_to_web_pose(sequence: u64, capture_timestamp_ms: f64, pose: &NativePose, hands: &NativeHands) -> WebPoseFrame {
	let mut signals = Vec::new();
	if pose.landmark_count >= 33 {
		push_pose_signals(&mut signals, pose);
	}
	for hand in hands
		.hands
		.iter()
		.take(hands.hand_count as usize)
		.filter(|hand| hand.landmark_count >= 21)
	{
		let side = match hand.handedness_is_right {
			0 => "left",
			1 => "right",
			_ => continue,
		};
		push_hand_signals(&mut signals, side, hand);
	}
	WebPoseFrame {
		sequence,
		capture_timestamp_ms,
		confidence: if signals.is_empty() { 0.0 } else { 1.0 },
		signals,
	}
}

fn push_hand_signals(signals: &mut Vec<WebPoseSignal>, side: &str, hand: &NativeHand) {
	let confidence = hand.confidence.max(hand.handedness_score).clamp(0.0, 1.0);
	let landmarks = hand_points(hand);
	let wrist = landmarks[0];
	push_scalar(signals, format!("hand.{side}.x"), (wrist.x - 0.5) * 2.0, confidence);
	push_scalar(signals, format!("hand.{side}.y"), (0.5 - wrist.y) * 2.0, confidence);
	push_scalar(signals, format!("hand.{side}.z"), (-wrist.z).clamp(-1.0, 1.0), confidence);
	push_scalar(signals, format!("hand.{side}.open"), hand_open(&landmarks), confidence);
	push_scalar(signals, format!("hand.{side}.pinch"), finger_pinch(&landmarks), confidence);
	push_scalar(signals, format!("hand.{side}.palm.roll"), palm_roll(&landmarks), confidence);
	push_wrist_rotation_signals(signals, side, &landmarks, confidence);
	push_finger_curl_signals(signals, side, &landmarks, confidence);
	push_finger_spread_signals(signals, side, &landmarks, confidence);
}

fn push_wrist_rotation_signals(signals: &mut Vec<WebPoseSignal>, side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT], confidence: f32) {
	let wrist = landmarks[0];
	let index_mcp = landmarks[5];
	let middle_mcp = landmarks[9];
	let little_mcp = landmarks[17];
	let Some(forward) = normalize3(vec3(middle_mcp, wrist)) else {
		return;
	};
	let Some(across) = normalize3(vec3(index_mcp, little_mcp)) else {
		return;
	};
	let Some(normal) = normalize3(cross3(across, forward)) else {
		return;
	};
	let side_sign = if side == "left" { 1.0 } else { -1.0 };
	let pitch = (forward.y.atan2((forward.x * forward.x + forward.z * forward.z).sqrt()) / 1.2).clamp(-1.0, 1.0);
	let yaw = (forward.x.atan2(-forward.z) / 1.2).clamp(-1.0, 1.0);
	let roll = (across.y.atan2(across.x) / 1.2).clamp(-1.0, 1.0);
	for (name, value) in [
		("palm.forward.x", forward.x),
		("palm.forward.y", forward.y),
		("palm.forward.z", forward.z),
		("palm.across.x", across.x),
		("palm.across.y", across.y),
		("palm.across.z", across.z),
		("palm.normal.x", normal.x),
		("palm.normal.y", normal.y),
		("palm.normal.z", normal.z),
		("wrist.pitch", pitch),
		("wrist.yaw", yaw * side_sign),
		("wrist.roll", roll * side_sign),
	] {
		push_scalar(signals, format!("hand.{side}.{name}"), value, confidence);
	}
}

fn push_finger_curl_signals(signals: &mut Vec<WebPoseSignal>, side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT], confidence: f32) {
	push_scalar(signals, format!("hand.{side}.thumb.curl"), thumb_curl(landmarks), confidence);
	push_joint_curls(signals, side, "thumb", landmarks, [0, 1, 2, 3, 4], confidence);
	for (finger, indices) in [
		("index", [0, 5, 6, 7, 8]),
		("middle", [0, 9, 10, 11, 12]),
		("ring", [0, 13, 14, 15, 16]),
		("little", [0, 17, 18, 19, 20]),
	] {
		push_scalar(
			signals,
			format!("hand.{side}.{finger}.curl"),
			finger_curl(landmarks, [indices[1], indices[2], indices[3], indices[4]]),
			confidence,
		);
		push_joint_curls(signals, side, finger, landmarks, indices, confidence);
	}
}

fn push_joint_curls(
	signals: &mut Vec<WebPoseSignal>,
	side: &str,
	finger: &str,
	landmarks: &[Point3; HAND_LANDMARK_COUNT],
	indices: [usize; 5],
	confidence: f32,
) {
	let [root, mcp, pip, dip, tip] = indices;
	push_scalar(
		signals,
		format!("hand.{side}.{finger}.mcp.curl"),
		joint_curl(landmarks[root], landmarks[mcp], landmarks[pip]),
		confidence,
	);
	push_scalar(
		signals,
		format!("hand.{side}.{finger}.pip.curl"),
		joint_curl(landmarks[mcp], landmarks[pip], landmarks[dip]),
		confidence,
	);
	push_scalar(
		signals,
		format!("hand.{side}.{finger}.dip.curl"),
		joint_curl(landmarks[pip], landmarks[dip], landmarks[tip]),
		confidence,
	);
}

fn push_finger_spread_signals(signals: &mut Vec<WebPoseSignal>, side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT], confidence: f32) {
	let middle = finger_direction_angle(landmarks, 9, 12);
	for (finger, value) in [
		("thumb", ((finger_direction_angle(landmarks, 1, 4) - middle) / 1.2).clamp(-1.0, 1.0)),
		("index", ((finger_direction_angle(landmarks, 5, 8) - middle) / 0.7).clamp(-1.0, 1.0)),
		("middle", 0.0),
		(
			"ring",
			((finger_direction_angle(landmarks, 13, 16) - middle) / 0.7).clamp(-1.0, 1.0),
		),
		(
			"little",
			((finger_direction_angle(landmarks, 17, 20) - middle) / 0.7).clamp(-1.0, 1.0),
		),
	] {
		push_scalar(signals, format!("hand.{side}.{finger}.spread"), value, confidence);
	}
}

fn web_pose_to_frame(pose: &WebPoseFrame) -> UNMotionFrame {
	let mut frame = UNMotionFrame::new(pose.sequence);
	let capture_timestamp_ns = if pose.capture_timestamp_ms.is_finite() && pose.capture_timestamp_ms >= 0.0 {
		(pose.capture_timestamp_ms * 1_000_000.0) as u64
	} else {
		now_unix_ns()
	};
	let now = now_unix_ns();
	frame.header.timestamp_basis = TimestampBasis::SourceLocal;
	frame.header.capture_timestamp_ns = capture_timestamp_ns;
	frame.header.frame_timestamp_ns = capture_timestamp_ns;
	frame.header.processed_timestamp_ns = now;
	frame.header.coordinate_space = CoordinateSpace::UNMotion;
	frame.header.length_unit = LengthUnit::Normalized;
	frame.sources.push(MotionSourceInfo {
		source_id: "experiment:mediapipe-native".to_string(),
		source_kind: MotionSourceKind::ImagePose,
		display_name: Some("Native MediaPipe DLL".to_string()),
		confidence: pose.confidence.clamp(0.0, 1.0),
		latency_ns: None,
		state: if pose.signals.is_empty() {
			TrackingState::Lost
		} else {
			TrackingState::Valid
		},
	});
	frame.signals = pose
		.signals
		.iter()
		.map(|signal| MotionSignal {
			name: signal.name.clone(),
			value: MotionSignalValue::Scalar(signal.value.clamp(-1.0, 1.0)),
			confidence: signal.confidence.clamp(0.0, 1.0),
			source_index: Some(0),
			state: SampleState::Valid,
		})
		.collect();
	frame
}

fn hand_summaries(hands: &NativeHands) -> Vec<HandSummary> {
	hands
		.hands
		.iter()
		.take(hands.hand_count as usize)
		.filter(|hand| hand.landmark_count >= HAND_LANDMARK_COUNT as u32)
		.map(|hand| {
			let side = match hand.handedness_is_right {
				0 => "left",
				1 => "right",
				_ => "unknown",
			};
			let landmarks = hand_points(hand);
			let sibling_fold = ["middle", "ring", "little"]
				.iter()
				.map(|finger| match *finger {
					"middle" => finger_curl(&landmarks, [9, 10, 11, 12]),
					"ring" => finger_curl(&landmarks, [13, 14, 15, 16]),
					_ => finger_curl(&landmarks, [17, 18, 19, 20]),
				})
				.sum::<f32>()
				/ 3.0;
			HandSummary {
				side: side.to_string(),
				landmarks: hand.landmark_count,
				world_landmarks: hand.world_landmark_count,
				confidence: rounded(hand.confidence),
				handedness_score: rounded(hand.handedness_score),
				index_curl: rounded(finger_curl(&landmarks, [5, 6, 7, 8])),
				index_mcp_curl: rounded(joint_curl(landmarks[0], landmarks[5], landmarks[6])),
				index_pip_curl: rounded(joint_curl(landmarks[5], landmarks[6], landmarks[7])),
				index_dip_curl: rounded(joint_curl(landmarks[6], landmarks[7], landmarks[8])),
				sibling_fold: rounded(sibling_fold),
			}
		})
		.collect()
}

fn mask_summary(pose: &NativePose) -> Option<MaskSummary> {
	(pose.segmentation_mask_present != 0).then_some(MaskSummary {
		width: pose.segmentation_mask_width,
		height: pose.segmentation_mask_height,
		mean: rounded(pose.segmentation_mask_mean),
	})
}

fn face_summary(face: &NativeFace) -> FaceSummary {
	let mut blendshapes = face
		.blendshapes
		.iter()
		.take(face.blendshape_count as usize)
		.map(|blendshape| BlendshapeSummary {
			name: blendshape_name(blendshape.name),
			score: rounded(blendshape.score),
		})
		.filter(|blendshape| !blendshape.name.is_empty() && blendshape.name != "_neutral")
		.collect::<Vec<_>>();
	blendshapes.sort_by(|left, right| right.score.partial_cmp(&left.score).unwrap_or(std::cmp::Ordering::Equal));
	blendshapes.truncate(8);
	FaceSummary {
		landmarks: face.landmark_count,
		confidence: rounded(face.confidence),
		matrix_rows: face.matrix_rows,
		matrix_cols: face.matrix_cols,
		blendshape_count: face.blendshape_count,
		top_blendshapes: blendshapes,
	}
}

fn blendshape_name(bytes: [u8; 64]) -> String {
	let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
	String::from_utf8_lossy(&bytes[..end]).to_string()
}

fn gesture_summary(gestures: &NativeGestures) -> GestureSummary {
	GestureSummary {
		count: gestures.gesture_count,
		gestures: gestures
			.gestures
			.iter()
			.take(gestures.gesture_count as usize)
			.map(single_gesture_summary)
			.collect(),
	}
}

fn single_gesture_summary(gesture: &NativeGesture) -> SingleGestureSummary {
	SingleGestureSummary {
		side: match gesture.handedness_is_right {
			0 => "left",
			1 => "right",
			_ => "unknown",
		}
		.to_string(),
		score: rounded(gesture.handedness_score),
		categories: gesture
			.categories
			.iter()
			.take(gesture.category_count as usize)
			.map(|category| BlendshapeSummary {
				name: blendshape_name(category.name),
				score: rounded(category.score),
			})
			.filter(|category| !category.name.is_empty())
			.collect(),
	}
}

fn holistic_summary(holistic: &NativeHolistic) -> HolisticSummary {
	HolisticSummary {
		pose_landmarks: holistic.pose.landmark_count,
		pose_world_landmarks: holistic.pose.world_landmark_count,
		left_hand_landmarks: holistic.left_hand.landmark_count,
		left_hand_world_landmarks: holistic.left_hand.world_landmark_count,
		right_hand_landmarks: holistic.right_hand.landmark_count,
		right_hand_world_landmarks: holistic.right_hand.world_landmark_count,
		face_landmarks: holistic.face.landmark_count,
		face_blendshape_count: holistic.face.blendshape_count,
		pose_segmentation_mask: mask_summary(&holistic.pose),
	}
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	std::fs::write(path, serde_json::to_string_pretty(value)?).with_context(|| format!("failed to write {}", path.display()))
}

fn write_messages_jsonl(path: &Path, messages: &[OscMessage], capture_timestamp_ms: f64) -> anyhow::Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	let timestamp_ms = if capture_timestamp_ms.is_finite() && capture_timestamp_ms >= 0.0 {
		capture_timestamp_ms as u64
	} else {
		now_unix_ns() / 1_000_000
	};
	let mut writer = BufWriter::new(File::create(path).with_context(|| format!("failed to create {}", path.display()))?);
	for message in messages {
		let entry = json!({
			"timestampMs": timestamp_ms,
			"sourceAddr": "offline:native-mediapipe",
			"addr": message.addr,
			"args": message.args.iter().map(vmc_record_arg).collect::<Vec<_>>(),
		});
		writeln!(writer, "{}", serde_json::to_string(&entry)?)?;
	}
	Ok(())
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

fn vmc_record_arg(arg: &OscType) -> serde_json::Value {
	match arg {
		OscType::String(value) => json!({ "type": "string", "value": value }),
		OscType::Float(value) => json!({ "type": "float", "value": value }),
		OscType::Int(value) => json!({ "type": "int", "value": value }),
		OscType::Double(value) => json!({ "type": "double", "value": value }),
		OscType::Long(value) => json!({ "type": "long", "value": value }),
		OscType::Bool(value) => json!({ "type": "bool", "value": value }),
		OscType::Nil => json!({ "type": "nil", "value": null }),
		_ => json!({ "type": "unsupported", "value": format!("{arg:?}") }),
	}
}

fn push_pose_signals(signals: &mut Vec<WebPoseSignal>, pose: &NativePose) {
	let left_shoulder = pose_point(pose, 11);
	let right_shoulder = pose_point(pose, 12);
	let left_elbow = pose_point(pose, 13);
	let right_elbow = pose_point(pose, 14);
	let left_wrist = pose_point(pose, 15);
	let right_wrist = pose_point(pose, 16);
	push_point_signal(signals, "pose.left.shoulder", left_shoulder, pose.confidence);
	push_point_signal(signals, "pose.right.shoulder", right_shoulder, pose.confidence);
	push_point_signal(signals, "pose.left.elbow", left_elbow, pose.confidence);
	push_point_signal(signals, "pose.right.elbow", right_elbow, pose.confidence);
	push_point_signal(signals, "pose.left.wrist", left_wrist, pose.confidence);
	push_point_signal(signals, "pose.right.wrist", right_wrist, pose.confidence);
}

fn push_point_signal(signals: &mut Vec<WebPoseSignal>, prefix: &str, point: Point3, confidence: f32) {
	push_scalar(signals, format!("{prefix}.x"), (point.x - 0.5) * 2.0, confidence);
	push_scalar(signals, format!("{prefix}.y"), (0.5 - point.y) * 2.0, confidence);
	push_scalar(signals, format!("{prefix}.z"), point.z.clamp(-1.0, 1.0), confidence);
}

fn push_scalar(signals: &mut Vec<WebPoseSignal>, name: String, value: f32, confidence: f32) {
	signals.push(WebPoseSignal {
		name,
		value: value.clamp(-1.0, 1.0),
		confidence: confidence.clamp(0.0, 1.0),
	});
}

fn hand_points(hand: &NativeHand) -> [Point3; HAND_LANDMARK_COUNT] {
	let mut points = [Point3 { x: 0.0, y: 0.0, z: 0.0 }; HAND_LANDMARK_COUNT];
	for (index, landmark) in hand.landmarks.iter().enumerate() {
		points[index] = Point3 {
			x: landmark.x,
			y: landmark.y,
			z: landmark.z,
		};
	}
	points
}

fn pose_point(pose: &NativePose, index: usize) -> Point3 {
	let landmark = pose.landmarks[index];
	Point3 {
		x: landmark.x,
		y: landmark.y,
		z: landmark.z,
	}
}

fn finger_pinch(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	(1.0 - distance3d(landmarks[4], landmarks[8]) / (hand_palm_scale(landmarks) * 0.95)).clamp(0.0, 1.0)
}

fn hand_open(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	let wrist = landmarks[0];
	let tip_spread = [8, 12, 16, 20]
		.iter()
		.map(|index| distance3d(wrist, landmarks[*index]))
		.sum::<f32>()
		/ 4.0;
	let mcp_spread = [5, 9, 13, 17].iter().map(|index| distance3d(wrist, landmarks[*index])).sum::<f32>() / 4.0;
	((tip_spread - mcp_spread) / hand_palm_scale(landmarks)).clamp(0.0, 1.0)
}

fn finger_direction_angle(landmarks: &[Point3; HAND_LANDMARK_COUNT], base_index: usize, tip_index: usize) -> f32 {
	let base = landmarks[base_index];
	let tip = landmarks[tip_index];
	(tip.y - base.y).atan2(tip.x - base.x)
}

fn finger_curl(landmarks: &[Point3; HAND_LANDMARK_COUNT], indices: [usize; 4]) -> f32 {
	let [mcp, pip, dip, tip] = indices;
	let chain_length = distance3d(landmarks[mcp], landmarks[pip])
		+ distance3d(landmarks[pip], landmarks[dip])
		+ distance3d(landmarks[dip], landmarks[tip]);
	if chain_length <= 1e-5 {
		return 0.0;
	}
	(1.0 - distance3d(landmarks[mcp], landmarks[tip]) / chain_length).clamp(0.0, 1.0)
}

fn joint_curl(previous: Point3, joint: Point3, next: Point3) -> f32 {
	let a = vec3(previous, joint);
	let b = vec3(next, joint);
	let angle = angle_between3(a, b);
	((std::f32::consts::PI - angle) / 1.35).clamp(0.0, 1.0)
}

fn thumb_curl(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	let wrist = landmarks[0];
	let thumb_tip = landmarks[4];
	let index_mcp = landmarks[5];
	let closed_distance = distance3d(thumb_tip, index_mcp);
	let open_distance = distance3d(thumb_tip, wrist);
	(1.0 - closed_distance / open_distance.max(hand_palm_scale(landmarks))).clamp(0.0, 1.0)
}

fn palm_roll(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	let index_mcp = landmarks[5];
	let little_mcp = landmarks[17];
	((index_mcp.y - little_mcp.y).atan2(index_mcp.x - little_mcp.x) / std::f32::consts::PI).clamp(-1.0, 1.0)
}

fn hand_palm_scale(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	distance3d(landmarks[0], landmarks[9]).max(0.08)
}

fn distance3d(a: Point3, b: Point3) -> f32 {
	let dx = a.x - b.x;
	let dy = a.y - b.y;
	let dz = a.z - b.z;
	(dx * dx + dy * dy + dz * dz).sqrt()
}

fn vec3(a: Point3, b: Point3) -> Point3 {
	Point3 {
		x: a.x - b.x,
		y: a.y - b.y,
		z: a.z - b.z,
	}
}

fn dot3(a: Point3, b: Point3) -> f32 {
	a.x * b.x + a.y * b.y + a.z * b.z
}

fn cross3(a: Point3, b: Point3) -> Point3 {
	Point3 {
		x: a.y * b.z - a.z * b.y,
		y: a.z * b.x - a.x * b.z,
		z: a.x * b.y - a.y * b.x,
	}
}

fn normalize3(v: Point3) -> Option<Point3> {
	let length = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
	if length <= 1e-6 {
		return None;
	}
	Some(Point3 {
		x: v.x / length,
		y: v.y / length,
		z: v.z / length,
	})
}

fn angle_between3(a: Point3, b: Point3) -> f32 {
	let Some(na) = normalize3(a) else {
		return 0.0;
	};
	let Some(nb) = normalize3(b) else {
		return 0.0;
	};
	dot3(na, nb).clamp(-1.0, 1.0).acos()
}

fn rounded(value: f32) -> f32 {
	(value * 10_000.0).round() / 10_000.0
}

fn now_unix_ns() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_nanos()
		.min(u128::from(u64::MAX)) as u64
}
