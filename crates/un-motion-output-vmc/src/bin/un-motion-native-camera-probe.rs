use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
use serde::Serialize;
use un_motion_input_webcam_directshow::{
	DirectShowCaptureConfig, DirectShowWebcamBackend, WebcamCaptureBackend, list_directshow_capture_formats,
};
use un_motion_mediapipe_native::{
	NativeMediaPipeOptions, NativeMediaPipeRuntime, RUNNING_MODE_IMAGE, RgbImageRef, resolve_media_pipe_root, resolve_native_dir,
};

#[cfg(windows)]
use windows::Win32::Media::DirectShow::ICreateDevEnum;
#[cfg(windows)]
use windows::Win32::Media::MediaFoundation::{
	CLSID_SystemDeviceEnum, CLSID_VideoInputDeviceCategory, IMFActivate, IMFAttributes, MF_API_VERSION,
	MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
	MFCreateAttributes, MFEnumDeviceSources, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
};
#[cfg(windows)]
use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
#[cfg(windows)]
use windows::Win32::System::Com::{
	CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
#[cfg(windows)]
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VariantClear};
#[cfg(windows)]
use windows::core::{GUID, PWSTR, w};

const RGB_DECODABLE_FORMATS: &[FrameFormat] = &[
	FrameFormat::NV12,
	FrameFormat::MJPEG,
	FrameFormat::YUYV,
	FrameFormat::RAWRGB,
	FrameFormat::RAWBGR,
	FrameFormat::GRAY,
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeSummary {
	device: String,
	backend: String,
	camera_index: String,
	camera_name: String,
	requested: String,
	actual: String,
	frame_image: String,
	dll: String,
	return_code: i32,
	width: u32,
	height: u32,
	pose_landmarks: u32,
	pose_world_landmarks: u32,
	hand_count: u32,
	face_landmarks: u32,
	face_blendshapes: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CameraTimingSummary {
	device: String,
	backend: String,
	camera_index: String,
	camera_name: String,
	requested: String,
	actual: String,
	frames: u32,
	elapsed_ms: f32,
	avg_fps: f32,
	min_fps: f32,
	max_fps: f32,
	p95_interval_ms: f32,
	max_interval_ms: f32,
	backend_consumer_fps: Option<f32>,
	backend_source_fps: Option<f32>,
}

struct Args {
	list: bool,
	list_formats: bool,
	backend: CameraBackend,
	device: Option<String>,
	resolution: (u32, u32),
	fps: u32,
	timing_seconds: Option<f32>,
	media_pipe_root: PathBuf,
	dll: Option<PathBuf>,
	output_image: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CameraBackend {
	Nokhwa,
	DirectShow,
}

fn main() -> anyhow::Result<()> {
	let args = Args::parse()?;
	if args.list {
		print_camera_list()?;
		return Ok(());
	}
	if args.list_formats {
		print_camera_formats(&args)?;
		return Ok(());
	}
	if let Some(seconds) = args.timing_seconds {
		let summary = measure_camera_timing(&args, seconds)?;
		println!("{}", serde_json::to_string_pretty(&summary)?);
		return Ok(());
	}
	let captured = capture_camera_frame(&args)?;
	let image_path = args.output_image.unwrap_or_else(default_output_image_path);
	save_rgb_png(&image_path, &captured.rgb, captured.width, captured.height)?;

	let original_cwd = env::current_dir().context("failed to read current directory")?;
	let media_pipe_input = absolutize(&original_cwd, &args.media_pipe_root);
	let media_pipe_root = resolve_media_pipe_root(&media_pipe_input)?;
	let native_dir = resolve_native_dir(&media_pipe_input, &media_pipe_root);
	let dll_path = args.dll.unwrap_or_else(|| native_dir.join("un-motion-mediapipe.dll"));
	let dll_path = absolutize(&original_cwd, &dll_path)
		.canonicalize()
		.with_context(|| format!("failed to resolve DLL {}", dll_path.display()))?;
	configure_native_env(&media_pipe_root);

	let mut native = NativeMediaPipeRuntime::open_with_options(
		&dll_path,
		NativeMediaPipeOptions {
			running_mode: RUNNING_MODE_IMAGE,
			..NativeMediaPipeOptions::desktop_video()
		},
	)?;
	let output = native.process_rgb_at(
		RgbImageRef {
			bytes: &captured.rgb,
			width: captured.width,
			height: captured.height,
			stride: captured.width.saturating_mul(3),
		},
		0,
	)?;

	let summary = ProbeSummary {
		device: args.device.unwrap_or_else(|| "first-camera".to_string()),
		backend: captured.backend,
		camera_index: captured.camera_index,
		camera_name: captured.camera_name,
		requested: captured.requested,
		actual: captured.actual,
		frame_image: image_path.display().to_string(),
		dll: dll_path.display().to_string(),
		return_code: output.return_code,
		width: captured.width,
		height: captured.height,
		pose_landmarks: output.pose.landmark_count,
		pose_world_landmarks: output.pose.world_landmark_count,
		hand_count: output.hands.hand_count,
		face_landmarks: output.face.landmark_count,
		face_blendshapes: output.face.blendshape_count,
	};
	println!("{}", serde_json::to_string_pretty(&summary)?);
	Ok(())
}

impl Args {
	fn parse() -> anyhow::Result<Self> {
		let mut device = None;
		let mut backend = CameraBackend::Nokhwa;
		let mut list = false;
		let mut list_formats = false;
		let mut resolution = (1280_u32, 720_u32);
		let mut fps = 30_u32;
		let mut timing_seconds = None;
		let mut media_pipe_root = PathBuf::from(".");
		let mut dll = None;
		let mut output_image = None;
		let mut args = env::args().skip(1);
		while let Some(arg) = args.next() {
			match arg.as_str() {
				"--list" => list = true,
				"--list-formats" => list_formats = true,
				"--device" => device = args.next(),
				"--backend" => {
					let value = args.next().context("missing --backend value")?;
					backend = match value.as_str() {
						"nokhwa" => CameraBackend::Nokhwa,
						"directshow" | "dshow" => CameraBackend::DirectShow,
						_ => bail!("invalid --backend {value}; expected nokhwa or directshow"),
					};
				}
				"--resolution" => {
					let value = args.next().context("missing --resolution value")?;
					resolution = parse_resolution(&value)?;
				}
				"--fps" => {
					let value = args.next().context("missing --fps value")?;
					fps = value.parse().with_context(|| format!("invalid --fps {value}"))?;
				}
				"--timing-seconds" => {
					let value = args.next().context("missing --timing-seconds value")?;
					timing_seconds = Some(
						value
							.parse::<f32>()
							.with_context(|| format!("invalid --timing-seconds {value}"))?
							.max(0.1),
					);
				}
				"--media-pipe-root" => media_pipe_root = args.next().map(PathBuf::from).context("missing --media-pipe-root value")?,
				"--dll" => dll = args.next().map(PathBuf::from),
				"--output-image" => output_image = args.next().map(PathBuf::from),
				"--help" | "-h" => {
					print_usage();
					std::process::exit(0);
				}
				_ => bail!("unexpected argument: {arg}"),
			}
		}
		Ok(Self {
			list,
			list_formats,
			backend,
			device,
			resolution,
			fps,
			timing_seconds,
			media_pipe_root,
			dll,
			output_image,
		})
	}
}

fn print_usage() {
	eprintln!(
		"usage: un-motion-native-camera-probe [--list] [--list-formats] [--backend nokhwa|directshow] [--device \"OBS Virtual Camera\"] [--resolution 1280x720] [--fps 30] [--timing-seconds 5] [--media-pipe-root .] [--dll path] [--output-image path]"
	);
}

struct CapturedFrame {
	backend: String,
	camera_index: String,
	camera_name: String,
	requested: String,
	actual: String,
	rgb: Vec<u8>,
	width: u32,
	height: u32,
}

fn capture_camera_frame(args: &Args) -> anyhow::Result<CapturedFrame> {
	match args.backend {
		CameraBackend::Nokhwa => capture_nokhwa_frame(args),
		CameraBackend::DirectShow => capture_directshow_frame(args),
	}
}

fn capture_nokhwa_frame(args: &Args) -> anyhow::Result<CapturedFrame> {
	let camera_info = resolve_camera(args.device.as_deref())?;
	let requested = CameraFormat::new_from(args.resolution.0, args.resolution.1, FrameFormat::MJPEG, args.fps);
	let request = RequestedFormat::with_formats(RequestedFormatType::Closest(requested), RGB_DECODABLE_FORMATS);
	let mut camera = nokhwa::Camera::new(camera_info.index().clone(), request)?;
	camera.open_stream()?;
	let frame = camera.frame()?;
	let actual = camera.camera_format();
	let rgb = frame.decode_image::<nokhwa::pixel_format::RgbFormat>()?;
	Ok(CapturedFrame {
		backend: "nokhwa".to_string(),
		camera_index: format!("{:?}", camera_info.index()),
		camera_name: camera_info.human_name(),
		requested: format!("{}x{}@{} {:?}", args.resolution.0, args.resolution.1, args.fps, FrameFormat::MJPEG),
		actual: format!(
			"{}x{}@{} {:?}",
			actual.width(),
			actual.height(),
			actual.frame_rate(),
			actual.format()
		),
		rgb: rgb.as_raw().clone(),
		width: rgb.width(),
		height: rgb.height(),
	})
}

fn capture_directshow_frame(args: &Args) -> anyhow::Result<CapturedFrame> {
	let mut backend = DirectShowWebcamBackend::default();
	let devices = backend.list_devices()?;
	let selected = resolve_directshow_device(&devices, args.device.as_deref())?;
	let config = DirectShowCaptureConfig::new(args.resolution.0, args.resolution.1, args.fps);
	backend.set_capture_config(config.clone());
	let frame = backend
		.capture_next_image(&selected.id)?
		.context("DirectShow camera did not produce a frame before timeout")?;
	let actual = backend.active_format_label().unwrap_or_else(|| {
		format!(
			"{}x{} {:?} stride={}",
			frame.width, frame.height, frame.pixel_format, frame.stride_bytes
		)
	});
	Ok(CapturedFrame {
		backend: "directshow".to_string(),
		camera_index: selected.id,
		camera_name: selected.name,
		requested: config.requested_label(),
		actual,
		rgb: frame.data,
		width: frame.width,
		height: frame.height,
	})
}

fn measure_camera_timing(args: &Args, seconds: f32) -> anyhow::Result<CameraTimingSummary> {
	match args.backend {
		CameraBackend::DirectShow => measure_directshow_timing(args, seconds),
		CameraBackend::Nokhwa => bail!("--timing-seconds currently supports --backend directshow"),
	}
}

fn measure_directshow_timing(args: &Args, seconds: f32) -> anyhow::Result<CameraTimingSummary> {
	let mut backend = DirectShowWebcamBackend::default();
	let devices = backend.list_devices()?;
	let selected = resolve_directshow_device(&devices, args.device.as_deref())?;
	let config = DirectShowCaptureConfig::new(args.resolution.0, args.resolution.1, args.fps);
	backend.set_capture_config(config.clone());
	let started = Instant::now();
	let deadline = Duration::from_secs_f32(seconds.max(0.1));
	let mut previous = Instant::now();
	let mut intervals = Vec::new();
	let mut frames = 0_u32;
	let mut actual = "-".to_string();
	while started.elapsed() < deadline {
		let Some(_frame) = backend.capture_next_image(&selected.id)? else {
			continue;
		};
		actual = backend.active_format_label().unwrap_or_else(|| "-".to_string());
		let now = Instant::now();
		if frames > 0 {
			let interval_ms = now.duration_since(previous).as_secs_f32() * 1000.0;
			if interval_ms > 0.0 {
				intervals.push(interval_ms);
			}
		}
		previous = now;
		frames = frames.saturating_add(1);
	}
	let elapsed_ms = started.elapsed().as_secs_f32() * 1000.0;
	let avg_fps = if elapsed_ms > 0.0 {
		frames as f32 * 1000.0 / elapsed_ms
	} else {
		0.0
	};
	let min_fps = intervals
		.iter()
		.copied()
		.reduce(f32::max)
		.map(|interval| 1000.0 / interval)
		.unwrap_or(0.0);
	let max_fps = intervals
		.iter()
		.copied()
		.filter(|interval| *interval > 0.0)
		.reduce(f32::min)
		.map(|interval| 1000.0 / interval)
		.unwrap_or(0.0);
	let mut sorted = intervals.clone();
	sorted.sort_by(|a, b| a.total_cmp(b));
	let p95_interval_ms = percentile(&sorted, 0.95);
	let max_interval_ms = sorted.last().copied().unwrap_or(0.0);
	Ok(CameraTimingSummary {
		device: args.device.clone().unwrap_or_else(|| "first-camera".to_string()),
		backend: "directshow".to_string(),
		camera_index: selected.id,
		camera_name: selected.name,
		requested: config.requested_label(),
		actual,
		frames,
		elapsed_ms,
		avg_fps,
		min_fps,
		max_fps,
		p95_interval_ms,
		max_interval_ms,
		backend_consumer_fps: backend.active_observed_fps(),
		backend_source_fps: backend.active_fps().or_else(|| backend.active_observed_source_fps()),
	})
}

fn resolve_directshow_device(
	devices: &[un_motion_input_webcam_directshow::WebcamDeviceInfo],
	device: Option<&str>,
) -> anyhow::Result<un_motion_input_webcam_directshow::WebcamDeviceInfo> {
	if devices.is_empty() {
		bail!("no DirectShow cameras found");
	}
	let Some(device) = device else {
		return Ok(devices[0].clone());
	};
	if let Some(found) = devices.iter().find(|candidate| candidate.id == device) {
		return Ok(found.clone());
	}
	let needle = comparable_label(device);
	if let Some(found) = devices.iter().find(|candidate| {
		let name = comparable_label(&candidate.name);
		!needle.is_empty() && (name == needle || name.contains(&needle) || needle.contains(&name))
	}) {
		return Ok(found.clone());
	}
	bail!(
		"DirectShow camera not found: {device}; available cameras: {}",
		devices
			.iter()
			.map(|candidate| format!("{}:{}", candidate.id, candidate.name))
			.collect::<Vec<_>>()
			.join(", ")
	)
}

fn print_camera_list() -> anyhow::Result<()> {
	println!("nokhwa:");
	let cameras = nokhwa::query(ApiBackend::Auto).context("failed to query cameras")?;
	for camera in cameras {
		println!("{:?}\t{}", camera.index(), camera.human_name());
	}
	#[cfg(windows)]
	{
		println!("media-foundation:");
		let _guard = MfSessionGuard::new()?;
		for (index, activate) in mf_video_capture_activates()?.iter().enumerate() {
			let name = mf_allocated_string(activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_else(|_| "<unknown>".to_string());
			println!("Index({index})\t{name}");
		}
		println!("directshow:");
		for (index, name) in directshow_video_capture_names()?.iter().enumerate() {
			println!("Index({index})\t{name}");
		}
	}
	Ok(())
}

fn print_camera_formats(args: &Args) -> anyhow::Result<()> {
	match args.backend {
		CameraBackend::DirectShow => {
			let device = args.device.as_deref().unwrap_or_default();
			let formats = list_directshow_capture_formats(device)?;
			for format in formats {
				println!("{}", format.native_label());
			}
			Ok(())
		}
		CameraBackend::Nokhwa => bail!("--list-formats currently supports --backend directshow"),
	}
}

#[cfg(windows)]
struct MfSessionGuard {
	co_initialized: bool,
}

#[cfg(windows)]
impl MfSessionGuard {
	fn new() -> anyhow::Result<Self> {
		let co_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok() };
		unsafe { MFStartup(MF_API_VERSION, MFSTARTUP_NOSOCKET)? };
		Ok(Self { co_initialized })
	}
}

#[cfg(windows)]
impl Drop for MfSessionGuard {
	fn drop(&mut self) {
		unsafe {
			let _ = MFShutdown();
			if self.co_initialized {
				CoUninitialize();
			}
		}
	}
}

#[cfg(windows)]
fn mf_allocated_string(activate: &IMFActivate, key: &GUID) -> anyhow::Result<String> {
	let mut value = PWSTR::null();
	let mut len = 0;
	unsafe { activate.GetAllocatedString(key, &mut value, &mut len)? };
	if value.is_null() {
		bail!("Media Foundation string attribute was null");
	}
	let text = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(value.0, len as usize)) };
	unsafe { CoTaskMemFree(Some(value.0.cast())) };
	Ok(text)
}

#[cfg(windows)]
fn mf_video_capture_activates() -> anyhow::Result<Vec<IMFActivate>> {
	let mut attrs: Option<IMFAttributes> = None;
	unsafe { MFCreateAttributes(&mut attrs, 1)? };
	let attrs = attrs.context("MFCreateAttributes returned no attributes")?;
	unsafe { attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)? };

	let mut activates_ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
	let mut count = 0;
	unsafe { MFEnumDeviceSources(&attrs, &mut activates_ptr, &mut count)? };
	if activates_ptr.is_null() || count == 0 {
		return Ok(Vec::new());
	}
	let activates = unsafe { std::slice::from_raw_parts(activates_ptr, count as usize) }
		.iter()
		.filter_map(Clone::clone)
		.collect::<Vec<_>>();
	unsafe { CoTaskMemFree(Some(activates_ptr.cast())) };
	Ok(activates)
}

