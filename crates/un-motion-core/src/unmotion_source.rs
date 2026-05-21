//! Capturer の正式経路における Source/Engine 境界。
//!
//! この module は入力ごとの詳細を `UNMotionFrame` に正規化する。MediaPipe Native は
//! `ImageFrame -> Native output -> MediaPipePostProcessor -> UNMotionFrame`、VMC /
//! iFacialMocap は protocol decode -> `UNMotionFrame` として扱う。下流の Modifier /
//! Output は MediaPipe raw output や protocol 固有 frame を直接見ない。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::Context;
use std::sync::atomic::Ordering;
use tracing::{info, warn};
use un_motion_engine_mediapipe_native::{NativeMediaPipeEngineConfig, NativeMediaPipeImageEngine};
use un_motion_engine_mediapipe_post_process::{
	FacePoseModelConfig, MediaPipePostProcessConfig, MediaPipePostProcessRules, MediaPipePostProcessor,
};
use un_motion_frame::UNMotionFrame;
use un_motion_input_file_image::{FileImageEmissionMode, FileImageInputConfig, FileImageInputSource};
use un_motion_input_file_video::{FileVideoInputConfig, FileVideoInputSource};
use un_motion_input_webcam_directshow::{
	DirectShowCaptureConfig, DirectShowWebcamBackend, WebcamImageInputSource as DirectShowWebcamImageInputSource,
};
use un_motion_interfaces::{ImageFrame, ImageInputSource, ImageResizeOptions, ResizeAxis, RgbaColor};
#[cfg(not(windows))]
use un_motion_mediapipe_native::DELEGATE_GPU;
use un_motion_mediapipe_native::{
	DELEGATE_CPU, DELEGATE_XNNPACK, NativeLiveStreamStats, NativeMediaPipeOptions, NativeMediaPipeOutput, RUNNING_MODE_IMAGE,
};

use un_motion_runtime::{MotionFrameSource, SourceTelemetryHandle};

use crate::runtime_host::{CoreMediaPipePostProcessSettings, CoreMotionFrameStreamConfig, CoreUnmotionResizeConfig};

const LIVE_STREAM_PENDING_INPUT_CAPACITY: usize = 256;

pub(crate) fn open_motion_frame_source(config: &CoreMotionFrameStreamConfig) -> anyhow::Result<Option<CoreMotionFrameSource>> {
	// VMC 受信エンジンに分岐。profile の `runtime_engine` が `"vmc"` の場合は
	// VMC/UDP を listen し、OSC frame を UNMotionFrame に正規化する。
	if config.runtime_engine == "vmc" {
		return open_vmc_receive_source(config).map(Some);
	}

	// iFacialMocap 受信エンジンに分岐。profile の `runtime_engine` が
	// `"ifacialmocap"` の場合は UDP listen し、iFacialMocap テキストプロトコルを
	// UNMotionFrame に正規化する。
	if config.runtime_engine == "ifacialmocap" {
		return open_ifacialmocap_receive_source(config).map(Some);
	}

	if config.runtime_engine != "mediapipe-native" {
		info!(
			target: "un_motion_core::unmotion_source",
			runtime_engine = %config.runtime_engine,
			profile_stream_id = %config.profile_stream_id,
			"runtime_engine is not mediapipe-native; skip native MediaPipe source open",
		);
		return Ok(None);
	}

	info!(
		target: "un_motion_core::unmotion_source",
		input_component = %config.input_component,
		profile_stream_id = %config.profile_stream_id,
		device_id = %config.device_id,
		input_width = ?config.input_width,
		input_height = ?config.input_height,
		input_fps = config.input_fps,
		"opening MediaPipe Native source",
	);
	match config.input_component.as_str() {
		"file-image" => open_file_image_source(config),
		"file-video" => open_file_video_source(config),
		"webcam-directshow" => open_directshow_source(config),
		// GUI 上は "Webcam (MediaFoundation)" として表示する。
		// 内部実装は nokhwa で、現状キャプチャ自体は未対応。
		"webcam-nokhwa" | "webcam-mediafoundation" => {
			// nokhwa 経路は experimental 扱い。`un-motion-input-webcam-nokhwa` のキャプチャ実装が
			// まだスタブ (常に `Ok(None)`) なので、Core からネイティブ MediaPipe へ画像を渡せない。
			// v1 では Windows 以外の Webcam 入力はサポート対象外であることを明示する。
			anyhow::bail!(
				"Webcam (MediaFoundation) input is experimental and not yet wired into MediaPipe Native; use Webcam (DirectShow) on Windows"
			)
		}
		_ => Ok(None),
	}
}

