use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use image::{ImageBuffer, RgbaImage, imageops};
use un_motion_interfaces::{ImageFrame, ImageInputSource, ImageResizeOptions, PixelFormat, ResizeAxis, TimestampBasis};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileImageEmissionMode {
	Once,
	RepeatFps(u32),
}

#[derive(Clone, Debug)]
pub struct FileImageInputConfig {
	pub path: PathBuf,
	pub source_id: String,
	pub source_label: Option<String>,
	pub emission_mode: FileImageEmissionMode,
	pub resize: Option<ImageResizeOptions>,
}

impl FileImageInputConfig {
	pub fn once(path: impl Into<PathBuf>) -> Self {
		let path = path.into();
		Self {
			source_id: format!("file-image:{}", path.display()),
			source_label: path.file_name().map(|name| name.to_string_lossy().to_string()),
			path,
			emission_mode: FileImageEmissionMode::Once,
			resize: None,
		}
	}
}

pub struct FileImageInputSource {
	config: FileImageInputConfig,
	frame_data: Vec<u8>,
	width: u32,
	height: u32,
	stride_bytes: u32,
	sequence: u64,
	last_emit: Option<Instant>,
	emitted_once: bool,
}

impl FileImageInputSource {
	pub fn open(config: FileImageInputConfig) -> anyhow::Result<Self> {
		let image = image::open(&config.path)?;
		let image = if let Some(resize) = config.resize {
			resize_rgba(image.to_rgba8(), resize)
		} else {
			image.to_rgba8()
		};
		let width = image.width();
		let height = image.height();
		let frame_data = rgba_to_rgb(image);
		Ok(Self {
			config,
			frame_data,
			width,
			height,
			stride_bytes: width.saturating_mul(3),
			sequence: 0,
			last_emit: None,
			emitted_once: false,
		})
	}

	pub fn open_once(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
		Self::open(FileImageInputConfig::once(path))
	}

	pub fn path(&self) -> &Path {
		&self.config.path
	}

	fn should_emit(&self) -> bool {
		match self.config.emission_mode {
			FileImageEmissionMode::Once => !self.emitted_once,
			FileImageEmissionMode::RepeatFps(fps) => {
				let Some(last_emit) = self.last_emit else {
					return true;
				};
				let fps = fps.max(1);
				last_emit.elapsed() >= Duration::from_secs_f64(1.0 / fps as f64)
			}
		}
	}

	fn build_frame(&mut self) -> ImageFrame {
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
			.unwrap_or_default();
		let frame = ImageFrame {
			metadata: un_motion_interfaces::ImageFrameMetadata {
				sequence: self.sequence,
				capture_timestamp_ns: timestamp,
				timestamp_basis: TimestampBasis::UnixEpoch,
				source_id: self.config.source_id.clone(),
				source_label: self.config.source_label.clone(),
			},
			width: self.width,
			height: self.height,
			stride_bytes: self.stride_bytes,
			pixel_format: PixelFormat::Rgb8,
			data: self.frame_data.clone(),
		};
		self.sequence = self.sequence.saturating_add(1);
		self.last_emit = Some(Instant::now());
		self.emitted_once = true;
		frame
	}
}

impl ImageInputSource for FileImageInputSource {
	fn next_image_frame(&mut self) -> anyhow::Result<Option<ImageFrame>> {
		Ok(self.should_emit().then(|| self.build_frame()))
	}
}

fn resize_rgba(image: RgbaImage, options: ImageResizeOptions) -> RgbaImage {
	if options.output_width == 0 || options.output_height == 0 || options.reference_length == 0 {
		return image;
	}

	let (src_width, src_height) = image.dimensions();
	let (target_width, target_height) = if options.preserve_aspect_ratio {
		match options.reference_axis {
			ResizeAxis::Width => {
				let width = options.reference_length.min(options.output_width).max(1);
				let height = scale_other_axis(width, src_width, src_height).min(options.output_height).max(1);
				(width, height)
			}
			ResizeAxis::Height => {
				let height = options.reference_length.min(options.output_height).max(1);
				let width = scale_other_axis(height, src_height, src_width).min(options.output_width).max(1);
				(width, height)
			}
		}
	} else {
		(options.output_width, options.output_height)
	};

	let resized = imageops::resize(&image, target_width, target_height, imageops::FilterType::Triangle);
	let mut canvas = ImageBuffer::from_pixel(
		options.output_width,
		options.output_height,
		image::Rgba([options.pad_color.r, options.pad_color.g, options.pad_color.b, options.pad_color.a]),
	);
	let x = (options.output_width - target_width) / 2;
	let y = (options.output_height - target_height) / 2;
	imageops::overlay(&mut canvas, &resized, x.into(), y.into());
	canvas
}

fn scale_other_axis(reference: u32, src_reference: u32, src_other: u32) -> u32 {
	if src_reference == 0 {
		return reference;
	}
	((src_other as u64 * reference as u64 + src_reference as u64 / 2) / src_reference as u64)
		.max(1)
		.min(u32::MAX as u64) as u32
}

fn rgba_to_rgb(image: RgbaImage) -> Vec<u8> {
	let mut rgb = Vec::with_capacity(image.width() as usize * image.height() as usize * 3);
	for pixel in image.pixels() {
		rgb.extend_from_slice(&pixel.0[..3]);
	}
	rgb
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_interfaces::{ResizeAxis, RgbaColor};

	#[test]
	fn once_source_emits_only_one_frame() {
		let path = temp_image_path("once-source.png");
		write_test_image(&path);
		let mut source = FileImageInputSource::open_once(&path).unwrap();
		let first = source.next_image_frame().unwrap().expect("first frame");
		assert_eq!(first.width, 2);
		assert_eq!(first.height, 1);
		assert_eq!(first.pixel_format, PixelFormat::Rgb8);
		assert!(source.next_image_frame().unwrap().is_none());
		let _ = std::fs::remove_file(path);
	}

	#[test]
	fn resize_can_letterbox_to_requested_canvas() {
		let path = temp_image_path("letterbox-source.png");
		write_test_image(&path);
		let mut config = FileImageInputConfig::once(&path);
		config.resize = Some(ImageResizeOptions {
			preserve_aspect_ratio: true,
			reference_axis: ResizeAxis::Width,
			reference_length: 4,
			output_width: 4,
			output_height: 4,
			pad_color: RgbaColor { r: 1, g: 2, b: 3, a: 255 },
		});
		let mut source = FileImageInputSource::open(config).unwrap();
		let frame = source.next_image_frame().unwrap().expect("frame");
		assert_eq!((frame.width, frame.height), (4, 4));
		assert_eq!(&frame.data[..3], &[1, 2, 3]);
		let _ = std::fs::remove_file(path);
	}

	fn temp_image_path(name: &str) -> PathBuf {
		std::env::temp_dir().join(format!("un-motion-input-file-image-{name}"))
	}

	fn write_test_image(path: &Path) {
		let image = RgbaImage::from_fn(2, 1, |x, _| {
			if x == 0 {
				image::Rgba([255, 0, 0, 255])
			} else {
				image::Rgba([0, 255, 0, 255])
			}
		});
		image.save(path).unwrap();
	}
}