#[cfg(windows)]
fn directshow_video_capture_names() -> anyhow::Result<Vec<String>> {
	let dev_enum: ICreateDevEnum = unsafe { CoCreateInstance(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER)? };
	let mut enum_moniker = None;
	unsafe { dev_enum.CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut enum_moniker, 0)? };
	let Some(enum_moniker) = enum_moniker else {
		return Ok(Vec::new());
	};
	let mut names = Vec::new();
	loop {
		let mut monikers = [None];
		let mut fetched = 0;
		let hr = unsafe { enum_moniker.Next(&mut monikers, Some(&mut fetched)) };
		if fetched == 0 || hr.is_err() {
			break;
		}
		let Some(moniker) = monikers[0].as_ref() else {
			break;
		};
		let name = directshow_moniker_friendly_name(moniker).unwrap_or_else(|| {
			unsafe { moniker.GetDisplayName(None, None) }
				.map(|value| {
					if value.is_null() {
						"<unknown>".to_string()
					} else {
						unsafe {
							let mut len = 0usize;
							while *value.0.add(len) != 0 {
								len += 1;
							}
							let text = String::from_utf16_lossy(std::slice::from_raw_parts(value.0, len));
							CoTaskMemFree(Some(value.0.cast()));
							text
						}
					}
				})
				.unwrap_or_else(|_| "<unknown>".to_string())
		});
		names.push(name);
	}
	Ok(names)
}

