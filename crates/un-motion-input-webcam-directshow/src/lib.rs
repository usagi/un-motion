#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use un_motion_interfaces::{ImageFrame, ImageFrameMetadata, ImageInputSource, PixelFormat, TimestampBasis};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebcamDeviceInfo {
	pub id: String,
	pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectShowCaptureFormatInfo {
	pub width: u32,
	pub height: u32,
	pub fps: Option<u32>,
	pub pixel_format: String,
}

impl DirectShowCaptureFormatInfo {
	pub fn resolution_label(&self) -> String {
		format!("{}x{}", self.width, self.height)
	}

	pub fn native_label(&self) -> String {
		match self.fps {
			Some(fps) => format!("{}x{}@{} {}", self.width, self.height, fps, self.pixel_format),
			None => format!("{}x{} {}", self.width, self.height, self.pixel_format),
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebcamDiagnosticReport {
	pub backend: &'static str,
	pub device_count: usize,
	pub devices: Vec<WebcamDeviceInfo>,
}

impl WebcamDiagnosticReport {
	pub fn to_text_lines(&self) -> Vec<String> {
		let mut lines = vec![format!("{} webcam devices: {}", self.backend, self.device_count)];
		for device in &self.devices {
			lines.push(format!("- {} ({})", device.name, device.id));
		}
		lines
	}
}

pub trait WebcamCaptureBackend {
	fn list_devices(&mut self) -> anyhow::Result<Vec<WebcamDeviceInfo>>;
	fn capture_next_image(&mut self, device_id: &str) -> anyhow::Result<Option<ImageFrame>>;

	fn observed_source_fps(&self) -> Option<f32> {
		None
	}
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectShowCaptureConfig {
	pub width: Option<u32>,
	pub height: Option<u32>,
	pub fps: Option<u32>,
	pub pixel_format: Option<String>,
}

const DIRECTSHOW_REQUEST_FPS_TOLERANCE: f32 = 0.5;

impl DirectShowCaptureConfig {
	pub fn new(width: u32, height: u32, fps: u32) -> Self {
		Self {
			width: Some(width),
			height: Some(height),
			fps: (fps > 0).then_some(fps),
			pixel_format: None,
		}
	}

	pub fn with_pixel_format(mut self, pixel_format: Option<String>) -> Self {
		self.pixel_format = match pixel_format {
			Some(value) => {
				let normalized = normalize_capture_pixel_format(value.clone());
				if normalized.as_deref() == Some("RGB24") && matches!(value.trim().to_ascii_uppercase().as_str(), "MJPG" | "MJPEG") {
					tracing::info!(
						target: "un_motion_input_webcam_directshow",
						requested_pixel_format = %value,
						ccap_output_pixel_format = "RGB24",
						"DirectShow MJPG selection will be decoded by ccap-rs into packed RGB output",
					);
				} else if normalized.is_none() {
					tracing::warn!(
						target: "un_motion_input_webcam_directshow",
						requested_pixel_format = %value,
						"DirectShow pixel format is listed by the device but not directly requestable through ccap-rs; opening without pixel format override",
					);
				}
				normalized
			}
			None => None,
		};
		self
	}

	pub fn requested_label(&self) -> String {
		let resolution = match (self.width, self.height) {
			(Some(width), Some(height)) => format!("{width}x{height}"),
			_ => "default-size".to_string(),
		};
		let fps = self.fps.map(|fps| fps.to_string()).unwrap_or_else(|| "default-fps".to_string());
		let pixel_format = self.pixel_format.as_deref().unwrap_or("default-format");
		format!("{resolution}@{fps} {pixel_format}")
	}
}

#[derive(Default)]
pub struct DirectShowWebcamBackend {
	#[cfg(windows)]
	active: Option<ActiveDirectShowCapture>,
	config: DirectShowCaptureConfig,
}

impl WebcamCaptureBackend for DirectShowWebcamBackend {
	fn list_devices(&mut self) -> anyhow::Result<Vec<WebcamDeviceInfo>> {
		directshow_devices()
	}

	fn capture_next_image(&mut self, device_id: &str) -> anyhow::Result<Option<ImageFrame>> {
		#[cfg(windows)]
		{
			let device = self.resolve_device(device_id)?;
			let active = match self.active.as_mut() {
				Some(active) if active.device_id == device.id && active.config == self.config => active,
				_ => {
					self.release_active_to_cache();
					self.active = take_cached_capture(&device.id, &self.config);
					if self.active.is_none() {
						self.active = Some(ActiveDirectShowCapture::open(device, self.config.clone())?);
					}
					self.active.as_mut().expect("active capture was just opened")
				}
			};
			active.capture_next_image()
		}
		#[cfg(not(windows))]
		{
			let _ = device_id;
			Ok(None)
		}
	}

	fn observed_source_fps(&self) -> Option<f32> {
		self.active_fps().or_else(|| self.active_observed_source_fps())
	}
}

impl DirectShowWebcamBackend {
	pub fn with_capture_config(config: DirectShowCaptureConfig) -> Self {
		Self {
			#[cfg(windows)]
			active: None,
			config,
		}
	}

	pub fn set_capture_config(&mut self, config: DirectShowCaptureConfig) {
		if self.config != config {
			#[cfg(windows)]
			{
				self.release_active_to_cache();
			}
			self.config = config;
		}
	}

	pub fn capture_config(&self) -> &DirectShowCaptureConfig {
		&self.config
	}

	pub fn active_format_label(&self) -> Option<String> {
		#[cfg(windows)]
		{
			return self.active.as_ref().map(ActiveDirectShowCapture::format_label);
		}
		#[cfg(not(windows))]
		{
			None
		}
	}

	pub fn active_fps(&self) -> Option<f32> {
		#[cfg(windows)]
		{
			return self.active.as_ref().and_then(|active| active.actual_fps);
		}
		#[cfg(not(windows))]
		{
			None
		}
	}

	pub fn active_observed_fps(&self) -> Option<f32> {
		#[cfg(windows)]
		{
			return self.active.as_ref().and_then(ActiveDirectShowCapture::observed_consumer_fps);
		}
		#[cfg(not(windows))]
		{
			None
		}
	}

	pub fn active_observed_source_fps(&self) -> Option<f32> {
		#[cfg(windows)]
		{
			return self.active.as_ref().and_then(ActiveDirectShowCapture::observed_source_fps);
		}
		#[cfg(not(windows))]
		{
			None
		}
	}

	pub fn active_observed_interval_ms(&self) -> Option<f32> {
		#[cfg(windows)]
		{
			return self
				.active
				.as_ref()
				.and_then(ActiveDirectShowCapture::observed_consumer_interval_ms);
		}
		#[cfg(not(windows))]
		{
			None
		}
	}

	#[cfg(windows)]
	fn resolve_device(&mut self, device_id: &str) -> anyhow::Result<WebcamDeviceInfo> {
		let devices = self.list_devices()?;
		if devices.is_empty() {
			anyhow::bail!("no DirectShow webcam devices found");
		}
		if device_id.trim().is_empty() {
			return Ok(devices[0].clone());
		}
		if let Some(device) = devices.iter().find(|device| device.id == device_id) {
			return Ok(device.clone());
		}
		let needle = comparable_label(device_id);
		if let Some(device) = devices.iter().find(|device| {
			let name = comparable_label(&device.name);
			!needle.is_empty() && (name == needle || name.contains(&needle) || needle.contains(&name))
		}) {
			return Ok(device.clone());
		}
		anyhow::bail!(
			"DirectShow webcam not found: {device_id}; available: {}",
			devices
				.iter()
				.map(|device| format!("{}:{}", device.id, device.name))
				.collect::<Vec<_>>()
				.join(", ")
		);
	}

	#[cfg(windows)]
	fn release_active_to_cache(&mut self) {
		if let Some(active) = self.active.take() {
			store_cached_capture(active);
		}
	}
}

#[cfg(windows)]
impl Drop for DirectShowWebcamBackend {
	fn drop(&mut self) {
		self.release_active_to_cache();
	}
}

#[cfg(windows)]
struct ActiveDirectShowCapture {
	device_id: String,
	device_name: String,
	config: DirectShowCaptureConfig,
	provider: ccap::Provider,
	stats: DirectShowCaptureStats,
	sequence: u64,
	actual_width: Option<u32>,
	actual_height: Option<u32>,
	actual_fps: Option<f32>,
	actual_pixel_format: Option<String>,
	last_delivered_width: Option<u32>,
	last_delivered_height: Option<u32>,
}

#[cfg(windows)]
struct PendingDirectShowFrame {
	timestamp_ns: u64,
	frame_timestamp_ns: u64,
	width: u32,
	height: u32,
	pixel_format: ccap::PixelFormat,
	reported_orientation: ccap::FrameOrientation,
	orientation: ccap::FrameOrientation,
	stride: u32,
	plane_lengths: [usize; 3],
	frame_index: u64,
	data: Vec<u8>,
}

#[cfg(windows)]
#[derive(Default)]
struct DirectShowCaptureStats {
	frames: u64,
	last_consumer_timestamp_ns: Option<u64>,
	last_source_timestamp_ns: Option<u64>,
	last_frame_index: Option<u64>,
	observed_consumer_fps: Option<f32>,
	observed_consumer_interval_ms: Option<f32>,
	observed_source_fps: Option<f32>,
	observed_source_interval_ms: Option<f32>,
	observed_frame_index_delta: Option<u64>,
}

#[cfg(windows)]
impl ActiveDirectShowCapture {
	fn open(device: WebcamDeviceInfo, config: DirectShowCaptureConfig) -> anyhow::Result<Self> {
		if std::env::var_os("UN_MOTION_CCAP_VERBOSE").is_some() {
			ccap::Utils::set_log_level(ccap::LogLevel::Verbose);
		}
		let mut provider = ccap::Provider::with_device_name_and_extra_info(&device.name, Some("dshow"))?;
		let requested_label = config.requested_label();
		let _ = provider.stop_capture();
		if let (Some(width), Some(height)) = (config.width, config.height) {
			if let Err(error) = provider.set_resolution(width, height) {
				tracing::warn!(
					target: "un_motion_input_webcam_directshow",
					device_id = %device.id,
					device_name = %device.name,
					requested = %requested_label,
					error = %error,
					width,
					height,
					"DirectShow requested resolution set failed"
				);
			}
		}
		if let Some(pixel_format) = config.pixel_format.as_deref().filter(|value| is_yuv_request(Some(value))) {
			let requested_pixel_format = ccap_pixel_format_from_label(Some(pixel_format));
			if let Err(error) = provider.set_property(ccap::PropertyName::PixelFormatInternal, requested_pixel_format.to_c_enum() as f64) {
				tracing::warn!(
					target: "un_motion_input_webcam_directshow",
					device_id = %device.id,
					device_name = %device.name,
					requested = %requested_label,
					requested_pixel_format = %pixel_format,
					error = %error,
					"DirectShow requested internal pixel format set failed"
				);
			}
		}
		if let Err(error) = provider.set_pixel_format(ccap::PixelFormat::Rgb24) {
			tracing::warn!(
				target: "un_motion_input_webcam_directshow",
				device_id = %device.id,
				device_name = %device.name,
				requested = %requested_label,
				requested_output_pixel_format = "RGB24",
				error = %error,
				"DirectShow requested output pixel format set failed"
			);
		}
		if let Some(fps) = config.fps {
			if let Err(error) = provider.set_frame_rate(fps as f64) {
				tracing::warn!(
					target: "un_motion_input_webcam_directshow",
					device_id = %device.id,
					device_name = %device.name,
					requested = %requested_label,
					requested_fps = fps,
					error = %error,
					"DirectShow requested frame rate set failed"
				);
			}
		}
		provider.start_capture()?;
		let (actual_width, actual_height) = provider
			.resolution()
			.map_or((None, None), |(width, height)| (Some(width), Some(height)));
		let actual_fps = provider
			.frame_rate()
			.ok()
			.map(|fps| fps as f32)
			.filter(|fps| fps.is_finite() && *fps > 0.0);
		let actual_pixel_format = provider.pixel_format().ok().map(|format| format.as_str().to_string());
		log_capture_negotiation(
			&device.id,
			&device.name,
			&requested_label,
			&config,
			actual_width,
			actual_height,
			actual_fps,
			actual_pixel_format.as_deref(),
		);
		tracing::info!(
			target: "un_motion_input_webcam_directshow",
			device_id = %device.id,
			device_name = %device.name,
			requested = %config.requested_label(),
			actual_width = ?actual_width,
			actual_height = ?actual_height,
			actual_fps = ?actual_fps,
			actual_pixel_format = ?actual_pixel_format,
			"DirectShow capture opened",
		);
		Ok(Self {
			device_id: device.id,
			device_name: device.name,
			config,
			provider,
			stats: DirectShowCaptureStats::default(),
			sequence: 0,
			actual_width,
			actual_height,
			actual_fps,
			actual_pixel_format,
			last_delivered_width: None,
			last_delivered_height: None,
		})
	}

	fn capture_next_image(&mut self) -> anyhow::Result<Option<ImageFrame>> {
		let Some(frame) = self.provider.grab_frame(1000)? else {
			return Ok(None);
		};
		let mut pending = pending_frame_from_ccap_frame(&frame)?;
		self.apply_effective_orientation(&mut pending);
		self.log_frame_probe(&pending);
		self.observe_frame_sample(&pending);
		self.last_delivered_width = Some(pending.width);
		self.last_delivered_height = Some(pending.height);
		let mut width = pending.width;
		let mut height = pending.height;
		let data = if let (Some(requested_width), Some(requested_height)) = (self.config.width, self.config.height) {
			if requested_width > 0 && requested_height > 0 && (requested_width != width || requested_height != height) {
				width = requested_width;
				height = requested_height;
				resize_ccap_frame_to_rgb8(&pending, requested_width, requested_height)?
			} else {
				convert_ccap_frame_to_rgb8(&pending)?
			}
		} else {
			convert_ccap_frame_to_rgb8(&pending)?
		};
		maybe_dump_rgb_frame(
			&self.device_id,
			&self.config.requested_label(),
			&self.format_label(),
			self.sequence,
			width,
			height,
			&data,
		);
		let image = ImageFrame {
			metadata: ImageFrameMetadata {
				sequence: self.sequence,
				capture_timestamp_ns: pending.timestamp_ns,
				timestamp_basis: TimestampBasis::UnixEpoch,
				source_id: self.device_id.clone(),
				source_label: Some(format!("{} {}", self.device_name, self.format_label())),
			},
			width,
			height,
			stride_bytes: width.saturating_mul(3),
			pixel_format: PixelFormat::Rgb8,
			data,
		};
		self.sequence = self.sequence.saturating_add(1);
		Ok(Some(image))
	}

	fn observed_consumer_fps(&self) -> Option<f32> {
		self.stats.observed_consumer_fps
	}

	fn observed_consumer_interval_ms(&self) -> Option<f32> {
		self.stats.observed_consumer_interval_ms
	}

	fn observed_source_fps(&self) -> Option<f32> {
		self.stats.observed_source_fps
	}

	fn observe_frame_sample(&mut self, pending: &PendingDirectShowFrame) {
		if let Some(previous) = self.stats.last_consumer_timestamp_ns {
			let interval_ms = pending.timestamp_ns.saturating_sub(previous) as f32 / 1_000_000.0;
			if interval_ms > 0.0 {
				self.stats.observed_consumer_interval_ms =
					Some(smooth_optional_metric(self.stats.observed_consumer_interval_ms, interval_ms));
				self.stats.observed_consumer_fps = Some(smooth_optional_metric(self.stats.observed_consumer_fps, 1000.0 / interval_ms));
			}
		}
		if let (Some(previous_timestamp), Some(previous_index)) = (self.stats.last_source_timestamp_ns, self.stats.last_frame_index) {
			if pending.frame_timestamp_ns > previous_timestamp && pending.frame_index > previous_index {
				let interval_ms = (pending.frame_timestamp_ns - previous_timestamp) as f32 / 1_000_000.0;
				let frame_delta = pending.frame_index - previous_index;
				if interval_ms > 0.0 && frame_delta > 0 {
					let source_fps = frame_delta as f32 * 1000.0 / interval_ms;
					self.stats.observed_source_interval_ms = Some(smooth_optional_metric(
						self.stats.observed_source_interval_ms,
						interval_ms / frame_delta as f32,
					));
					self.stats.observed_source_fps = Some(smooth_optional_metric(self.stats.observed_source_fps, source_fps));
					self.stats.observed_frame_index_delta = Some(frame_delta);
				}
			}
		}
		if self.stats.frames == 30 || self.stats.frames == 120 || self.stats.frames % 600 == 0 {
			tracing::info!(
				target: "un_motion_input_webcam_directshow",
				device_id = %self.device_id,
				requested = %self.config.requested_label(),
				actual = %self.format_label(),
				consumer_fps = ?self.stats.observed_consumer_fps,
				consumer_interval_ms = ?self.stats.observed_consumer_interval_ms,
				source_fps = ?self.stats.observed_source_fps,
				source_interval_ms = ?self.stats.observed_source_interval_ms,
				frame_index_delta = ?self.stats.observed_frame_index_delta,
				"DirectShow capture observed timing",
			);
		}
		self.stats.last_consumer_timestamp_ns = Some(pending.timestamp_ns);
		self.stats.last_source_timestamp_ns = Some(pending.frame_timestamp_ns);
		self.stats.last_frame_index = Some(pending.frame_index);
		self.stats.frames = self.stats.frames.saturating_add(1);
	}

	fn log_frame_probe(&self, pending: &PendingDirectShowFrame) {
		if self.stats.frames < 3 || self.stats.frames == 30 {
			tracing::info!(
				target: "un_motion_input_webcam_directshow",
				device_id = %self.device_id,
				requested = %self.config.requested_label(),
				actual = %self.format_label(),
				frame_index = pending.frame_index,
				frame_timestamp_ns = pending.frame_timestamp_ns,
				width = pending.width,
				height = pending.height,
				pixel_format = %pending.pixel_format.as_str(),
				reported_orientation = ?pending.reported_orientation,
				effective_orientation = ?pending.orientation,
				stride0 = pending.stride,
				plane0_len = pending.plane_lengths[0],
				plane1_len = pending.plane_lengths[1],
				plane2_len = pending.plane_lengths[2],
				"DirectShow frame probe",
			);
		}
	}

	fn apply_effective_orientation(&self, pending: &mut PendingDirectShowFrame) {
		if is_yuv_request(self.config.pixel_format.as_deref())
			&& is_packed_rgb_frame(pending.pixel_format)
			&& matches!(pending.reported_orientation, ccap::FrameOrientation::TopToBottom)
		{
			pending.orientation = ccap::FrameOrientation::BottomToTop;
			if self.stats.frames < 3 || self.stats.frames == 30 {
				tracing::info!(
					target: "un_motion_input_webcam_directshow",
					device_id = %self.device_id,
					requested = %self.config.requested_label(),
					frame_pixel_format = %pending.pixel_format.as_str(),
					reported_orientation = ?pending.reported_orientation,
					effective_orientation = ?pending.orientation,
					"DirectShow frame orientation overridden for decoded YUV request",
				);
			}
		}
	}

	fn format_label(&self) -> String {
		let resolution = match (
			self.last_delivered_width.or(self.actual_width),
			self.last_delivered_height.or(self.actual_height),
		) {
			(Some(width), Some(height)) if width > 0 && height > 0 => format!("{width}x{height}"),
			_ => "unknown-size".to_string(),
		};
		let fps = self
			.actual_fps
			.map(|fps| format!("{fps:.1}fps"))
			.unwrap_or_else(|| "unknown-fps".to_string());
		let pixel_format = self.actual_pixel_format.as_deref().unwrap_or("RGB24");
		let resize_suffix = match (
			self.config.width,
			self.config.height,
			self.last_delivered_width,
			self.last_delivered_height,
		) {
			(Some(width), Some(height), Some(delivered_width), Some(delivered_height))
				if width > 0 && height > 0 && (width != delivered_width || height != delivered_height) =>
			{
				format!(" -> {width}x{height}")
			}
			_ => String::new(),
		};
		format!("{resolution}@{fps} {pixel_format}{resize_suffix}")
	}
}

#[cfg(windows)]
impl Drop for ActiveDirectShowCapture {
	fn drop(&mut self) {
		let _ = self.provider.stop_capture();
	}
}

#[cfg(windows)]
static DIRECTSHOW_CAPTURE_CACHE: OnceLock<Mutex<Option<ActiveDirectShowCapture>>> = OnceLock::new();

#[cfg(windows)]
fn directshow_capture_cache() -> &'static Mutex<Option<ActiveDirectShowCapture>> {
	DIRECTSHOW_CAPTURE_CACHE.get_or_init(|| Mutex::new(None))
}

#[cfg(windows)]
fn take_cached_capture(device_id: &str, config: &DirectShowCaptureConfig) -> Option<ActiveDirectShowCapture> {
	let mut cached = directshow_capture_cache().lock().ok()?;
	let matches = cached
		.as_ref()
		.is_some_and(|active| active.device_id == device_id && active.config == *config);
	if matches {
		cached.take()
	} else {
		let stale = cached.take();
		drop(stale);
		None
	}
}

#[cfg(windows)]
fn store_cached_capture(active: ActiveDirectShowCapture) {
	if let Ok(mut cached) = directshow_capture_cache().lock() {
		let previous = cached.replace(active);
		drop(previous);
	}
}

#[cfg(windows)]
fn pending_frame_from_ccap_frame(frame: &ccap::VideoFrame) -> anyhow::Result<PendingDirectShowFrame> {
	let info = frame.info()?;
	let Some(src) = info.data_planes[0] else {
		anyhow::bail!("DirectShow frame has no first data plane");
	};
	let captured_at_ns = now_unix_ns();
	let plane_lengths = [
		info.data_planes[0].map_or(0, |plane| plane.len()),
		info.data_planes[1].map_or(0, |plane| plane.len()),
		info.data_planes[2].map_or(0, |plane| plane.len()),
	];
	Ok(PendingDirectShowFrame {
		timestamp_ns: captured_at_ns,
		frame_timestamp_ns: if info.timestamp > 0 { info.timestamp } else { captured_at_ns },
		width: info.width,
		height: info.height,
		pixel_format: info.pixel_format,
		reported_orientation: info.orientation,
		orientation: info.orientation,
		stride: info.strides[0],
		plane_lengths,
		frame_index: info.frame_index,
		data: src.to_vec(),
	})
}

#[cfg(windows)]
fn smooth_optional_metric(previous: Option<f32>, value: f32) -> f32 {
	previous.map_or(value, |previous| previous * 0.85 + value * 0.15)
}

fn normalize_capture_pixel_format(value: String) -> Option<String> {
	let upper = value.trim().to_ascii_uppercase();
	match upper.as_str() {
		"RGB24" | "BGR24" | "RGBA32" | "BGRA32" | "YUY2" | "YUYV" | "UYVY" => Some(upper),
		// ccap-rs does not expose MJPG as a returned frame format on Windows. Its DirectShow
		// backend negotiates MJPG internally and asks SampleGrabber for decoded packed RGB.
		"MJPG" | "MJPEG" => Some("RGB24".to_string()),
		_ => None,
	}
}

#[cfg(windows)]
fn is_yuv_request(pixel_format: Option<&str>) -> bool {
	matches!(
		pixel_format.map(|value| value.trim().to_ascii_uppercase()),
		Some(value) if matches!(value.as_str(), "YUY2" | "YUYV" | "UYVY")
	)
}

#[cfg(windows)]
fn is_packed_rgb_frame(pixel_format: ccap::PixelFormat) -> bool {
	matches!(
		pixel_format,
		ccap::PixelFormat::Rgb24 | ccap::PixelFormat::Bgr24 | ccap::PixelFormat::Rgba32 | ccap::PixelFormat::Bgra32
	)
}

#[cfg(windows)]
fn maybe_dump_rgb_frame(device_id: &str, requested: &str, actual: &str, sequence: u64, width: u32, height: u32, data: &[u8]) {
	let Ok(dir) = std::env::var("UN_MOTION_DIRECTSHOW_DUMP_FRAME_DIR") else {
		return;
	};
	if sequence
		> std::env::var("UN_MOTION_DIRECTSHOW_DUMP_MAX_SEQUENCE")
			.ok()
			.and_then(|value| value.parse::<u64>().ok())
			.unwrap_or(0)
	{
		return;
	}
	if width == 0 || height == 0 || data.len() < width as usize * height as usize * 3 {
		return;
	}
	let safe_requested = sanitize_dump_name(requested);
	let safe_actual = sanitize_dump_name(actual);
	let path = std::path::Path::new(&dir).join(format!("{device_id}-{sequence:04}-{safe_requested}-{safe_actual}.bmp"));
	let write_result = (|| -> std::io::Result<()> {
		std::fs::create_dir_all(&dir)?;
		write_rgb8_bmp(&path, width, height, data)
	})();
	match write_result {
		Ok(()) => tracing::info!(
			target: "un_motion_input_webcam_directshow",
			path = %path.display(),
			requested = %requested,
			actual = %actual,
			sequence,
			"DirectShow RGB frame dumped",
		),
		Err(error) => tracing::warn!(
			target: "un_motion_input_webcam_directshow",
			error = %error,
			path = %path.display(),
			"failed to dump DirectShow RGB frame",
		),
	}
}

#[cfg(windows)]
fn write_rgb8_bmp(path: &std::path::Path, width: u32, height: u32, data: &[u8]) -> std::io::Result<()> {
	use std::io::Write;

	let row_bytes = width as usize * 3;
	let padded_row_bytes = row_bytes.div_ceil(4) * 4;
	let pixel_bytes = padded_row_bytes * height as usize;
	let file_size = 14 + 40 + pixel_bytes;
	let mut file = std::fs::File::create(path)?;

	file.write_all(b"BM")?;
	file.write_all(&(file_size as u32).to_le_bytes())?;
	file.write_all(&[0, 0, 0, 0])?;
	file.write_all(&(54_u32).to_le_bytes())?;
	file.write_all(&(40_u32).to_le_bytes())?;
	file.write_all(&(width as i32).to_le_bytes())?;
	file.write_all(&(-(height as i32)).to_le_bytes())?;
	file.write_all(&(1_u16).to_le_bytes())?;
	file.write_all(&(24_u16).to_le_bytes())?;
	file.write_all(&(0_u32).to_le_bytes())?;
	file.write_all(&(pixel_bytes as u32).to_le_bytes())?;
	file.write_all(&(2835_i32).to_le_bytes())?;
	file.write_all(&(2835_i32).to_le_bytes())?;
	file.write_all(&(0_u32).to_le_bytes())?;
	file.write_all(&(0_u32).to_le_bytes())?;

	let padding = vec![0_u8; padded_row_bytes - row_bytes];
	for row in 0..height as usize {
		let src_row = &data[row * row_bytes..row * row_bytes + row_bytes];
		for pixel in src_row.chunks_exact(3) {
			file.write_all(&[pixel[2], pixel[1], pixel[0]])?;
		}
		file.write_all(&padding)?;
	}
	Ok(())
}

#[cfg(windows)]
fn sanitize_dump_name(value: &str) -> String {
	value
		.chars()
		.map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
		.collect::<String>()
		.trim_matches('-')
		.to_string()
}

#[cfg(windows)]
fn ccap_pixel_format_from_label(value: Option<&str>) -> ccap::PixelFormat {
	match value.unwrap_or("RGB24").trim().to_ascii_uppercase().as_str() {
		"BGR24" => ccap::PixelFormat::Bgr24,
		"RGBA32" => ccap::PixelFormat::Rgba32,
		"BGRA32" => ccap::PixelFormat::Bgra32,
		"YUY2" | "YUYV" => ccap::PixelFormat::Yuyv,
		"UYVY" => ccap::PixelFormat::Uyvy,
		_ => ccap::PixelFormat::Rgb24,
	}
}

#[cfg(windows)]
fn log_capture_negotiation(
	device_id: &str,
	device_name: &str,
	requested_label: &str,
	config: &DirectShowCaptureConfig,
	actual_width: Option<u32>,
	actual_height: Option<u32>,
	actual_fps: Option<f32>,
	actual_pixel_format: Option<&str>,
) {
	if let (Some(requested_width), Some(requested_height), Some(width), Some(height)) =
		(config.width, config.height, actual_width, actual_height)
	{
		if requested_width > 0 && requested_height > 0 && (requested_width != width || requested_height != height) {
			tracing::warn!(
				target: "un_motion_input_webcam_directshow",
				device_id = %device_id,
				device_name = %device_name,
				requested = %requested_label,
				requested_width = requested_width,
				requested_height = requested_height,
				actual_width = width,
				actual_height = height,
				"DirectShow capture resolution was changed by backend"
			);
		}
	}
	match (config.fps, actual_fps) {
		(Some(requested_fps), Some(actual_fps)) if requested_fps > 0 => {
			let requested_fps = requested_fps as f32;
			if (requested_fps - actual_fps).abs() > DIRECTSHOW_REQUEST_FPS_TOLERANCE {
				tracing::warn!(
					target: "un_motion_input_webcam_directshow",
					device_id = %device_id,
					device_name = %device_name,
					requested = %requested_label,
					requested_fps = requested_fps,
					actual_fps = actual_fps,
					"DirectShow capture frame-rate was changed by backend"
				);
			}
		}
		(Some(requested_fps), None) if requested_fps > 0 => {
			tracing::warn!(
				target: "un_motion_input_webcam_directshow",
				device_id = %device_id,
				device_name = %device_name,
				requested = %requested_label,
				requested_fps = requested_fps,
				"DirectShow capture did not report frame-rate"
			);
		}
		_ => {}
	}
	if let Some(requested_pixel_format) = config.pixel_format.as_deref() {
		let Some(requested_pixel_format) = normalize_capture_pixel_format(requested_pixel_format.to_string()) else {
			return;
		};
		match actual_pixel_format {
			Some(actual_pixel_format) => {
				if requested_pixel_format != actual_pixel_format.trim().to_ascii_uppercase() {
					tracing::warn!(
						target: "un_motion_input_webcam_directshow",
						device_id = %device_id,
						device_name = %device_name,
						requested = %requested_label,
						requested_pixel_format = %requested_pixel_format,
						actual_pixel_format = %actual_pixel_format,
						"DirectShow capture pixel format was changed by backend"
					);
				}
			}
			None => tracing::warn!(
				target: "un_motion_input_webcam_directshow",
				device_id = %device_id,
				device_name = %device_name,
				requested = %requested_label,
				requested_pixel_format = %requested_pixel_format,
				"DirectShow capture did not report pixel format"
			),
		}
	}
}

#[cfg(windows)]
fn resize_ccap_frame_to_rgb8(frame: &PendingDirectShowFrame, dst_width: u32, dst_height: u32) -> anyhow::Result<Vec<u8>> {
	if frame.width == 0 || frame.height == 0 || dst_width == 0 || dst_height == 0 {
		anyhow::bail!("invalid RGB resize dimensions");
	}
	if frame.width == dst_width && frame.height == dst_height {
		return convert_ccap_frame_to_rgb8(frame);
	}

	let src_width = frame.width as usize;
	let src_height = frame.height as usize;
	let dst_width_usize = dst_width as usize;
	let dst_height_usize = dst_height as usize;
	let src = frame.data.as_slice();
	let stride = frame.stride as usize;
	let row_bytes = ccap_pixel_row_bytes(frame.pixel_format, src_width)?;
	if stride < row_bytes {
		anyhow::bail!("DirectShow frame stride {stride} is shorter than row bytes {row_bytes}");
	}
	let required = stride
		.checked_mul(src_height)
		.ok_or_else(|| anyhow::anyhow!("DirectShow frame size overflow"))?;
	if src.len() < required {
		anyhow::bail!("DirectShow frame data is shorter than stride * height");
	}
	let x_map = (0..dst_width)
		.map(|x| (x as u64 * frame.width as u64 / dst_width as u64) as usize)
		.collect::<Vec<_>>();
	let mut dst = vec![0_u8; dst_width_usize * dst_height_usize * 3];
	for y in 0..dst_height_usize {
		let mapped_y = y * src_height / dst_height_usize;
		let src_y = match frame.orientation {
			ccap::FrameOrientation::TopToBottom => mapped_y,
			ccap::FrameOrientation::BottomToTop => src_height - 1 - mapped_y,
		};
		for (x, src_x) in x_map.iter().copied().enumerate() {
			let dst_index = (y * dst_width_usize + x) * 3;
			let pixel = read_ccap_rgb_pixel(frame, src, stride, src_x, src_y)?;
			dst[dst_index..dst_index + 3].copy_from_slice(&pixel);
		}
	}
	Ok(dst)
}

#[cfg(windows)]
fn convert_ccap_frame_to_rgb8(frame: &PendingDirectShowFrame) -> anyhow::Result<Vec<u8>> {
	let width = frame.width as usize;
	let height = frame.height as usize;
	let src = frame.data.as_slice();
	let stride = frame.stride as usize;
	let row_bytes = ccap_pixel_row_bytes(frame.pixel_format, width)?;
	if stride < row_bytes {
		anyhow::bail!("DirectShow frame stride {stride} is shorter than row bytes {row_bytes}");
	}
	let required = stride
		.checked_mul(height)
		.ok_or_else(|| anyhow::anyhow!("DirectShow frame size overflow"))?;
	if src.len() < required {
		anyhow::bail!("DirectShow frame data is shorter than stride * height");
	}
	if matches!(frame.pixel_format, ccap::PixelFormat::Rgb24) {
		if matches!(frame.orientation, ccap::FrameOrientation::TopToBottom) && stride == row_bytes {
			return Ok(src[..required].to_vec());
		}
		let mut rgb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(3));
		for row in 0..height {
			let src_row = match frame.orientation {
				ccap::FrameOrientation::TopToBottom => row,
				ccap::FrameOrientation::BottomToTop => height - 1 - row,
			};
			rgb.extend_from_slice(&src[src_row * stride..src_row * stride + row_bytes]);
		}
		return Ok(rgb);
	}
	if matches!(frame.pixel_format, ccap::PixelFormat::Bgr24) {
		return convert_bgr24_to_rgb8(src, stride, width, height, frame.orientation);
	}
	let mut rgb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(3));
	for row in 0..height {
		let src_row = match frame.orientation {
			ccap::FrameOrientation::TopToBottom => row,
			ccap::FrameOrientation::BottomToTop => height - 1 - row,
		};
		for x in 0..width {
			rgb.extend_from_slice(&read_ccap_rgb_pixel(frame, src, stride, x, src_row)?);
		}
	}
	Ok(rgb)
}

#[cfg(windows)]
fn convert_bgr24_to_rgb8(
	src: &[u8],
	stride: usize,
	width: usize,
	height: usize,
	orientation: ccap::FrameOrientation,
) -> anyhow::Result<Vec<u8>> {
	let row_bytes = width
		.checked_mul(3)
		.ok_or_else(|| anyhow::anyhow!("DirectShow BGR row size overflow"))?;
	if matches!(orientation, ccap::FrameOrientation::TopToBottom) && stride == row_bytes {
		let required = row_bytes
			.checked_mul(height)
			.ok_or_else(|| anyhow::anyhow!("DirectShow BGR frame size overflow"))?;
		let mut rgb = src
			.get(..required)
			.ok_or_else(|| anyhow::anyhow!("DirectShow BGR frame is out of bounds"))?
			.to_vec();
		for pixel in rgb.chunks_exact_mut(3) {
			pixel.swap(0, 2);
		}
		return Ok(rgb);
	}
	let mut rgb = vec![0_u8; row_bytes * height];
	for row in 0..height {
		let src_row = match orientation {
			ccap::FrameOrientation::TopToBottom => row,
			ccap::FrameOrientation::BottomToTop => height - 1 - row,
		};
		let src_offset = src_row
			.checked_mul(stride)
			.ok_or_else(|| anyhow::anyhow!("DirectShow BGR source row offset overflow"))?;
		let dst_offset = row
			.checked_mul(row_bytes)
			.ok_or_else(|| anyhow::anyhow!("DirectShow BGR destination row offset overflow"))?;
		let src_row = src
			.get(src_offset..src_offset + row_bytes)
			.ok_or_else(|| anyhow::anyhow!("DirectShow BGR source row is out of bounds"))?;
		let dst_row = &mut rgb[dst_offset..dst_offset + row_bytes];
		for (src_pixel, dst_pixel) in src_row.chunks_exact(3).zip(dst_row.chunks_exact_mut(3)) {
			dst_pixel.copy_from_slice(&[src_pixel[2], src_pixel[1], src_pixel[0]]);
		}
	}
	Ok(rgb)
}

#[cfg(windows)]
fn ccap_pixel_layout(pixel_format: ccap::PixelFormat) -> anyhow::Result<(usize, PackedRgbOrder)> {
	match pixel_format {
		ccap::PixelFormat::Rgb24 => Ok((3, PackedRgbOrder::Rgb)),
		ccap::PixelFormat::Bgr24 => Ok((3, PackedRgbOrder::Bgr)),
		ccap::PixelFormat::Rgba32 => Ok((4, PackedRgbOrder::Rgba)),
		ccap::PixelFormat::Bgra32 => Ok((4, PackedRgbOrder::Bgra)),
		other => anyhow::bail!("unsupported packed RGB DirectShow output pixel format: {}", other.as_str()),
	}
}

#[cfg(windows)]
fn ccap_pixel_row_bytes(pixel_format: ccap::PixelFormat, width: usize) -> anyhow::Result<usize> {
	match pixel_format {
		ccap::PixelFormat::Yuyv | ccap::PixelFormat::Uyvy => width
			.checked_mul(2)
			.ok_or_else(|| anyhow::anyhow!("DirectShow YUV row size overflow")),
		_ => {
			let (channels, _) = ccap_pixel_layout(pixel_format)?;
			width
				.checked_mul(channels)
				.ok_or_else(|| anyhow::anyhow!("DirectShow RGB row size overflow"))
		}
	}
}

#[cfg(windows)]
fn read_ccap_rgb_pixel(frame: &PendingDirectShowFrame, src: &[u8], stride: usize, x: usize, y: usize) -> anyhow::Result<[u8; 3]> {
	match frame.pixel_format {
		ccap::PixelFormat::Rgb24 | ccap::PixelFormat::Bgr24 | ccap::PixelFormat::Rgba32 | ccap::PixelFormat::Bgra32 => {
			let (channels, order) = ccap_pixel_layout(frame.pixel_format)?;
			let index = y
				.checked_mul(stride)
				.and_then(|base| base.checked_add(x.checked_mul(channels)?))
				.ok_or_else(|| anyhow::anyhow!("DirectShow RGB pixel offset overflow"))?;
			let pixel = src
				.get(index..index + channels)
				.ok_or_else(|| anyhow::anyhow!("DirectShow RGB pixel is out of bounds"))?;
			Ok(match order {
				PackedRgbOrder::Rgb | PackedRgbOrder::Rgba => [pixel[0], pixel[1], pixel[2]],
				PackedRgbOrder::Bgr | PackedRgbOrder::Bgra => [pixel[2], pixel[1], pixel[0]],
			})
		}
		ccap::PixelFormat::Yuyv | ccap::PixelFormat::Uyvy => {
			let pair_x = x & !1;
			let index = y
				.checked_mul(stride)
				.and_then(|base| base.checked_add(pair_x.checked_mul(2)?))
				.ok_or_else(|| anyhow::anyhow!("DirectShow YUV pixel offset overflow"))?;
			let pair = src
				.get(index..index + 4)
				.ok_or_else(|| anyhow::anyhow!("DirectShow YUV pixel is out of bounds"))?;
			let (y0, u, y1, v) = match frame.pixel_format {
				ccap::PixelFormat::Yuyv => (pair[0], pair[1], pair[2], pair[3]),
				ccap::PixelFormat::Uyvy => (pair[1], pair[0], pair[3], pair[2]),
				_ => unreachable!(),
			};
			let y_value = if x == pair_x { y0 } else { y1 };
			Ok(yuv_to_rgb(y_value, u, v))
		}
		other => anyhow::bail!("unsupported DirectShow output pixel format: {}", other.as_str()),
	}
}

#[cfg(windows)]
fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
	let c = y as i32 - 16;
	let d = u as i32 - 128;
	let e = v as i32 - 128;
	let r = (298 * c + 409 * e + 128) >> 8;
	let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
	let b = (298 * c + 516 * d + 128) >> 8;
	[clamp_u8(r), clamp_u8(g), clamp_u8(b)]
}

