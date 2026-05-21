use std::io::Read;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use un_motion_interfaces::{ImageFrame, ImageInputSource};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn command_without_console(command: &mut Command) -> &mut Command {
	#[cfg(windows)]
	{
		command.creation_flags(CREATE_NO_WINDOW);
	}
	command
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileVideoInputConfig {
	pub path: PathBuf,
	pub source_id: String,
	pub source_label: Option<String>,
	pub fps: u32,
	pub output_width: u32,
	pub output_height: u32,
	pub repeat: bool,
	pub ffmpeg_path: PathBuf,
}

impl FileVideoInputConfig {
	pub fn new(path: impl Into<PathBuf>, output_width: u32, output_height: u32, fps: u32) -> Self {
		let path = path.into();
		Self {
			source_id: format!("file-video:{}", path.display()),
			source_label: path.file_name().map(|name| name.to_string_lossy().to_string()),
			path,
			fps,
			output_width,
			output_height,
			repeat: false,
			ffmpeg_path: PathBuf::from("ffmpeg"),
		}
	}
}

pub struct FileVideoInputSource {
	config: FileVideoInputConfig,
	child: Option<Child>,
	stdout: Option<ChildStdout>,
	sequence: u64,
	frame_len: usize,
}

impl FileVideoInputSource {
	pub fn open(config: FileVideoInputConfig) -> anyhow::Result<Self> {
		validate_config(&config)?;
		let frame_len = rgb_frame_len(config.output_width, config.output_height)?;
		let mut source = Self {
			config,
			child: None,
			stdout: None,
			sequence: 0,
			frame_len,
		};
		source.spawn_ffmpeg()?;
		Ok(source)
	}

	pub fn path(&self) -> &Path {
		&self.config.path
	}

	pub fn ffmpeg_args(config: &FileVideoInputConfig) -> Vec<String> {
		let mut args = vec!["-hide_banner".to_string(), "-loglevel".to_string(), "error".to_string()];
		if config.repeat {
			args.extend(["-stream_loop".to_string(), "-1".to_string()]);
		}
		args.extend([
			"-i".to_string(),
			config.path.display().to_string(),
			"-vf".to_string(),
			format!("fps={},scale={}x{}", config.fps.max(1), config.output_width, config.output_height),
			"-an".to_string(),
			"-sn".to_string(),
			"-f".to_string(),
			"rawvideo".to_string(),
			"-pix_fmt".to_string(),
			"rgb24".to_string(),
			"pipe:1".to_string(),
		]);
		args
	}

	fn spawn_ffmpeg(&mut self) -> anyhow::Result<()> {
		let mut command = Command::new(&self.config.ffmpeg_path);
		command
			.args(Self::ffmpeg_args(&self.config))
			.stdin(Stdio::null())
			.stdout(Stdio::piped())
			.stderr(Stdio::null());
		let mut child = command_without_console(&mut command)
			.spawn()
			.map_err(|error| anyhow::anyhow!("failed to start ffmpeg at {}: {error}", self.config.ffmpeg_path.display()))?;
		self.stdout = child.stdout.take();
		self.child = Some(child);
		Ok(())
	}

	fn next_rgb_bytes(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
		let Some(stdout) = self.stdout.as_mut() else {
			return Ok(None);
		};
		let mut data = vec![0_u8; self.frame_len];
		let mut offset = 0;
		while offset < data.len() {
			match stdout.read(&mut data[offset..]) {
				Ok(0) => return Ok(None),
				Ok(read) => offset += read,
				Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
				Err(error) => return Err(error.into()),
			}
		}
		Ok(Some(data))
	}
}

impl Drop for FileVideoInputSource {
	fn drop(&mut self) {
		if let Some(mut child) = self.child.take() {
			let _ = child.kill();
			let _ = child.wait();
		}
	}
}

impl ImageInputSource for FileVideoInputSource {
	fn next_image_frame(&mut self) -> anyhow::Result<Option<ImageFrame>> {
		let Some(data) = self.next_rgb_bytes()? else {
			return Ok(None);
		};
		let timestamp = now_unix_ns();
		let mut frame = ImageFrame::new_rgb8(
			self.sequence,
			timestamp,
			self.config.source_id.clone(),
			self.config.output_width,
			self.config.output_height,
			data,
		)?;
		frame.metadata.source_label = self.config.source_label.clone();
		self.sequence = self.sequence.saturating_add(1);
		Ok(Some(frame))
	}
}

fn validate_config(config: &FileVideoInputConfig) -> anyhow::Result<()> {
	if config.path.as_os_str().is_empty() {
		anyhow::bail!("video path must not be empty");
	}
	if config.output_width == 0 || config.output_height == 0 {
		anyhow::bail!("video output dimensions must be non-zero");
	}
	if config.fps == 0 {
		anyhow::bail!("video fps must be greater than zero");
	}
	Ok(())
}

fn rgb_frame_len(width: u32, height: u32) -> anyhow::Result<usize> {
	width
		.checked_mul(height)
		.and_then(|pixels| pixels.checked_mul(3))
		.map(|bytes| bytes as usize)
		.ok_or_else(|| anyhow::anyhow!("video frame size overflow"))
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

	#[test]
	fn ffmpeg_args_include_repeat_fps_scale_and_raw_rgb_output() {
		let mut config = FileVideoInputConfig::new("movie.mkv", 1280, 720, 30);
		config.repeat = true;
		let args = FileVideoInputSource::ffmpeg_args(&config);
		assert!(args.windows(2).any(|pair| pair == ["-stream_loop", "-1"]));
		assert!(args.windows(2).any(|pair| pair == ["-vf", "fps=30,scale=1280x720"]));
		assert!(args.windows(2).any(|pair| pair == ["-pix_fmt", "rgb24"]));
		assert_eq!(args.last().map(String::as_str), Some("pipe:1"));
	}

	#[test]
	fn config_defaults_source_label_from_file_name() {
		let config = FileVideoInputConfig::new("clip.mp4", 640, 480, 24);
		assert_eq!(config.source_label.as_deref(), Some("clip.mp4"));
		assert_eq!(config.source_id, "file-video:clip.mp4");
	}

	#[test]
	fn zero_dimensions_are_rejected() {
		let config = FileVideoInputConfig::new("clip.mp4", 0, 480, 24);
		let err = validate_config(&config).expect_err("zero width should fail");
		assert!(err.to_string().contains("dimensions"));
	}
}