#[cfg(windows)]
fn directshow_moniker_friendly_name(moniker: &windows::Win32::System::Com::IMoniker) -> Option<String> {
	let bag: IPropertyBag = unsafe { moniker.BindToStorage(None, None).ok()? };
	let mut value = VARIANT::default();
	let read_result = unsafe { bag.Read(w!("FriendlyName"), &mut value, None) };
	if read_result.is_err() {
		return None;
	}
	let variant = unsafe { &value.Anonymous.Anonymous };
	let text = if variant.vt == VT_BSTR {
		let bstr = unsafe { &variant.Anonymous.bstrVal };
		String::try_from(&**bstr).ok()
	} else {
		None
	};
	unsafe {
		let _ = VariantClear(&mut value);
	}
	text
}

fn resolve_camera(device: Option<&str>) -> anyhow::Result<nokhwa::utils::CameraInfo> {
	let cameras = nokhwa::query(ApiBackend::Auto).context("failed to query cameras")?;
	if cameras.is_empty() {
		bail!("no cameras found");
	}
	let Some(device) = device else {
		return Ok(cameras[0].clone());
	};
	if let Ok(index) = device.parse::<u32>() {
		if let Some(camera) = cameras
			.iter()
			.find(|camera| matches!(camera.index(), CameraIndex::Index(value) if *value == index))
		{
			return Ok(camera.clone());
		}
	}
	let needle = comparable_label(device);
	if let Some(camera) = cameras.iter().find(|camera| {
		let name = comparable_label(&camera.human_name());
		!needle.is_empty() && (name == needle || name.contains(&needle) || needle.contains(&name))
	}) {
		return Ok(camera.clone());
	}
	if device.trim().is_empty() && cameras.len() == 1 {
		return Ok(cameras[0].clone());
	}
	bail!("camera not found: {device}; available cameras: {}", camera_list_for_error())
}

