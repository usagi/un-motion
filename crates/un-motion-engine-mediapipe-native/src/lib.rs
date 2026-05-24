use std::path::Path;

use un_motion_engine_mediapipe_types::MediaPipeRawOutput;
use un_motion_interfaces::{ImageFrame, ImageInferenceEngine, PixelFormat};
use un_motion_mediapipe_native::{
	NativeLiveStreamStats, NativeMediaPipeOptions, NativeMediaPipeOutput, NativeMediaPipeRuntime, RgbImageRef,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeMediaPipeEngineConfig {
	pub options: NativeMediaPipeOptions,
	pub include_gestures: bool,
}

impl Default for NativeMediaPipeEngineConfig {
	fn default() -> Self {
		Self {
			options: NativeMediaPipeOptions::desktop_video(),
			include_gestures: false,
		}
	}
}

pub struct NativeMediaPipeImageEngine {
	runtime: NativeMediaPipeRuntime,
	config: NativeMediaPipeEngineConfig,
}

impl NativeMediaPipeImageEngine {
	pub fn open_default(config: NativeMediaPipeEngineConfig) -> anyhow::Result<Self> {
		let runtime = NativeMediaPipeRuntime::open_default_with_options(config.options)?;
		Ok(Self { runtime, config })
	}

	pub fn open_dll(path: impl AsRef<Path>, config: NativeMediaPipeEngineConfig) -> anyhow::Result<Self> {
		let runtime = NativeMediaPipeRuntime::open_with_options(path, config.options)?;
		Ok(Self { runtime, config })
	}

	pub fn config(&self) -> &NativeMediaPipeEngineConfig {
		&self.config
	}

	pub fn process_rgb_frame(&mut self, frame: &ImageFrame) -> anyhow::Result<NativeMediaPipeOutput> {
		let rgb = image_frame_rgb_ref(frame)?;
		let timestamp_ms = frame.metadata.capture_timestamp_ns.saturating_div(1_000_000).min(i64::MAX as u64) as i64;
		if self.config.include_gestures {
			self.runtime.process_rgb_everything_at(rgb, timestamp_ms)
		} else {
			self.runtime.process_rgb_at(rgb, timestamp_ms)
		}
	}

	pub fn poll_latest(&mut self) -> anyhow::Result<NativeMediaPipeOutput> {
		self.runtime.poll_latest(self.config.include_gestures)
	}

	pub fn live_stream_stats(&mut self) -> Option<NativeLiveStreamStats> {
		self.runtime.live_stream_stats()
	}
}

impl ImageInferenceEngine for NativeMediaPipeImageEngine {
	type Output = MediaPipeRawOutput;

	fn process_image(&mut self, frame: &ImageFrame) -> anyhow::Result<Self::Output> {
		self.process_rgb_frame(frame).map(Into::into)
	}
}

pub fn image_frame_rgb_ref(frame: &ImageFrame) -> anyhow::Result<RgbImageRef<'_>> {
	if frame.pixel_format != PixelFormat::Rgb8 {
		anyhow::bail!("MediaPipe Native currently requires Rgb8 ImageFrame input");
	}
	if frame.width == 0 || frame.height == 0 || frame.stride_bytes == 0 {
		anyhow::bail!("ImageFrame dimensions and stride must be non-zero");
	}
	let minimum_len = frame.stride_bytes as usize * frame.height as usize;
	if frame.data.len() < minimum_len {
		anyhow::bail!("ImageFrame data is shorter than stride * height");
	}
	Ok(RgbImageRef {
		bytes: &frame.data,
		width: frame.width,
		height: frame.height,
		stride: frame.stride_bytes,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_interfaces::{ImageFrameMetadata, TimestampBasis};

	#[test]
	fn accepts_rgb8_image_frame_as_native_rgb_ref() {
		let frame = ImageFrame::new_rgb8(1, 2_000_000, "file:test", 2, 1, vec![255, 0, 0, 0, 255, 0]).unwrap();
		let rgb = image_frame_rgb_ref(&frame).unwrap();
		assert_eq!(rgb.width, 2);
		assert_eq!(rgb.height, 1);
		assert_eq!(rgb.stride, 6);
		assert_eq!(rgb.bytes.len(), 6);
	}

	#[test]
	fn rejects_non_rgb8_image_frame() {
		let frame = ImageFrame {
			metadata: ImageFrameMetadata {
				sequence: 1,
				capture_timestamp_ns: 0,
				timestamp_basis: TimestampBasis::UnixEpoch,
				source_id: "test".to_string(),
				source_label: None,
			},
			width: 1,
			height: 1,
			stride_bytes: 4,
			pixel_format: PixelFormat::Rgba8,
			data: vec![0, 0, 0, 255],
		};
		let err = image_frame_rgb_ref(&frame).expect_err("rgba must be rejected");
		assert!(err.to_string().contains("Rgb8"));
	}

	#[test]
	fn rejects_short_image_frame_buffer() {
		let frame = ImageFrame {
			metadata: ImageFrameMetadata {
				sequence: 1,
				capture_timestamp_ns: 0,
				timestamp_basis: TimestampBasis::UnixEpoch,
				source_id: "test".to_string(),
				source_label: None,
			},
			width: 2,
			height: 2,
			stride_bytes: 6,
			pixel_format: PixelFormat::Rgb8,
			data: vec![0; 11],
		};
		let err = image_frame_rgb_ref(&frame).expect_err("short buffer must be rejected");
		assert!(err.to_string().contains("stride * height"));
	}
}