#[cfg(windows)]
fn clamp_u8(value: i32) -> u8 {
	value.clamp(0, 255) as u8
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum PackedRgbOrder {
	Rgb,
	Bgr,
	Rgba,
	Bgra,
}

pub fn diagnose_devices<B: WebcamCaptureBackend>(backend: &mut B) -> anyhow::Result<WebcamDiagnosticReport> {
	let devices = backend.list_devices()?;
	Ok(WebcamDiagnosticReport {
		backend: "directshow",
		device_count: devices.len(),
		devices,
	})
}

pub fn list_directshow_capture_formats(device_id_or_name: &str) -> anyhow::Result<Vec<DirectShowCaptureFormatInfo>> {
	directshow_capture_formats(device_id_or_name)
}

/// DirectShow video capture デバイスの一覧を返す Supervisor GUI 向け公開 API。
/// GUI から呼んで Camera device dropdown を構築する用途。Windows 以外では
/// 空の `Vec` を返す (`directshow_devices()` の非 Windows 実装に従う)。
pub fn list_directshow_devices() -> anyhow::Result<Vec<WebcamDeviceInfo>> {
	directshow_devices()
}

pub struct WebcamImageInputSource<B: WebcamCaptureBackend> {
	backend: B,
	device_id: String,
}

impl<B: WebcamCaptureBackend> WebcamImageInputSource<B> {
	pub fn new(backend: B, device_id: impl Into<String>) -> Self {
		Self {
			backend,
			device_id: device_id.into(),
		}
	}
}

impl<B: WebcamCaptureBackend> ImageInputSource for WebcamImageInputSource<B> {
	fn next_image_frame(&mut self) -> anyhow::Result<Option<ImageFrame>> {
		self.backend.capture_next_image(&self.device_id)
	}

	fn observed_source_fps(&self) -> Option<f32> {
		self.backend.observed_source_fps()
	}
}

#[cfg(windows)]
fn directshow_devices() -> anyhow::Result<Vec<WebcamDeviceInfo>> {
	let _guard = ComGuard::new();
	Ok(directshow_device_monikers()?.into_iter().map(|entry| entry.device).collect())
}

#[cfg(windows)]
fn directshow_capture_formats(device_id_or_name: &str) -> anyhow::Result<Vec<DirectShowCaptureFormatInfo>> {
	use std::collections::BTreeSet;
	use std::ffi::c_void;
	use std::mem::size_of;
	use windows::Win32::Media::DirectShow::{IAMStreamConfig, IBaseFilter, ICaptureGraphBuilder2, IGraphBuilder};
	use windows::Win32::Media::MediaFoundation::{
		AM_MEDIA_TYPE, CLSID_CaptureGraphBuilder2, CLSID_FilterGraph, MEDIATYPE_Video, PIN_CATEGORY_CAPTURE, VIDEOINFOHEADER,
		VIDEOINFOHEADER2,
	};
	use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
	use windows::core::Interface;

	let _guard = ComGuard::new();
	let entries = directshow_device_monikers()?;
	let entry = resolve_directshow_device_entry(&entries, device_id_or_name)?;
	let filter: IBaseFilter = unsafe { entry.moniker.BindToObject(None, None)? };
	let graph: IGraphBuilder = unsafe { CoCreateInstance(&CLSID_FilterGraph, None, CLSCTX_INPROC_SERVER)? };
	let capture_builder: ICaptureGraphBuilder2 = unsafe { CoCreateInstance(&CLSID_CaptureGraphBuilder2, None, CLSCTX_INPROC_SERVER)? };
	unsafe {
		capture_builder.SetFiltergraph(&graph)?;
		graph.AddFilter(&filter, windows::core::w!("UNMotion DirectShow Source"))?;
	}

	let mut stream_config_ptr: *mut c_void = std::ptr::null_mut();
	unsafe {
		capture_builder.FindInterface(
			Some(&PIN_CATEGORY_CAPTURE),
			Some(&MEDIATYPE_Video),
			&filter,
			&IAMStreamConfig::IID,
			&mut stream_config_ptr,
		)?;
	}
	if stream_config_ptr.is_null() {
		anyhow::bail!("DirectShow IAMStreamConfig was not found for {}", entry.device.name);
	}
	let stream_config = unsafe { IAMStreamConfig::from_raw(stream_config_ptr as _) };
	let mut count = 0;
	let mut size = 0;
	unsafe { stream_config.GetNumberOfCapabilities(&mut count, &mut size)? };
	if count <= 0 || size <= 0 {
		return Ok(Vec::new());
	}

	let mut dedupe = BTreeSet::new();
	let mut formats = Vec::new();
	for index in 0..count {
		let mut media_type_ptr: *mut AM_MEDIA_TYPE = std::ptr::null_mut();
		let mut caps = vec![0_u8; size as usize];
		let result = unsafe { stream_config.GetStreamCaps(index, &mut media_type_ptr, caps.as_mut_ptr()) };
		if result.is_err() || media_type_ptr.is_null() {
			unsafe {
				free_am_media_type(media_type_ptr);
			}
			continue;
		}
		let parsed = unsafe { parse_am_media_type(&*media_type_ptr, size_of::<VIDEOINFOHEADER>(), size_of::<VIDEOINFOHEADER2>()) };
		unsafe {
			free_am_media_type(media_type_ptr);
		}
		let Some(format) = parsed else {
			continue;
		};
		let key = (format.width, format.height, format.fps.unwrap_or(0), format.pixel_format.clone());
		if dedupe.insert(key) {
			formats.push(format);
		}
	}
	formats.sort_by(|left, right| {
		(
			left.width as u64 * left.height as u64,
			left.width,
			left.height,
			left.fps.unwrap_or(0),
			&left.pixel_format,
		)
			.cmp(&(
				right.width as u64 * right.height as u64,
				right.width,
				right.height,
				right.fps.unwrap_or(0),
				&right.pixel_format,
			))
	});
	Ok(formats)
}

#[cfg(windows)]
struct DirectShowDeviceEntry {
	device: WebcamDeviceInfo,
	moniker: windows::Win32::System::Com::IMoniker,
}

#[cfg(windows)]
struct ComGuard {
	initialized: bool,
}

#[cfg(windows)]
impl ComGuard {
	fn new() -> Self {
		use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
		Self {
			initialized: unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok() },
		}
	}
}