/// VMC 受信エンジンの組み立て。
///
/// listen address は profile の **専用フィールド** `runtime_selection.vmc_receive_listen_addr`
/// から取得する (例: `0.0.0.0:39539`)。未指定なら既定値 `0.0.0.0:39539` (VMC 標準
/// ポート) を使う。
///
/// VMC 専用 listen address と MediaPipe webcam 用 device id のフィールド責務を
/// 分離している。Engine Type の変更 (例: webcam → VMC) が同一プロファイル内で発生
/// しても、各 Engine の固有設定が混ざらないようにする。
fn open_vmc_receive_source(config: &CoreMotionFrameStreamConfig) -> anyhow::Result<CoreMotionFrameSource> {
	let listen_addr_str = config.vmc_receive_listen_addr.as_deref().unwrap_or("0.0.0.0:39539");
	let listen_addr: std::net::SocketAddr = listen_addr_str
		.parse()
		.with_context(|| format!("invalid VMC receive listen address: {listen_addr_str}"))?;
	info!(
		target: "un_motion_core::unmotion_source",
		profile_stream_id = %config.profile_stream_id,
		listen_addr = %listen_addr,
		"opening VMC receive source (MotionFrameSource trait via VmcUnmotionSource)",
	);
	let source_id = format!("vmc-receive:{}", config.profile_stream_id);
	let source = un_motion_runtime::VmcUnmotionSource::bind(source_id, listen_addr)
		.with_context(|| format!("VMC receive engine bind failed at {listen_addr}"))?;
	Ok(CoreMotionFrameSource::VmcReceive(source))
}

/// iFacialMocap 受信エンジン (UDP listen) の組み立て。
///
/// listen address は profile の専用フィールド
/// `runtime_selection.ifacialmocap_receive_listen_addr` から取得する
/// (例: `0.0.0.0:49983`)。未指定なら iFacialMocap UDP デフォルトポート 49983 を使う。
///
/// `engine_type = "ifacialmocap"` のときに `open_motion_frame_source` から呼ばれる。
/// VMC 受信エンジンと同様、protocol 固有 frame をここで `UNMotionFrame` に正規化し、
/// 下流は Modifier → Output の正式経路だけを扱う。
fn open_ifacialmocap_receive_source(config: &CoreMotionFrameStreamConfig) -> anyhow::Result<CoreMotionFrameSource> {
	let listen_addr_str = config.ifacialmocap_receive_listen_addr.as_deref().unwrap_or("0.0.0.0:49983");
	let listen_addr: std::net::SocketAddr = listen_addr_str
		.parse()
		.with_context(|| format!("invalid iFacialMocap receive listen address: {listen_addr_str}"))?;
	info!(
		target: "un_motion_core::unmotion_source",
		profile_stream_id = %config.profile_stream_id,
		listen_addr = %listen_addr,
		"opening iFacialMocap receive source (MotionFrameSource trait via IfacialMocapUnmotionSource)",
	);
	let source_id = format!("ifacialmocap-receive:{}", config.profile_stream_id);
	let source = un_motion_runtime::IfacialMocapUnmotionSource::bind(source_id, listen_addr)
		.with_context(|| format!("iFacialMocap receive engine bind failed at {listen_addr}"))?;
	Ok(CoreMotionFrameSource::IfacialMocapReceive(source))
}

pub(crate) enum CoreMotionFrameSource {
	FileImage(CoreImageMotionFrameSource<FileImageInputSource>),
	FileVideo(CoreImageMotionFrameSource<FileVideoInputSource>),
	DirectShow(CoreImageMotionFrameSource<DirectShowWebcamImageInputSource<DirectShowWebcamBackend>>),
	/// VMC/UDP 受信エンジン (`UNMotionFrame` を直接生成し Modifier →
	/// [UNMF/Z, VMC/UDP] に流すコンバータ用)。
	VmcReceive(un_motion_runtime::VmcUnmotionSource),
	/// iFacialMocap UDP 受信エンジン (face / eye / blendshape を `UNMotionFrame` に直接変換)。
	IfacialMocapReceive(un_motion_runtime::IfacialMocapUnmotionSource),
}

impl MotionFrameSource for CoreMotionFrameSource {
	fn next_frame(&mut self) -> anyhow::Result<Option<UNMotionFrame>> {
		match self {
			Self::FileImage(source) => source.next_frame(),
			Self::FileVideo(source) => source.next_frame(),
			Self::DirectShow(source) => source.next_frame(),
			Self::VmcReceive(source) => source.next_frame(),
			Self::IfacialMocapReceive(source) => source.next_frame(),
		}
	}

