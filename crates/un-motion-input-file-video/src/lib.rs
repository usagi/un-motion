use std::io::Read;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use un_motion_interfaces::{ImageFrame, ImageInputSource, checked_image_pixel_count};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const STDERR_TAIL_MAX_BYTES: usize = 8 * 1024;

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
	frame_rx: Option<Receiver<Vec<u8>>>,
	reader_stop: Arc<AtomicBool>,
	reader_done: Arc<AtomicBool>,
	reader_error: Arc<Mutex<Option<String>>>,
	reader_join: Option<JoinHandle<()>>,
	stderr_tail: Arc<Mutex<Vec<u8>>>,
	stderr_join: Option<JoinHandle<()>>,
	sequence: u64,
	frame_len: usize,
	exhausted: bool,
}

impl FileVideoInputSource {
	pub fn open(config: FileVideoInputConfig) -> anyhow::Result<Self> {
		validate_config(&config)?;
		let frame_len = rgb_frame_len(config.output_width, config.output_height)?;
		let mut source = Self {
			config,
			child: None,
			frame_rx: None,
			reader_stop: Arc::new(AtomicBool::new(false)),
			reader_done: Arc::new(AtomicBool::new(false)),
			reader_error: Arc::new(Mutex::new(None)),
			reader_join: None,
			stderr_tail: Arc::new(Mutex::new(Vec::new())),
			stderr_join: None,
			sequence: 0,
			frame_len,
			exhausted: false,
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
			.stderr(Stdio::piped());
		let mut child = command_without_console(&mut command)
			.spawn()
			.map_err(|error| anyhow::anyhow!("failed to start ffmpeg at {}: {error}", self.config.ffmpeg_path.display()))?;
		let stdout = child
			.stdout
			.take()
			.ok_or_else(|| anyhow::anyhow!("ffmpeg stdout pipe was not created"))?;
		let stderr = child
			.stderr
			.take()
			.ok_or_else(|| anyhow::anyhow!("ffmpeg stderr pipe was not created"))?;
		let (frame_tx, frame_rx) = mpsc::sync_channel(1);
		let stop = Arc::clone(&self.reader_stop);
		let done = Arc::clone(&self.reader_done);
		let error = Arc::clone(&self.reader_error);
		let frame_len = self.frame_len;
		self.reader_join = Some(thread::spawn(move || {
			run_frame_reader(stdout, frame_len, frame_tx, stop, done, error)
		}));
		let stderr_tail = Arc::clone(&self.stderr_tail);
		self.stderr_join = Some(thread::spawn(move || run_stderr_reader(stderr, stderr_tail)));
		self.frame_rx = Some(frame_rx);
		self.child = Some(child);
		Ok(())
	}

	fn next_rgb_bytes(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
		if self.exhausted {
			return Ok(None);
		}
		if let Some(error) = self.reader_error.lock().ok().and_then(|mut error| error.take()) {
			self.exhausted = true;
			return Err(anyhow::anyhow!(error));
		};
		let Some(frame_rx) = self.frame_rx.as_ref() else {
			self.exhausted = true;
			return Ok(None);
		};
		match frame_rx.try_recv() {
			Ok(data) => Ok(Some(data)),
			Err(TryRecvError::Empty) if !self.reader_done.load(Ordering::Acquire) => Ok(None),
			Err(TryRecvError::Empty | TryRecvError::Disconnected) => self.finish_ffmpeg(),
		}
	}

	fn finish_ffmpeg(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
		let Some(child) = self.child.as_mut() else {
			self.exhausted = true;
			return Ok(None);
		};
		let Some(status) = child.try_wait()? else {
			return Ok(None);
		};
		self.child = None;
		self.exhausted = true;
		self.join_reader_threads();
		if status.success() {
			return Ok(None);
		}
		let stderr = self
			.stderr_tail
			.lock()
			.map(|tail| String::from_utf8_lossy(&tail).trim().to_string())
			.unwrap_or_default();
		let detail = if stderr.is_empty() {
			"stderr was empty".to_string()
		} else {
			stderr
		};
		Err(anyhow::anyhow!("ffmpeg exited with status {status}: {detail}"))
	}

	fn join_reader_threads(&mut self) {
		if let Some(reader_join) = self.reader_join.take() {
			let _ = reader_join.join();
		}
		if let Some(stderr_join) = self.stderr_join.take() {
			let _ = stderr_join.join();
		}
	}
}

fn run_frame_reader(
	mut stdout: ChildStdout,
	frame_len: usize,
	frame_tx: SyncSender<Vec<u8>>,
	stop: Arc<AtomicBool>,
	done: Arc<AtomicBool>,
	error: Arc<Mutex<Option<String>>>,
) {
	while !stop.load(Ordering::Acquire) {
		let mut data = vec![0_u8; frame_len];
		let mut offset = 0;
		while offset < data.len() && !stop.load(Ordering::Acquire) {
			match stdout.read(&mut data[offset..]) {
				Ok(0) => {
					done.store(true, Ordering::Release);
					return;
				}
				Ok(read) => offset += read,
				Err(read_error) if read_error.kind() == std::io::ErrorKind::Interrupted => continue,
				Err(read_error) => {
					if let Ok(mut slot) = error.lock() {
						*slot = Some(format!("read ffmpeg stdout: {read_error}"));
					}
					done.store(true, Ordering::Release);
					return;
				}
			}
		}
		if offset != data.len() {
			break;
		}
		if !send_decoded_frame(&frame_tx, data) {
			done.store(true, Ordering::Release);
			return;
		}
	}
	done.store(true, Ordering::Release);
}

fn send_decoded_frame(frame_tx: &SyncSender<Vec<u8>>, data: Vec<u8>) -> bool {
	frame_tx.send(data).is_ok()
}

fn run_stderr_reader(mut stderr: ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
	let mut buffer = [0_u8; 1024];
	loop {
		match stderr.read(&mut buffer) {
			Ok(0) => return,
			Ok(read) => {
				let Ok(mut tail) = tail.lock() else { return };
				append_bounded_stderr(&mut tail, &buffer[..read]);
			}
			Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
			Err(_) => return,
		}
	}
}

fn append_bounded_stderr(tail: &mut Vec<u8>, bytes: &[u8]) {
	tail.extend_from_slice(bytes);
	let overflow = tail.len().saturating_sub(STDERR_TAIL_MAX_BYTES);
	if overflow > 0 {
		tail.drain(..overflow);
	}
}

impl Drop for FileVideoInputSource {
	fn drop(&mut self) {
		self.reader_stop.store(true, Ordering::Release);
		self.frame_rx = None;
		if let Some(mut child) = self.child.take() {
			let _ = child.kill();
			let _ = child.wait();
		}
		self.join_reader_threads();
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
	checked_image_pixel_count(config.output_width, config.output_height)?;
	if config.fps == 0 {
		anyhow::bail!("video fps must be greater than zero");
	}
	Ok(())
}

fn rgb_frame_len(width: u32, height: u32) -> anyhow::Result<usize> {
	checked_image_pixel_count(width, height)?
		.checked_mul(3)
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

	#[test]
	fn oversized_dimensions_are_rejected() {
		let config = FileVideoInputConfig::new("clip.mp4", 8192, 8192, 24);
		assert!(validate_config(&config).is_err());
		assert!(rgb_frame_len(8192, 8192).is_err());
	}

	#[test]
	fn exhausted_source_stays_idle() {
		let config = FileVideoInputConfig::new("clip.mp4", 640, 480, 30);
		let mut source = FileVideoInputSource {
			config,
			child: None,
			frame_rx: None,
			reader_stop: Arc::new(AtomicBool::new(false)),
			reader_done: Arc::new(AtomicBool::new(true)),
			reader_error: Arc::new(Mutex::new(None)),
			reader_join: None,
			stderr_tail: Arc::new(Mutex::new(Vec::new())),
			stderr_join: None,
			sequence: 0,
			frame_len: 640 * 480 * 3,
			exhausted: true,
		};
		assert!(source.next_rgb_bytes().expect("first idle poll").is_none());
		assert!(source.next_rgb_bytes().expect("repeated idle poll").is_none());
		assert!(source.exhausted);
	}

	#[test]
	fn stderr_tail_keeps_only_latest_bounded_bytes() {
		let mut tail = vec![b'a'; STDERR_TAIL_MAX_BYTES];
		append_bounded_stderr(&mut tail, b"latest");
		assert_eq!(tail.len(), STDERR_TAIL_MAX_BYTES);
		assert!(tail.ends_with(b"latest"));
	}

	#[test]
	fn decoded_frame_send_stops_when_receiver_disconnects() {
		let (tx, rx) = mpsc::sync_channel(1);
		drop(rx);
		assert!(!send_decoded_frame(&tx, vec![1, 2, 3]));
	}
}