#[cfg(windows)]
impl Drop for ComGuard {
	fn drop(&mut self) {
		if self.initialized {
			unsafe {
				windows::Win32::System::Com::CoUninitialize();
			}
		}
	}
}

#[cfg(windows)]
fn directshow_device_monikers() -> anyhow::Result<Vec<DirectShowDeviceEntry>> {
	use windows::Win32::Media::DirectShow::ICreateDevEnum;
	use windows::Win32::Media::MediaFoundation::{CLSID_SystemDeviceEnum, CLSID_VideoInputDeviceCategory};
	use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};

	let dev_enum: ICreateDevEnum = unsafe { CoCreateInstance(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER)? };
	let mut enum_moniker = None;
	unsafe { dev_enum.CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut enum_moniker, 0)? };
	let Some(enum_moniker) = enum_moniker else {
		return Ok(Vec::new());
	};

	let mut devices = Vec::new();
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
		let name = directshow_moniker_friendly_name(moniker).unwrap_or_else(|_| "<unknown>".to_string());
		let index = devices.len();
		devices.push(DirectShowDeviceEntry {
			device: WebcamDeviceInfo {
				id: format!("dshow{index}"),
				name,
			},
			moniker: moniker.clone(),
		});
	}
	Ok(devices)
}

#[cfg(windows)]
fn resolve_directshow_device_entry<'a>(
	entries: &'a [DirectShowDeviceEntry],
	device_id_or_name: &str,
) -> anyhow::Result<&'a DirectShowDeviceEntry> {
	if entries.is_empty() {
		anyhow::bail!("no DirectShow webcam devices found");
	}
	let selected = device_id_or_name.split(':').next().unwrap_or(device_id_or_name);
	if let Some(entry) = entries
		.iter()
		.find(|entry| entry.device.id == selected || entry.device.id == device_id_or_name)
	{
		return Ok(entry);
	}
	let label = device_id_or_name
		.split_once(':')
		.map(|(_, label)| label)
		.unwrap_or(device_id_or_name);
	let needle = comparable_label(label);
	if let Some(entry) = entries.iter().find(|entry| {
		let name = comparable_label(&entry.device.name);
		!needle.is_empty() && (name == needle || name.contains(&needle) || needle.contains(&name))
	}) {
		return Ok(entry);
	}
	if entries.len() == 1 {
		return Ok(&entries[0]);
	}
	anyhow::bail!("DirectShow webcam not found: {device_id_or_name}");
}