fn camera_list_for_error() -> String {
	nokhwa::query(ApiBackend::Auto)
		.map(|cameras| {
			cameras
				.iter()
				.map(|camera| format!("{:?}:{}", camera.index(), camera.human_name()))
				.collect::<Vec<_>>()
				.join(", ")
		})
		.unwrap_or_else(|error| format!("query failed: {error}"))
}

fn comparable_label(value: &str) -> String {
	value
		.chars()
		.filter(|ch| ch.is_ascii_alphanumeric())
		.flat_map(char::to_lowercase)
		.collect()
}

fn parse_resolution(value: &str) -> anyhow::Result<(u32, u32)> {
	let Some((width, height)) = value.split_once('x').or_else(|| value.split_once('X')) else {
		bail!("invalid resolution {value}; expected WIDTHxHEIGHT");
	};
	let width = width.parse().with_context(|| format!("invalid resolution width {width}"))?;
	let height = height.parse().with_context(|| format!("invalid resolution height {height}"))?;
	Ok((width, height))
}

fn percentile(sorted_values: &[f32], percentile: f32) -> f32 {
	if sorted_values.is_empty() {
		return 0.0;
	}
	let index = ((sorted_values.len() - 1) as f32 * percentile.clamp(0.0, 1.0)).round() as usize;
	sorted_values[index]
}