	fn telemetry_handle(&self) -> Option<un_motion_runtime::SourceTelemetryHandle> {
		// 各 source が露出するロックフリー atomic counter を Capturer の runtime loop が
		// `runtime_snapshot` 直前に `load(Relaxed)` し、Supervisor 側で Δcount/Δt として
		// FPS 表示に使う。VMC / iFacialMocap の受信エンジンに加えて、MediaPipe Native
		// 系 (FileImage / FileVideo / DirectShow) もハンドルを露出する。
		match self {
			Self::FileImage(source) => MotionFrameSource::telemetry_handle(source),
			Self::FileVideo(source) => MotionFrameSource::telemetry_handle(source),
			Self::DirectShow(source) => MotionFrameSource::telemetry_handle(source),
			Self::VmcReceive(source) => MotionFrameSource::telemetry_handle(source),
			Self::IfacialMocapReceive(source) => MotionFrameSource::telemetry_handle(source),
		}
	}
}

pub(crate) struct CoreImageMotionFrameSource<I> {
	input: I,
	engine: NativeMediaPipeImageEngine,
	post_processor: MediaPipePostProcessor,
	post_process_component: String,
	input_component: String,
	runtime_engine: String,
	live_stream_mode: bool,
	native_options: NativeMediaPipeOptions,
	pending_live_stream_inputs: VecDeque<SubmittedImageFrame>,
	/// Capturer 内のロックフリーテレメトリ。`next_frame()` 呼出毎にカメラ/ファイル
	/// 由来 1 サンプル受信、Engine 推論 1 結果出力としてカウンタを +1 する。
	/// Supervisor 側でこの差分を周期サンプリングし Capturers 一覧の Src FPS 表示に使う。
	telemetry: SourceTelemetryHandle,
}

#[derive(Clone, Debug)]
struct SubmittedImageFrame {
	timestamp_ms: i64,
	sequence: u64,
	capture_timestamp_ns: u64,
	width: u32,
	height: u32,
	source_label: Option<String>,
}

impl SubmittedImageFrame {
	fn from_image(image: &ImageFrame) -> Self {
		Self {
			timestamp_ms: image.metadata.capture_timestamp_ns.saturating_div(1_000_000).min(i64::MAX as u64) as i64,
			sequence: image.metadata.sequence,
			capture_timestamp_ns: image.metadata.capture_timestamp_ns,
			width: image.width,
			height: image.height,
			source_label: image.metadata.source_label.clone(),
		}
	}
}

impl<I> MotionFrameSource for CoreImageMotionFrameSource<I>
where
	I: ImageInputSource + Send + 'static,
{
	fn next_frame(&mut self) -> anyhow::Result<Option<UNMotionFrame>> {
		if self.live_stream_mode
			&& let Some(frame) = self.drain_live_stream_result()?
		{
			return Ok(Some(frame));
		}
		let Some(image) = self.input.next_image_frame()? else {
			return Ok(None);
		};
		if let Some(source_fps) = self.input.observed_source_fps().filter(|fps| fps.is_finite() && *fps > 0.0) {
			let milli_fps = (source_fps * 1000.0).round().clamp(0.0, u64::MAX as f32) as u64;
			self.telemetry.atomics.observed_source_fps_milli.store(milli_fps, Ordering::Relaxed);
		}
		self.telemetry.atomics.raw_received.fetch_add(1, Ordering::Relaxed);
		let submitted = SubmittedImageFrame::from_image(&image);
		if self.live_stream_mode {
			self.pending_live_stream_inputs.push_back(submitted.clone());
			while self.pending_live_stream_inputs.len() > LIVE_STREAM_PENDING_INPUT_CAPACITY {
				self.pending_live_stream_inputs.pop_front();
			}
			let native = self.engine.process_rgb_frame(&image)?;
			self.update_native_live_stream_stats();
			if native.return_code != 0 && native.return_code != 30 {
				self.telemetry.atomics.decode_errors.fetch_add(1, Ordering::Relaxed);
			}
			return Ok(None);
		}
		let native = self.engine.process_rgb_frame(&image)?;
		Ok(Some(self.process_native_output_for_submitted(submitted, &native)))
	}

	fn telemetry_handle(&self) -> Option<SourceTelemetryHandle> {
		Some(self.telemetry.clone())
	}
}