#[cfg(windows)]
fn directshow_moniker_friendly_name(moniker: &windows::Win32::System::Com::IMoniker) -> anyhow::Result<String> {
	use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
	use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VariantClear};
	use windows::core::w;

	let bag: IPropertyBag = unsafe { moniker.BindToStorage(None, None)? };
	let mut value = VARIANT::default();
	unsafe { bag.Read(w!("FriendlyName"), &mut value, None)? };
	let variant = unsafe { &value.Anonymous.Anonymous };
	let text = if variant.vt == VT_BSTR {
		let bstr = unsafe { &variant.Anonymous.bstrVal };
		String::try_from(&**bstr).unwrap_or_else(|_| "<unknown>".to_string())
	} else {
		"<unknown>".to_string()
	};
	unsafe {
		let _ = VariantClear(&mut value);
	}
	Ok(text)
}

#[cfg(windows)]
unsafe fn parse_am_media_type(
	media_type: &windows::Win32::Media::MediaFoundation::AM_MEDIA_TYPE,
	video_info_size: usize,
	video_info2_size: usize,
) -> Option<DirectShowCaptureFormatInfo> {
	use windows::Win32::Media::MediaFoundation::{FORMAT_VideoInfo, FORMAT_VideoInfo2, MEDIATYPE_Video, VIDEOINFOHEADER, VIDEOINFOHEADER2};

	if media_type.majortype != MEDIATYPE_Video || media_type.pbFormat.is_null() {
		return None;
	}
	let (width, height, avg_time_per_frame) =
		if media_type.formattype == FORMAT_VideoInfo && media_type.cbFormat as usize >= video_info_size {
			let info = unsafe { &*(media_type.pbFormat as *const VIDEOINFOHEADER) };
			(
				info.bmiHeader.biWidth.unsigned_abs(),
				info.bmiHeader.biHeight.unsigned_abs(),
				info.AvgTimePerFrame,
			)
		} else if media_type.formattype == FORMAT_VideoInfo2 && media_type.cbFormat as usize >= video_info2_size {
			let info = unsafe { &*(media_type.pbFormat as *const VIDEOINFOHEADER2) };
			(
				info.bmiHeader.biWidth.unsigned_abs(),
				info.bmiHeader.biHeight.unsigned_abs(),
				info.AvgTimePerFrame,
			)
		} else {
			return None;
		};
	if width == 0 || height == 0 {
		return None;
	}
	Some(DirectShowCaptureFormatInfo {
		width,
		height,
		fps: avg_time_per_frame_to_fps(avg_time_per_frame),
		pixel_format: directshow_subtype_label(&media_type.subtype),
	})
}