fn default_output_image_path() -> PathBuf {
	PathBuf::from("target")
		.join("vmc-captures")
		.join(format!("native-camera-input-{}.png", now_unix_ms()))
}

fn save_rgb_png(path: &Path, bytes: &[u8], width: u32, height: u32) -> anyhow::Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	image::save_buffer_with_format(path, bytes, width, height, image::ColorType::Rgb8, image::ImageFormat::Png)
		.with_context(|| format!("failed to write {}", path.display()))
}

fn configure_native_env(media_pipe_root: &Path) {
	unsafe {
		env::set_var(
			"UN_MOTION_MEDIAPIPE_MODEL",
			media_pipe_root.join("models/pose_landmarker_lite.task"),
		);
		env::set_var(
			"UN_MOTION_MEDIAPIPE_HAND_MODEL",
			media_pipe_root.join("models/hand_landmarker.task"),
		);
		env::set_var(
			"UN_MOTION_MEDIAPIPE_FACE_MODEL",
			media_pipe_root.join("models/face_landmarker.task"),
		);
		env::set_var("UN_MOTION_MEDIAPIPE_QUIET", "1");
		env::set_var("UN_MOTION_MEDIAPIPE_LOG_LEVEL", "3");
		env::set_var("TF_CPP_MIN_LOG_LEVEL", "3");
		env::set_var("GLOG_minloglevel", "2");
	}
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
	if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

fn now_unix_ms() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_millis()
		.min(u128::from(u64::MAX)) as u64
}