impl<I> CoreImageMotionFrameSource<I>
where
	I: ImageInputSource + Send + 'static,
{
	fn drain_live_stream_result(&mut self) -> anyhow::Result<Option<UNMotionFrame>> {
		let native = self.engine.poll_latest()?;
		self.update_native_live_stream_stats();
		if native_live_stream_result_pending(&native) {
			self.telemetry.atomics.live_stream_poll_misses.fetch_add(1, Ordering::Relaxed);
			return Ok(None);
		}
		let Some(submitted) = self.take_submitted_image_for_native(&native) else {
			return Ok(None);
		};
		Ok(Some(self.process_native_output_for_submitted(submitted, &native)))
	}

	fn take_submitted_image_for_native(&mut self, native: &NativeMediaPipeOutput) -> Option<SubmittedImageFrame> {
		let Some(result_timestamp_ms) = native.result_timestamp_ms else {
			return self.pending_live_stream_inputs.pop_front();
		};
		let mut selected = None;
		while let Some(candidate) = self.pending_live_stream_inputs.pop_front() {
			let is_match = candidate.timestamp_ms >= result_timestamp_ms;
			selected = Some(candidate);
			if is_match {
				break;
			}
		}
		selected
	}

	fn process_native_output_for_submitted(&mut self, submitted: SubmittedImageFrame, native: &NativeMediaPipeOutput) -> UNMotionFrame {
		self.post_processor.config.input_width = submitted.width;
		self.post_processor.config.input_height = submitted.height;
		let mut frame = if self.post_process_component == "none" {
			self.post_processor
				.native_raw_passthrough_frame(submitted.sequence, submitted.capture_timestamp_ns, native)
		} else {
			let mut frame =
				self.post_processor
					.process_native_output_with_sequence(submitted.sequence, submitted.capture_timestamp_ns, native);
			frame.metadata.notes.push(format!("post_process={}", self.post_process_component));
			frame
		};
		frame.metadata.notes.push(format!(
			"core_unmotion input={} engine={} native_return_code={} image={}x{}",
			self.input_component, self.runtime_engine, native.return_code, submitted.width, submitted.height
		));
		if let Some(source_label) = submitted.source_label.as_deref() {
			frame.metadata.notes.push(format!("input_source={source_label}"));
		}
		self.telemetry.atomics.frames_emitted.fetch_add(1, Ordering::Relaxed);
		frame
	}

	fn update_native_live_stream_stats(&mut self) {
		if !self.live_stream_mode {
			return;
		}
		let Some(stats) = self.engine.live_stream_stats() else {
			return;
		};
		let counters = native_live_stream_frame_counters(self.native_options, stats);
		self.telemetry.atomics.native_callbacks.store(counters.callbacks, Ordering::Relaxed);
		self.telemetry
			.atomics
			.native_submissions
			.store(counters.submissions, Ordering::Relaxed);
		self.telemetry
			.atomics
			.native_submission_errors
			.store(counters.submission_errors, Ordering::Relaxed);
	}
}

#[derive(Clone, Copy, Debug, Default)]
struct NativeLiveStreamFrameCounters {
	submissions: u64,
	submission_errors: u64,
	callbacks: u64,
}

fn native_live_stream_frame_counters(options: NativeMediaPipeOptions, stats: NativeLiveStreamStats) -> NativeLiveStreamFrameCounters {
	if options.enable_holistic != 0 {
		return NativeLiveStreamFrameCounters {
			submissions: stats.holistic_submit_count,
			submission_errors: stats.holistic_submit_error_count,
			callbacks: stats.holistic_callback_count,
		};
	}
	let mut submissions = Vec::new();
	let mut submission_errors = Vec::new();
	let mut callbacks = Vec::new();
	if options.enable_pose != 0 {
		submissions.push(stats.pose_submit_count);
		submission_errors.push(stats.pose_submit_error_count);
		callbacks.push(stats.pose_callback_count);
	}
	if options.enable_hands != 0 {
		submissions.push(stats.hands_submit_count);
		submission_errors.push(stats.hands_submit_error_count);
		callbacks.push(stats.hands_callback_count);
	}
	if options.enable_face != 0 {
		submissions.push(stats.face_submit_count);
		submission_errors.push(stats.face_submit_error_count);
		callbacks.push(stats.face_callback_count);
	}
	if options.enable_gestures != 0 {
		submissions.push(stats.gestures_submit_count);
		submission_errors.push(stats.gestures_submit_error_count);
		callbacks.push(stats.gestures_callback_count);
	}
	NativeLiveStreamFrameCounters {
		submissions: submissions.into_iter().min().unwrap_or(0),
		submission_errors: submission_errors.into_iter().max().unwrap_or(0),
		callbacks: callbacks.into_iter().min().unwrap_or(0),
	}
}

fn open_file_image_source(config: &CoreMotionFrameStreamConfig) -> anyhow::Result<Option<CoreMotionFrameSource>> {
	let path = config
		.input_path
		.as_ref()
		.context("file-image Motion frame stream requires inputPath")?;
	let input = FileImageInputSource::open(file_image_input_config(config, PathBuf::from(path))?)
		.with_context(|| format!("failed to open file-image input {path}"))?;
	Ok(Some(CoreMotionFrameSource::FileImage(open_image_source(
		config,
		input,
		format!("file-image:{}", config.profile_stream_id),
		"MediaPipe Native File Image",
	)?)))
}