#[cfg(windows)]
fn avg_time_per_frame_to_fps(avg_time_per_frame: i64) -> Option<u32> {
	if avg_time_per_frame <= 0 {
		return None;
	}
	let fps = 10_000_000_f64 / avg_time_per_frame as f64;
	(fps.is_finite() && fps > 0.0).then(|| fps.round().clamp(1.0, 1000.0) as u32)
}

#[cfg(windows)]
fn directshow_subtype_label(subtype: &windows::core::GUID) -> String {
	use windows::Win32::Media::MediaFoundation::{
		MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_RGB24, MFVideoFormat_RGB32, MFVideoFormat_YUY2,
	};
	if *subtype == MFVideoFormat_MJPG {
		"MJPG".to_string()
	} else if *subtype == MFVideoFormat_NV12 {
		"NV12".to_string()
	} else if *subtype == MFVideoFormat_YUY2 {
		"YUY2".to_string()
	} else if *subtype == MFVideoFormat_RGB24 {
		"RGB24".to_string()
	} else if *subtype == MFVideoFormat_RGB32 {
		"RGB32".to_string()
	} else if let Some(fourcc) = directshow_fourcc_label(subtype) {
		fourcc
	} else {
		format!("{subtype:?}")
	}
}

#[cfg(windows)]
fn directshow_fourcc_label(subtype: &windows::core::GUID) -> Option<String> {
	const DIRECTSHOW_FOURCC_TAIL: (u16, u16, [u8; 8]) = (0x0000, 0x0010, [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]);
	if (subtype.data2, subtype.data3, subtype.data4) != DIRECTSHOW_FOURCC_TAIL {
		return None;
	}
	let chars = subtype.data1.to_le_bytes();
	chars
		.iter()
		.all(|byte| byte.is_ascii_graphic() || *byte == b' ')
		.then(|| String::from_utf8_lossy(&chars).trim().to_string())
}

