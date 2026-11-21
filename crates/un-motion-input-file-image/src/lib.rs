use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use image::{ImageBuffer, RgbaImage, imageops};
use un_motion_interfaces::{
	ImageFrame, ImageInputSource, ImageResizeOptions, PixelFormat, ResizeAxis, TimestampBasis, checked_image_pixel_count,
};

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
	frame_data: Arc<[u8]>,
	width: u32,
	height: u32,
	stride_bytes: u32,
	sequence: u64,
	last_emit: Option<Instant>,
	emitted_once: bool,
}

impl FileImageInputSource {
	pub fn open(config: FileImageInputConfig) -> anyhow::Result<Self> {
		let (source_width, source_height) = image::image_dimensions(&config.path)?;
		checked_image_pixel_count(source_width, source_height)?;
		if let Some(resize) = config.resize
			&& resize.output_width > 0
			&& resize.output_height > 0
			&& resize.reference_length > 0
		{
			checked_image_pixel_count(resize.output_width, resize.output_height)?;
		}
		let image = image::open(&config.path)?;
		let image = if let Some(resize) = config.resize {
			resize_rgba(image.to_rgba8(), resize)
		} else {
			image.to_rgba8()
		};
		let width = image.width();
		let height = image.height();
		let frame_data = Arc::from(rgba_to_rgb(image));
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
				let height = scale_other_axis(width, src_width, src_height);
				if height > options.output_height {
					let height = options.output_height;
					(scale_other_axis(height, src_height, src_width).min(width), height)
				} else {
					(width, height)
				}
			}
			ResizeAxis::Height => {
				let height = options.reference_length.min(options.output_height).max(1);
				let width = scale_other_axis(height, src_height, src_width);
				if width > options.output_width {
					let width = options.output_width;
					(width, scale_other_axis(width, src_width, src_height).min(height))
				} else {
					(width, height)
				}
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
	let pixels = checked_image_pixel_count(image.width(), image.height()).expect("validated image dimensions");
	let mut rgb = Vec::with_capacity(pixels * 3);
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
	fn repeated_frames_share_immutable_pixel_storage() {
		let path = temp_image_path("shared-repeat-source.png");
		write_test_image(&path);
		let mut config = FileImageInputConfig::once(&path);
		config.emission_mode = FileImageEmissionMode::RepeatFps(60);
		let mut source = FileImageInputSource::open(config).unwrap();
		let first = source.build_frame();
		let second = source.build_frame();
		assert!(Arc::ptr_eq(&first.data, &second.data));
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

	#[test]
	fn aspect_preservation_fits_both_canvas_axes() {
		for (width, height, axis, expected) in [
			(2, 4, ResizeAxis::Width, (1, 0, 2, 3)),
			(4, 2, ResizeAxis::Height, (0, 1, 3, 2)),
		] {
			let source = RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255]));
			let path = temp_image_path(&format!("aspect-{width}-{height}.png"));
			source.save(&path).unwrap();
			let mut config = FileImageInputConfig::once(&path);
			config.resize = Some(ImageResizeOptions {
				preserve_aspect_ratio: true, reference_axis: axis, reference_length: 4,
				output_width: 4, output_height: 4, pad_color: RgbaColor::BLACK,
			});
			let mut input = FileImageInputSource::open(config).unwrap();
			let frame = input.next_image_frame().unwrap().unwrap();
			assert_eq!((frame.width, frame.height, frame.pixel_format), (4, 4, PixelFormat::Rgb8));
			let content: Vec<_> = frame.data.chunks_exact(3).enumerate().filter(|(_, pixel)| pixel[0] == 255).map(|(index,_)| (index as u32 % 4, index as u32 / 4)).collect();
			let bounds = (content.iter().map(|p| p.0).min().unwrap(), content.iter().map(|p| p.1).min().unwrap(), content.iter().map(|p| p.0).max().unwrap(), content.iter().map(|p| p.1).max().unwrap());
			assert_eq!(bounds, expected, "portrait/landscape content must retain its aspect ratio");
			let _ = std::fs::remove_file(path);
		}
	}

	#[test]
	fn oversized_resize_canvas_is_rejected_before_allocation() {
		let path = temp_image_path("oversized-resize.png");
		write_test_image(&path);
		let mut config = FileImageInputConfig::once(&path);
		config.resize = Some(ImageResizeOptions {
			preserve_aspect_ratio: false,
			reference_axis: ResizeAxis::Width,
			reference_length: 1,
			output_width: 8192,
			output_height: 8192,
			pad_color: RgbaColor::BLACK,
		});
		assert!(FileImageInputSource::open(config).is_err());
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