fn open_file_video_source(config: &CoreMotionFrameStreamConfig) -> anyhow::Result<Option<CoreMotionFrameSource>> {
	let path = config
		.input_path
		.as_ref()
		.context("file-video Motion frame stream requires inputPath")?;
	let input = FileVideoInputSource::open(file_video_input_config(config, PathBuf::from(path))?)
		.with_context(|| format!("failed to open file-video input {path}"))?;
	Ok(Some(CoreMotionFrameSource::FileVideo(open_image_source(
		config,
		input,
		format!("file-video:{}", config.profile_stream_id),
		"MediaPipe Native File Video",
	)?)))
}

fn open_directshow_source(config: &CoreMotionFrameStreamConfig) -> anyhow::Result<Option<CoreMotionFrameSource>> {
	let (width, height) = capture_dimensions(config);
	info!(
		target: "un_motion_core::unmotion_source",
		device_id = %config.device_id,
		width,
		height,
		input_fps = config.input_fps,
		input_pixel_format = ?config.input_pixel_format,
		profile_stream_id = %config.profile_stream_id,
		"opening DirectShow webcam backend",
	);
	let capture_config = DirectShowCaptureConfig::new(width, height, config.input_fps).with_pixel_format(config.input_pixel_format.clone());
	let backend = DirectShowWebcamBackend::with_capture_config(capture_config);
	let input = DirectShowWebcamImageInputSource::new(backend, config.device_id.clone());
	Ok(Some(CoreMotionFrameSource::DirectShow(open_image_source(
		config,
		input,
		format!("webcam-directshow:{}", config.profile_stream_id),
		"MediaPipe Native DirectShow",
	)?)))
}

fn open_image_source<I>(
	config: &CoreMotionFrameStreamConfig,
	input: I,
	source_id: String,
	display_name: impl Into<String>,
) -> anyhow::Result<CoreImageMotionFrameSource<I>>
where
	I: ImageInputSource + Send + 'static,
{
	configure_native_mediapipe_env_from_workspace();
	let options = native_mediapipe_options(config);
	info!(
		target: "un_motion_core::unmotion_source",
		profile_stream_id = %config.profile_stream_id,
		enable_pose = options.enable_pose,
		enable_hands = options.enable_hands,
		enable_face = options.enable_face,
		enable_holistic = options.enable_holistic,
		running_mode = options.running_mode,
		"opening MediaPipe Native engine",
	);
	let engine = NativeMediaPipeImageEngine::open_default(NativeMediaPipeEngineConfig {
		options,
		include_gestures: false,
	})
	.map_err(|error| {
		warn!(
			target: "un_motion_core::unmotion_source",
			profile_stream_id = %config.profile_stream_id,
			error = %error,
			"NativeMediaPipeImageEngine::open_default failed (check models/*.task availability and UN_MOTION_MEDIAPIPE_* env)",
		);
		error
	})?;
	info!(
		target: "un_motion_core::unmotion_source",
		profile_stream_id = %config.profile_stream_id,
		"MediaPipe Native engine opened",
	);
	let post_processor = MediaPipePostProcessor::new(media_pipe_post_process_config(
		&config.media_pipe_post_process,
		source_id.clone(),
		display_name,
	));
	let telemetry_kind = source_id
		.split_once(':')
		.map(|(prefix, _)| prefix.to_string())
		.unwrap_or_else(|| config.input_component.clone());
	let telemetry = SourceTelemetryHandle::new(telemetry_kind, source_id);

	Ok(CoreImageMotionFrameSource {
		input,
		engine,
		post_processor,
		post_process_component: config.post_process_component.clone(),
		input_component: config.input_component.clone(),
		runtime_engine: config.runtime_engine.clone(),
		live_stream_mode: config.media_pipe_running_mode == "live-stream",
		native_options: options,
		pending_live_stream_inputs: VecDeque::new(),
		telemetry,
	})
}

fn file_image_input_config(config: &CoreMotionFrameStreamConfig, path: PathBuf) -> anyhow::Result<FileImageInputConfig> {
	Ok(FileImageInputConfig {
		source_id: format!("file-image:{}", config.profile_stream_id),
		source_label: path.file_name().map(|name| name.to_string_lossy().to_string()),
		path,
		emission_mode: if config.input_repeat {
			FileImageEmissionMode::RepeatFps(config.input_fps)
		} else {
			FileImageEmissionMode::Once
		},
		resize: config.input_resize.as_ref().map(image_resize_options).transpose()?,
	})
}