#[cfg(windows)]
unsafe fn free_am_media_type(media_type: *mut windows::Win32::Media::MediaFoundation::AM_MEDIA_TYPE) {
	use std::mem::ManuallyDrop;
	use windows::Win32::System::Com::CoTaskMemFree;
	if media_type.is_null() {
		return;
	}
	let media_type_ref = unsafe { &mut *media_type };
	if !media_type_ref.pbFormat.is_null() {
		unsafe { CoTaskMemFree(Some(media_type_ref.pbFormat.cast())) };
		media_type_ref.pbFormat = std::ptr::null_mut();
	}
	let punk = unsafe { ManuallyDrop::take(&mut media_type_ref.pUnk) };
	drop(punk);
	unsafe { CoTaskMemFree(Some(media_type.cast())) };
}

#[cfg(not(windows))]
fn directshow_devices() -> anyhow::Result<Vec<WebcamDeviceInfo>> {
	Ok(Vec::new())
}

#[cfg(not(windows))]
fn directshow_capture_formats(_device_id_or_name: &str) -> anyhow::Result<Vec<DirectShowCaptureFormatInfo>> {
	Ok(Vec::new())
}

fn comparable_label(value: &str) -> String {
	value
		.chars()
		.filter(|ch| ch.is_ascii_alphanumeric())
		.flat_map(char::to_lowercase)
		.collect()
}

