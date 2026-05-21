#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
	Rgb8,
	Rgba8,
	Bgr8,
	Bgra8,
	Gray8,
	Nv12,
	Yuyv,
	Encoded,
}

impl PixelFormat {
	pub fn bytes_per_pixel(self) -> Option<u32> {
		match self {
			Self::Rgb8 | Self::Bgr8 => Some(3),
			Self::Rgba8 | Self::Bgra8 => Some(4),
			Self::Gray8 => Some(1),
			Self::Nv12 | Self::Yuyv | Self::Encoded => None,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampBasis {
	UnixEpoch,
	Monotonic,
	SourceMediaTime,
	Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbaColor {
	pub r: u8,
	pub g: u8,
	pub b: u8,
	pub a: u8,
}

impl RgbaColor {
	pub const BLACK: Self = Self { r: 0, g: 0, b: 0, a: 255 };

	pub fn parse_rrggbbaa(value: &str) -> anyhow::Result<Self> {
		let hex = value.trim().trim_start_matches('#');
		if hex.len() != 8 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
			anyhow::bail!("expected RRGGBBAA color");
		}
		Ok(Self {
			r: parse_hex_byte(&hex[0..2])?,
			g: parse_hex_byte(&hex[2..4])?,
			b: parse_hex_byte(&hex[4..6])?,
			a: parse_hex_byte(&hex[6..8])?,
		})
	}
}

fn parse_hex_byte(value: &str) -> anyhow::Result<u8> {
	u8::from_str_radix(value, 16).map_err(Into::into)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeAxis {
	Width,
	Height,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageResizeOptions {
	pub preserve_aspect_ratio: bool,
	pub reference_axis: ResizeAxis,
	pub reference_length: u32,
	pub output_width: u32,
	pub output_height: u32,
	pub pad_color: RgbaColor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageFrameMetadata {
	pub sequence: u64,
	pub capture_timestamp_ns: u64,
	pub timestamp_basis: TimestampBasis,
	pub source_id: String,
	pub source_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageFrame {
	pub metadata: ImageFrameMetadata,
	pub width: u32,
	pub height: u32,
	pub stride_bytes: u32,
	pub pixel_format: PixelFormat,
	pub data: Vec<u8>,
}

impl ImageFrame {
	pub fn new_rgb8(
		sequence: u64,
		capture_timestamp_ns: u64,
		source_id: impl Into<String>,
		width: u32,
		height: u32,
		data: Vec<u8>,
	) -> anyhow::Result<Self> {
		let stride_bytes = width.saturating_mul(3);
		let expected_len = stride_bytes as usize * height as usize;
		if data.len() < expected_len {
			anyhow::bail!("RGB frame buffer is shorter than stride * height");
		}
		Ok(Self {
			metadata: ImageFrameMetadata {
				sequence,
				capture_timestamp_ns,
				timestamp_basis: TimestampBasis::UnixEpoch,
				source_id: source_id.into(),
				source_label: None,
			},
			width,
			height,
			stride_bytes,
			pixel_format: PixelFormat::Rgb8,
			data,
		})
	}
}

pub trait ImageInputSource {
	fn next_image_frame(&mut self) -> anyhow::Result<Option<ImageFrame>>;

	fn observed_source_fps(&self) -> Option<f32> {
		None
	}
}

pub trait EventImageInputSource {
	fn poll_image_events(&mut self) -> anyhow::Result<Vec<ImageFrame>>;
}

pub trait ImageFrameBuffer {
	fn push(&mut self, frame: ImageFrame);
	fn latest(&self) -> Option<ImageFrame>;
	fn latest_by_source(&self, source_id: &str) -> Option<ImageFrame>;
	fn read_batch(&mut self, max_frames: usize) -> Vec<ImageFrame>;
	fn len(&self) -> usize;
	fn capacity(&self) -> usize;
	fn dropped_count(&self) -> u64;
	fn event_version(&self) -> u64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueOverflowPolicy {
	DropOldest,
	DropNewest,
	BlockProducer,
	ReplaceOld,
}

impl Default for QueueOverflowPolicy {
	fn default() -> Self {
		Self::DropOldest
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueuePushResult {
	Accepted,
	ReplacedOld,
	DroppedNew,
	Blocked,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueStats {
	pub len: usize,
	pub capacity: usize,
	pub pushed: u64,
	pub popped: u64,
	pub replaced_old: u64,
	pub dropped_new: u64,
	pub blocked: u64,
	pub event_version: u64,
}

pub trait FrameQueue<T> {
	fn push(&mut self, item: T) -> QueuePushResult;
	fn pop_latest(&mut self) -> Option<T>;
	fn pop_oldest(&mut self) -> Option<T>;
	fn drain(&mut self, max: usize) -> Vec<T>;
	fn stats(&self) -> QueueStats;
}

pub trait ImageInferenceEngine {
	type Output;

	fn process_image(&mut self, frame: &ImageFrame) -> anyhow::Result<Self::Output>;
}

pub trait FrameProcessor<I, O> {
	fn process(&mut self, input: I) -> anyhow::Result<O>;
}

pub trait OutputSink<T> {
	fn send(&mut self, frame: &T) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn image_frame_can_wrap_rgb_data() {
		let frame = ImageFrame::new_rgb8(7, 123, "image:test", 2, 1, vec![255, 0, 0, 0, 255, 0]).unwrap();
		assert_eq!(frame.width, 2);
		assert_eq!(frame.pixel_format, PixelFormat::Rgb8);
		assert_eq!(frame.metadata.sequence, 7);
		assert_eq!(frame.metadata.source_id, "image:test");
	}

	#[test]
	fn rgba_color_parses_rrggbbaa() {
		assert_eq!(
			RgbaColor::parse_rrggbbaa("11223344").unwrap(),
			RgbaColor {
				r: 0x11,
				g: 0x22,
				b: 0x33,
				a: 0x44
			}
		);
		assert!(RgbaColor::parse_rrggbbaa("112233").is_err());
	}
}