fn file_video_input_config(config: &CoreMotionFrameStreamConfig, path: PathBuf) -> anyhow::Result<FileVideoInputConfig> {
	let (width, height) = capture_dimensions(config);
	let mut video = FileVideoInputConfig::new(path.clone(), width, height, config.input_fps);
	video.source_id = format!("file-video:{}", config.profile_stream_id);
	video.source_label = path.file_name().map(|name| name.to_string_lossy().to_string());
	video.repeat = config.input_repeat;
	if let Some(ffmpeg_path) = config.input_ffmpeg_path.as_deref().filter(|path| !path.trim().is_empty()) {
		video.ffmpeg_path = PathBuf::from(ffmpeg_path);
	}
	Ok(video)
}

fn image_resize_options(config: &CoreUnmotionResizeConfig) -> anyhow::Result<ImageResizeOptions> {
	Ok(ImageResizeOptions {
		preserve_aspect_ratio: config.preserve_aspect_ratio,
		reference_axis: match config.axis.as_str() {
			"height" => ResizeAxis::Height,
			_ => ResizeAxis::Width,
		},
		reference_length: config.reference,
		output_width: config.width,
		output_height: config.height,
		pad_color: RgbaColor::parse_rrggbbaa(&config.pad_color)?,
	})
}

fn capture_dimensions(config: &CoreMotionFrameStreamConfig) -> (u32, u32) {
	(config.input_width.unwrap_or(640), config.input_height.unwrap_or(480))
}

fn native_mediapipe_options(config: &CoreMotionFrameStreamConfig) -> NativeMediaPipeOptions {
	let mut options = match config.media_pipe_running_mode.as_str() {
		"image" => NativeMediaPipeOptions {
			running_mode: RUNNING_MODE_IMAGE,
			..NativeMediaPipeOptions::desktop_video()
		},
		"video" => NativeMediaPipeOptions::desktop_video(),
		_ => NativeMediaPipeOptions::desktop_live_stream(),
	};

	let post_process = &config.media_pipe_post_process;
	let needs_pose = post_process.head_enabled
		|| post_process.arms_ik_enabled
		|| post_process.torso_enabled
		|| post_process.legs_enabled
		|| post_process.feet_enabled;
	let needs_face = post_process.head_enabled || post_process.face_enabled;
	let needs_hands = post_process.hands_enabled || post_process.arms_ik_enabled;
	if config.media_pipe_holistic_enabled {
		options.enable_pose = u8::from(needs_pose);
		options.enable_hands = u8::from(needs_hands);
		options.enable_face = u8::from(needs_face);
		options.enable_gestures = 0;
		options.enable_holistic = 1;
	} else {
		options.enable_pose = u8::from(needs_pose);
		options.enable_hands = u8::from(needs_hands);
		options.enable_face = u8::from(needs_face);
		options.enable_holistic = 0;
	}
	apply_native_mediapipe_profile_options(config, &mut options);
	apply_native_mediapipe_env_overrides(&mut options);
	options
}

fn apply_native_mediapipe_profile_options(config: &CoreMotionFrameStreamConfig, options: &mut NativeMediaPipeOptions) {
	if let Some(delegate) = config.media_pipe_delegate.as_deref() {
		options.delegate = native_mediapipe_delegate_from_str(delegate, options.delegate);
	}
	if let Some(num_threads) = config.media_pipe_num_threads {
		options.delegate_num_threads = num_threads.max(1);
	}
	options.holistic_flow_limiter_enabled = u8::from(config.media_pipe_holistic_flow_limiter_enabled);
	options.holistic_flow_limiter_max_in_flight = config.media_pipe_holistic_flow_limiter_max_in_flight.max(1);
	options.holistic_flow_limiter_max_in_queue = config.media_pipe_holistic_flow_limiter_max_in_queue;
}