fn now_unix_ns() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_nanos()
		.min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Default)]
	struct MockBackend {
		devices: Vec<WebcamDeviceInfo>,
	}

	impl WebcamCaptureBackend for MockBackend {
		fn list_devices(&mut self) -> anyhow::Result<Vec<WebcamDeviceInfo>> {
			Ok(self.devices.clone())
		}

		fn capture_next_image(&mut self, _device_id: &str) -> anyhow::Result<Option<ImageFrame>> {
			Ok(None)
		}
	}

	#[test]
	fn diagnostic_report_renders_directshow_devices() {
		let mut backend = MockBackend {
			devices: vec![WebcamDeviceInfo {
				id: "dshow0".to_string(),
				name: "OBS Virtual Camera".to_string(),
			}],
		};
		let report = diagnose_devices(&mut backend).unwrap();
		assert_eq!(report.device_count, 1);
		assert_eq!(report.to_text_lines()[0], "directshow webcam devices: 1");
	}

	#[test]
	fn mjpg_request_normalizes_to_decoded_rgb_output() {
		assert_eq!(normalize_capture_pixel_format("MJPG".to_string()).as_deref(), Some("RGB24"));
		assert_eq!(normalize_capture_pixel_format("mjpeg".to_string()).as_deref(), Some("RGB24"));
	}

	#[test]
	fn unknown_pixel_format_is_not_requestable() {
		assert_eq!(normalize_capture_pixel_format("NV12".to_string()), None);
		assert_eq!(normalize_capture_pixel_format("YUV420".to_string()), None);
	}

	#[test]
	fn yuv_request_detection_is_limited_to_packed_yuv_formats() {
		assert!(is_yuv_request(Some("YUY2")));
		assert!(is_yuv_request(Some("uyvy")));
		assert!(!is_yuv_request(Some("MJPG")));
		assert!(!is_yuv_request(Some("BGR24")));
		assert!(!is_yuv_request(None));
	}
}