fn apply_native_mediapipe_env_overrides(options: &mut NativeMediaPipeOptions) {
	if let Ok(delegate) = std::env::var("UN_MOTION_MEDIAPIPE_DELEGATE") {
		options.delegate = native_mediapipe_delegate_from_str(&delegate, options.delegate);
	}
	if let Ok(value) = std::env::var("UN_MOTION_MEDIAPIPE_NUM_THREADS")
		&& let Ok(num_threads) = value.trim().parse::<u32>()
	{
		options.delegate_num_threads = num_threads;
	}
	if let Ok(value) = std::env::var("UN_MOTION_MEDIAPIPE_HOLISTIC_FLOW_LIMITER") {
		options.holistic_flow_limiter_enabled =
			u8::from(!matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"));
	}
	if let Ok(value) = std::env::var("UN_MOTION_MEDIAPIPE_HOLISTIC_FLOW_MAX_IN_FLIGHT")
		&& let Ok(max_in_flight) = value.trim().parse::<u32>()
	{
		options.holistic_flow_limiter_max_in_flight = max_in_flight.max(1);
	}
	if let Ok(value) = std::env::var("UN_MOTION_MEDIAPIPE_HOLISTIC_FLOW_MAX_IN_QUEUE")
		&& let Ok(max_in_queue) = value.trim().parse::<u32>()
	{
		options.holistic_flow_limiter_max_in_queue = max_in_queue;
	}
}

fn native_mediapipe_delegate_from_str(delegate: &str, current: u8) -> u8 {
	match delegate.trim().to_ascii_lowercase().as_str() {
		"xnnpack" => DELEGATE_XNNPACK,
		"cpu" | "tflite" => DELEGATE_CPU,
		#[cfg(not(windows))]
		"gpu" => DELEGATE_GPU,
		#[cfg(windows)]
		"gpu" => {
			warn!("Native MediaPipe GPU delegate is not available in Windows builds; keeping current delegate");
			current
		}
		_ => current,
	}
}

fn media_pipe_post_process_config(
	post_process: &CoreMediaPipePostProcessSettings,
	source_id: impl Into<String>,
	display_name: impl Into<String>,
) -> MediaPipePostProcessConfig {
	MediaPipePostProcessConfig {
		head_enabled: post_process.head_enabled,
		face_enabled: post_process.face_enabled,
		hands_enabled: post_process.hands_enabled,
		arms_ik_enabled: post_process.arms_ik_enabled,
		torso_enabled: post_process.torso_enabled,
		legs_enabled: post_process.legs_enabled,
		feet_enabled: post_process.feet_enabled,
		include_fingers: post_process.hands_enabled,
		min_landmark_confidence: post_process.min_landmark_confidence,
		camera_diagonal_view_angle_deg: post_process.camera_diagonal_view_angle_deg,
		eye_open_bias: post_process.eye_open_bias,
		mirror_mode: post_process.mirror_mode.clone(),
		source_id: source_id.into(),
		display_name: display_name.into(),
		face_pose_model: post_process.face_pose_model.as_ref().map(|model| FacePoseModelConfig {
			enabled: model.enabled,
			neutral_nose_drop_eye_mouth: model.neutral_nose_drop_eye_mouth,
		}),
		rules: MediaPipePostProcessRules {
			hold_lost_landmarks: post_process.post_process_rules.hold_lost_landmarks,
			ease_recovery: post_process.post_process_rules.ease_recovery,
			limit_rotation_jumps: post_process.post_process_rules.limit_rotation_jumps,
			head_source_switch_blend: post_process.post_process_rules.head_source_switch_blend,
			lost_signal_behavior: post_process.post_process_rules.lost_signal_behavior.clone(),
			lost_signal_rest_pose_blend: post_process.post_process_rules.lost_signal_rest_pose_blend,
			lost_signal_hold_seconds: post_process.post_process_rules.lost_signal_hold_seconds,
			lost_signal_head_behavior: post_process.post_process_rules.lost_signal_head_behavior.clone(),
			lost_signal_head_rest_pose_blend: post_process.post_process_rules.lost_signal_head_rest_pose_blend,
			lost_signal_head_hold_seconds: post_process.post_process_rules.lost_signal_head_hold_seconds,
			lost_signal_hands_behavior: post_process.post_process_rules.lost_signal_hands_behavior.clone(),
			lost_signal_hands_rest_pose_blend: post_process.post_process_rules.lost_signal_hands_rest_pose_blend,
			lost_signal_hands_hold_seconds: post_process.post_process_rules.lost_signal_hands_hold_seconds,
			lost_signal_arms_behavior: post_process.post_process_rules.lost_signal_arms_behavior.clone(),
			lost_signal_arms_rest_pose_blend: post_process.post_process_rules.lost_signal_arms_rest_pose_blend,
			lost_signal_arms_hold_seconds: post_process.post_process_rules.lost_signal_arms_hold_seconds,
			lost_signal_recovery_seconds: post_process.post_process_rules.lost_signal_recovery_seconds,
			head_from_pose: post_process.post_process_rules.head_from_pose,
			head_from_face_matrix: post_process.post_process_rules.head_from_face_matrix,
			head_reconcile: post_process.post_process_rules.head_reconcile,
			neutral_eye_fallback: post_process.post_process_rules.neutral_eye_fallback,
			hand_camera_target: post_process.post_process_rules.hand_camera_target,
			hand_orientation: post_process.post_process_rules.hand_orientation,
			finger_derived: post_process.post_process_rules.finger_derived,
			arm_from_pose: post_process.post_process_rules.arm_from_pose,
			arm_ik_from_hands: post_process.post_process_rules.arm_ik_from_hands,
			crossed_hand_heuristic: post_process.post_process_rules.crossed_hand_heuristic,
			coordinate_correction: post_process.post_process_rules.coordinate_correction,
			final_clamp: post_process.post_process_rules.final_clamp,
		},
		..MediaPipePostProcessConfig::default()
	}
}

fn native_live_stream_result_pending(native: &NativeMediaPipeOutput) -> bool {
	native.result_timestamp_ms.is_none() || (native.return_code == 30 && !native_output_has_landmarks(native))
}

fn native_output_has_landmarks(native: &NativeMediaPipeOutput) -> bool {
	native.pose.landmark_count > 0
		|| native.pose.world_landmark_count > 0
		|| native.hands.hand_count > 0
		|| native.face.landmark_count > 0
		|| native.holistic.pose.landmark_count > 0
		|| native.holistic.pose.world_landmark_count > 0
		|| native.holistic.left_hand.landmark_count > 0
		|| native.holistic.left_hand.world_landmark_count > 0
		|| native.holistic.right_hand.landmark_count > 0
		|| native.holistic.right_hand.world_landmark_count > 0
		|| native.holistic.face.landmark_count > 0
}

fn configure_native_mediapipe_env_from_workspace() {
	// `current_dir` だけだと Capturer の cwd が workspace root と一致しない経路
	// (Supervisor からの起動や release zip の隣で起動する経路) で models/*.task を見失う。
	// `current_exe()` の ancestors と `CARGO_MANIFEST_DIR` の祖先からも `models/` を探す。
	let mut roots: Vec<std::path::PathBuf> = Vec::new();
	if let Ok(current_dir) = std::env::current_dir() {
		roots.push(current_dir);
	}
	if let Ok(current_exe) = std::env::current_exe() {
		for ancestor in current_exe.ancestors() {
			roots.push(ancestor.to_path_buf());
		}
	}
	// `CARGO_MANIFEST_DIR` は un-motion-core crate dir なので、workspace root はその祖先。
	for ancestor in Path::new(env!("CARGO_MANIFEST_DIR")).ancestors() {
		roots.push(ancestor.to_path_buf());
	}

	let mut found_pose = false;
	let mut found_hand = false;
	let mut found_face = false;
	let mut found_holistic = false;
	let mut found_root: Option<std::path::PathBuf> = None;
	for root in &roots {
		let pose_model = root.join("models/pose_landmarker_lite.task");
		let hand_model = root.join("models/hand_landmarker.task");
		let face_model = root.join("models/face_landmarker.task");
		let holistic_model = root.join("models/holistic_landmarker.task");
		if pose_model.exists() && !found_pose {
			unsafe {
				std::env::set_var("UN_MOTION_MEDIAPIPE_MODEL", &pose_model);
			}
			found_pose = true;
			if found_root.is_none() {
				found_root = Some(root.clone());
			}
		}
		if hand_model.exists() && !found_hand {
			unsafe {
				std::env::set_var("UN_MOTION_MEDIAPIPE_HAND_MODEL", &hand_model);
			}
			found_hand = true;
		}
		if face_model.exists() && !found_face {
			unsafe {
				std::env::set_var("UN_MOTION_MEDIAPIPE_FACE_MODEL", &face_model);
			}
			found_face = true;
		}
		if holistic_model.exists() && !found_holistic {
			unsafe {
				std::env::set_var("UN_MOTION_MEDIAPIPE_HOLISTIC_MODEL", &holistic_model);
			}
			found_holistic = true;
		}
		if found_pose && found_hand && found_face && found_holistic {
			break;
		}
	}
	unsafe {
		std::env::set_var("UN_MOTION_MEDIAPIPE_QUIET", "1");
		std::env::set_var("UN_MOTION_MEDIAPIPE_LOG_LEVEL", "3");
		std::env::set_var("TF_CPP_MIN_LOG_LEVEL", "3");
		std::env::set_var("GLOG_minloglevel", "2");
	}
	if found_pose {
		info!(
			target: "un_motion_core::unmotion_source",
			pose = found_pose,
			hand = found_hand,
			face = found_face,
			holistic = found_holistic,
			root = ?found_root,
			"MediaPipe Native model files resolved",
		);
	} else {
		warn!(
			target: "un_motion_core::unmotion_source",
			searched_roots = ?roots,
			"MediaPipe Native model file pose_landmarker_lite.task not found; engine open will likely fail",
		);
	}
}
