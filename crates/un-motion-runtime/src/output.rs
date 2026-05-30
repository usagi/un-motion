use std::fs::{self, File};
use std::io::BufWriter;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{
	Arc,
	mpsc::{self, Receiver, Sender},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Context;
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use tracing::{debug, info, warn};
use un_motion_frame::UNMotionFrame;
use un_motion_output_vmc::{vmc_packets_for_frame, vmc_packets_for_frame_without_ok};
use un_motion_output_vrc_osc::{VrcOscOutputOptions, VrcOscOutputSink};
use un_motion_record::MessagePackStreamRecorder;

use crate::signal_enrich::{enrich_frame_with_signal_derived_motion, frame_needs_signal_derived_motion};
use crate::{ModifierConfig, ModifierPipeline, vmc_bone_name_to_humanoid_bone};

#[derive(Clone, Debug, PartialEq)]
pub struct VmcOutputConfig {
	pub target_addr: SocketAddr,
	pub send_ok_packet: bool,
	/// Capturer の出力段で適用する Modifier。
	///
	/// 正式経路では post-modifier `UNMotionFrame` を VMC OSC に変換する。
	pub modifier: ModifierConfig,
}

impl VmcOutputConfig {
	pub fn new(target_addr: SocketAddr) -> Self {
		Self {
			target_addr,
			send_ok_packet: true,
			modifier: ModifierConfig::default(),
		}
	}

	pub fn with_ok_packet(mut self, enabled: bool) -> Self {
		self.send_ok_packet = enabled;
		self
	}

	pub fn with_modifier(mut self, modifier: ModifierConfig) -> Self {
		self.modifier = modifier;
		self
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum VmcOutputFrame {
	/// 正式経路: post-process 済み `UNMotionFrame` を受け取り、Modifier 適用後に
	/// `vmc_packets_for_frame` で OSC bundle に変換して送信する。
	UnmotionFrame(UNMotionFrame),
	SharedUnmotionFrame(Arc<UNMotionFrame>),
}

impl From<UNMotionFrame> for VmcOutputFrame {
	fn from(frame: UNMotionFrame) -> Self {
		Self::UnmotionFrame(frame)
	}
}

impl From<Arc<UNMotionFrame>> for VmcOutputFrame {
	fn from(frame: Arc<UNMotionFrame>) -> Self {
		Self::SharedUnmotionFrame(frame)
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum VmcOutputCommand {
	Send(VmcOutputFrame),
	Shutdown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VmcOutputStats {
	pub sent_datagrams: u64,
	pub sent_packets: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmcOutputEvent {
	Sent {
		target_addr: SocketAddr,
		datagrams: u64,
		packets: u64,
	},
	Error {
		target_addr: SocketAddr,
		message: String,
	},
	Stopped {
		target_addr: SocketAddr,
		stats: VmcOutputStats,
	},
}

pub struct VmcOutputWorker {
	config: VmcOutputConfig,
	modifier_pipeline: ModifierPipeline,
	socket: UdpSocket,
	stats: VmcOutputStats,
}

impl VmcOutputWorker {
	pub fn bind(config: VmcOutputConfig) -> anyhow::Result<Self> {
		let socket = UdpSocket::bind("0.0.0.0:0").context("VMC output UDP socket bind failed")?;
		Ok(Self {
			modifier_pipeline: ModifierPipeline::from_config(&config.modifier),
			config,
			socket,
			stats: VmcOutputStats::default(),
		})
	}

	pub fn target_addr(&self) -> SocketAddr {
		self.config.target_addr
	}

	pub fn stats(&self) -> &VmcOutputStats {
		&self.stats
	}

	pub fn send_frame(&mut self, frame: VmcOutputFrame) -> anyhow::Result<VmcOutputEvent> {
		match frame {
			VmcOutputFrame::UnmotionFrame(mut frame) => self.send_unmotion_frame(&mut frame),
			VmcOutputFrame::SharedUnmotionFrame(frame) => {
				if self.config.modifier.is_pass_through() && !frame_needs_signal_derived_motion(&frame) {
					return self.send_borrowed_unmotion_frame(&frame);
				}
				let mut frame = (*frame).clone();
				self.send_unmotion_frame(&mut frame)
			}
		}
	}

	fn send_unmotion_frame(&mut self, frame: &mut UNMotionFrame) -> anyhow::Result<VmcOutputEvent> {
		// 正式経路: UNMotionFrame を OSC bundle に変換する final edge。
		// signal-only frame はまず canonical な body/face/hand に昇格し、Modifier を
		// UNMotionFrame レベルで適用したのち VMC OSC packet 列を生成する。
		enrich_frame_with_signal_derived_motion(frame);
		self.modifier_pipeline.apply(frame);
		let mut packets = if self.config.send_ok_packet {
			vmc_packets_for_frame(frame)
		} else {
			vmc_packets_for_frame_without_ok(frame)
		};
		retain_vmc_packets_for_modifier(&mut packets, &self.config.modifier);
		self.emit_osc_packets(packets)
	}

	fn send_borrowed_unmotion_frame(&mut self, frame: &UNMotionFrame) -> anyhow::Result<VmcOutputEvent> {
		let packets = if self.config.send_ok_packet {
			vmc_packets_for_frame(frame)
		} else {
			vmc_packets_for_frame_without_ok(frame)
		};
		self.emit_osc_packets(packets)
	}

	fn emit_osc_packets(&mut self, packets: Vec<OscPacket>) -> anyhow::Result<VmcOutputEvent> {
		if packets.is_empty() {
			return Ok(VmcOutputEvent::Sent {
				target_addr: self.config.target_addr,
				datagrams: 0,
				packets: 0,
			});
		}
		let packet_count = packets.len() as u64;
		let encoded = encoder::encode(&OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content: packets,
		}))
		.context("VMC OSC encode failed")?;
		self.socket
			.send_to(&encoded, self.config.target_addr)
			.with_context(|| format!("VMC UDP send failed: {}", self.config.target_addr))?;
		self.stats.sent_datagrams += 1;
		self.stats.sent_packets += packet_count;
		Ok(VmcOutputEvent::Sent {
			target_addr: self.config.target_addr,
			datagrams: 1,
			packets: packet_count,
		})
	}

	pub fn stopped_event(&self) -> VmcOutputEvent {
		VmcOutputEvent::Stopped {
			target_addr: self.config.target_addr,
			stats: self.stats.clone(),
		}
	}
}

fn retain_vmc_packets_for_modifier(packets: &mut Vec<OscPacket>, modifier: &ModifierConfig) {
	packets.retain_mut(|packet| vmc_packet_enabled(packet, modifier));
}

fn vmc_packet_enabled(packet: &mut OscPacket, modifier: &ModifierConfig) -> bool {
	match packet {
		OscPacket::Message(message) => vmc_message_enabled(message, modifier),
		OscPacket::Bundle(bundle) => {
			bundle.content.retain_mut(|packet| vmc_packet_enabled(packet, modifier));
			!bundle.content.is_empty()
		}
	}
}

fn vmc_message_enabled(message: &OscMessage, modifier: &ModifierConfig) -> bool {
	match message.addr.as_str() {
		"/VMC/Ext/Root/Pos" => modifier.torso_enabled,
		"/VMC/Ext/Bone/Pos" => {
			let Some(OscType::String(name)) = message.args.first() else {
				return true;
			};
			vmc_bone_name_to_humanoid_bone(name).is_none_or(|bone| modifier.bone_enabled(bone))
		}
		"/VMC/Ext/Blend/Val" | "/VMC/Ext/Blend/Apply" => modifier.face_enabled,
		_ => true,
	}
}

pub struct VmcOutputWorkerHandle {
	pub target_addr: SocketAddr,
	command_tx: Sender<VmcOutputCommand>,
	join: Option<JoinHandle<()>>,
}

impl VmcOutputWorkerHandle {
	pub fn send(&self, frame: impl Into<VmcOutputFrame>) -> Result<(), std::sync::mpsc::SendError<VmcOutputCommand>> {
		self.command_tx.send(VmcOutputCommand::Send(frame.into()))
	}

	pub fn shutdown(&self) {
		let _ = self.command_tx.send(VmcOutputCommand::Shutdown);
	}

	pub fn join(mut self) -> thread::Result<()> {
		self.shutdown();
		if let Some(join) = self.join.take() { join.join() } else { Ok(()) }
	}
}

impl Drop for VmcOutputWorkerHandle {
	fn drop(&mut self) {
		let _ = self.command_tx.send(VmcOutputCommand::Shutdown);
	}
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FileOutputFormat {
	#[default]
	UnMotionFrameMessagePack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOutputConfig {
	pub path: PathBuf,
	pub format: FileOutputFormat,
	pub create_parent_dirs: bool,
	pub flush_each_frame: bool,
}

impl FileOutputConfig {
	pub fn new(path: impl Into<PathBuf>) -> Self {
		Self {
			path: path.into(),
			format: FileOutputFormat::UnMotionFrameMessagePack,
			create_parent_dirs: true,
			flush_each_frame: false,
		}
	}

	pub fn with_flush_each_frame(mut self, enabled: bool) -> Self {
		self.flush_each_frame = enabled;
		self
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileOutputCommand {
	WriteFrame(UNMotionFrame),
	Flush,
	Shutdown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileOutputStats {
	pub written_frames: u64,
	pub flushes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOutputEvent {
	Written { path: PathBuf, frames: u64 },
	Flushed { path: PathBuf, flushes: u64 },
	Error { path: PathBuf, message: String },
	Stopped { path: PathBuf, stats: FileOutputStats },
}

pub struct FileOutputWorker {
	config: FileOutputConfig,
	recorder: MessagePackStreamRecorder<BufWriter<File>>,
	stats: FileOutputStats,
}

impl FileOutputWorker {
	pub fn open(config: FileOutputConfig) -> anyhow::Result<Self> {
		if config.create_parent_dirs
			&& let Some(parent) = config.path.parent()
			&& !parent.as_os_str().is_empty()
		{
			fs::create_dir_all(parent).with_context(|| format!("failed to create file output directory {}", parent.display()))?;
		}
		let file = File::create(&config.path).with_context(|| format!("failed to create file output {}", config.path.display()))?;
		Ok(Self {
			config,
			recorder: MessagePackStreamRecorder::new(BufWriter::new(file)),
			stats: FileOutputStats::default(),
		})
	}

	pub fn path(&self) -> &PathBuf {
		&self.config.path
	}

	pub fn stats(&self) -> &FileOutputStats {
		&self.stats
	}

	pub fn write_frame(&mut self, frame: &UNMotionFrame) -> anyhow::Result<FileOutputEvent> {
		match self.config.format {
			FileOutputFormat::UnMotionFrameMessagePack => {
				self.recorder
					.write_frame(frame)
					.with_context(|| format!("failed to write UNMotionFrame file output {}", self.config.path.display()))?;
			}
		}
		self.stats.written_frames += 1;
		if self.config.flush_each_frame {
			self.flush()?;
		}
		Ok(FileOutputEvent::Written {
			path: self.config.path.clone(),
			frames: 1,
		})
	}

	pub fn flush(&mut self) -> anyhow::Result<FileOutputEvent> {
		self.recorder
			.flush()
			.with_context(|| format!("failed to flush file output {}", self.config.path.display()))?;
		self.stats.flushes += 1;
		Ok(FileOutputEvent::Flushed {
			path: self.config.path.clone(),
			flushes: self.stats.flushes,
		})
	}

	pub fn stopped_event(&self) -> FileOutputEvent {
		FileOutputEvent::Stopped {
			path: self.config.path.clone(),
			stats: self.stats.clone(),
		}
	}
}

pub struct FileOutputWorkerHandle {
	pub path: PathBuf,
	command_tx: Sender<FileOutputCommand>,
	join: Option<JoinHandle<()>>,
}

impl FileOutputWorkerHandle {
	pub fn write_frame(&self, frame: UNMotionFrame) -> Result<(), std::sync::mpsc::SendError<FileOutputCommand>> {
		self.command_tx.send(FileOutputCommand::WriteFrame(frame))
	}

	pub fn flush(&self) -> Result<(), std::sync::mpsc::SendError<FileOutputCommand>> {
		self.command_tx.send(FileOutputCommand::Flush)
	}

	pub fn shutdown(&self) {
		let _ = self.command_tx.send(FileOutputCommand::Shutdown);
	}

	pub fn join(mut self) -> thread::Result<()> {
		self.shutdown();
		if let Some(join) = self.join.take() { join.join() } else { Ok(()) }
	}
}

impl Drop for FileOutputWorkerHandle {
	fn drop(&mut self) {
		let _ = self.command_tx.send(FileOutputCommand::Shutdown);
	}
}

pub fn spawn_vmc_output_worker(config: VmcOutputConfig, event_tx: Sender<VmcOutputEvent>) -> anyhow::Result<VmcOutputWorkerHandle> {
	let mut worker = VmcOutputWorker::bind(config)?;
	let target_addr = worker.target_addr();
	info!(
		target: "un_motion_runtime::vmc_output",
		target_addr = %target_addr,
		"VMC output worker bound",
	);
	let (command_tx, command_rx) = mpsc::channel();
	let join = thread::spawn(move || run_vmc_output_worker(&mut worker, command_rx, event_tx));
	Ok(VmcOutputWorkerHandle {
		target_addr,
		command_tx,
		join: Some(join),
	})
}

#[derive(Clone, Debug, PartialEq)]
pub struct VrcOscOutputConfig {
	pub target_addr: SocketAddr,
	pub parameter_prefix: String,
	pub send_only_when_vrchat_running: bool,
	pub process_poll_interval: Duration,
	pub modifier: ModifierConfig,
}

impl VrcOscOutputConfig {
	pub fn new(target_addr: SocketAddr) -> Self {
		Self {
			target_addr,
			parameter_prefix: String::new(),
			send_only_when_vrchat_running: true,
			process_poll_interval: Duration::from_secs(10),
			modifier: ModifierConfig::default(),
		}
	}

	pub fn with_parameter_prefix(mut self, prefix: impl Into<String>) -> Self {
		self.parameter_prefix = prefix.into();
		self
	}

	pub fn with_process_gate(mut self, enabled: bool, poll_interval: Duration) -> Self {
		self.send_only_when_vrchat_running = enabled;
		self.process_poll_interval = poll_interval.max(Duration::from_secs(1));
		self
	}

	pub fn with_modifier(mut self, modifier: ModifierConfig) -> Self {
		self.modifier = modifier;
		self
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum VrcOscOutputFrame {
	UnmotionFrame(UNMotionFrame),
	SharedUnmotionFrame(Arc<UNMotionFrame>),
}

impl From<UNMotionFrame> for VrcOscOutputFrame {
	fn from(frame: UNMotionFrame) -> Self {
		Self::UnmotionFrame(frame)
	}
}

impl From<Arc<UNMotionFrame>> for VrcOscOutputFrame {
	fn from(frame: Arc<UNMotionFrame>) -> Self {
		Self::SharedUnmotionFrame(frame)
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum VrcOscOutputCommand {
	Send(VrcOscOutputFrame),
	Shutdown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VrcOscOutputStats {
	pub sent_datagrams: u64,
	pub sent_packets: u64,
	pub skipped_frames: u64,
	pub process_gate_blocked_frames: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VrcOscOutputEvent {
	Sent {
		target_addr: SocketAddr,
		datagrams: u64,
		packets: u64,
		vrchat_detected: bool,
	},
	Skipped {
		target_addr: SocketAddr,
		process_gate_blocked: bool,
		vrchat_detected: bool,
	},
	Error {
		target_addr: SocketAddr,
		message: String,
	},
	Stopped {
		target_addr: SocketAddr,
		stats: VrcOscOutputStats,
	},
}

pub struct VrcOscOutputWorker {
	config: VrcOscOutputConfig,
	modifier_pipeline: ModifierPipeline,
	sink: VrcOscOutputSink,
	stats: VrcOscOutputStats,
	last_process_poll: Option<Instant>,
	vrchat_detected: bool,
}

impl VrcOscOutputWorker {
	pub fn bind(config: VrcOscOutputConfig) -> anyhow::Result<Self> {
		let sink = VrcOscOutputSink::new(config.target_addr)?.with_options(VrcOscOutputOptions {
			parameter_prefix: config.parameter_prefix.clone(),
			..VrcOscOutputOptions::default()
		});
		Ok(Self {
			modifier_pipeline: ModifierPipeline::from_config(&config.modifier),
			config,
			sink,
			stats: VrcOscOutputStats::default(),
			last_process_poll: None,
			vrchat_detected: false,
		})
	}

	pub fn target_addr(&self) -> SocketAddr {
		self.config.target_addr
	}

	pub fn stats(&self) -> &VrcOscOutputStats {
		&self.stats
	}

	pub fn vrchat_detected(&self) -> bool {
		self.vrchat_detected
	}

	pub fn send_frame(&mut self, frame: VrcOscOutputFrame) -> anyhow::Result<VrcOscOutputEvent> {
		if !self.process_gate_allows_send() {
			self.stats.skipped_frames = self.stats.skipped_frames.saturating_add(1);
			self.stats.process_gate_blocked_frames = self.stats.process_gate_blocked_frames.saturating_add(1);
			return Ok(VrcOscOutputEvent::Skipped {
				target_addr: self.config.target_addr,
				process_gate_blocked: true,
				vrchat_detected: self.vrchat_detected,
			});
		}
		match frame {
			VrcOscOutputFrame::UnmotionFrame(mut frame) => self.send_unmotion_frame(&mut frame),
			VrcOscOutputFrame::SharedUnmotionFrame(frame) => {
				let mut frame = (*frame).clone();
				self.send_unmotion_frame(&mut frame)
			}
		}
	}

	fn send_unmotion_frame(&mut self, frame: &mut UNMotionFrame) -> anyhow::Result<VrcOscOutputEvent> {
		self.modifier_pipeline.apply(frame);
		let (datagrams, packets) = self.sink.send(frame)?;
		if datagrams == 0 || packets == 0 {
			self.stats.skipped_frames = self.stats.skipped_frames.saturating_add(1);
			return Ok(VrcOscOutputEvent::Skipped {
				target_addr: self.config.target_addr,
				process_gate_blocked: false,
				vrchat_detected: self.vrchat_detected,
			});
		}
		self.stats.sent_datagrams = self.stats.sent_datagrams.saturating_add(datagrams);
		self.stats.sent_packets = self.stats.sent_packets.saturating_add(packets);
		Ok(VrcOscOutputEvent::Sent {
			target_addr: self.config.target_addr,
			datagrams,
			packets,
			vrchat_detected: self.vrchat_detected,
		})
	}

	fn process_gate_allows_send(&mut self) -> bool {
		if !self.config.send_only_when_vrchat_running {
			return true;
		}
		let now = Instant::now();
		let should_poll = self
			.last_process_poll
			.is_none_or(|last| now.duration_since(last) >= self.config.process_poll_interval);
		if should_poll {
			self.vrchat_detected = vrchat_process_is_running();
			self.last_process_poll = Some(now);
		}
		self.vrchat_detected
	}

	pub fn stopped_event(&self) -> VrcOscOutputEvent {
		VrcOscOutputEvent::Stopped {
			target_addr: self.config.target_addr,
			stats: self.stats.clone(),
		}
	}
}

pub struct VrcOscOutputWorkerHandle {
	pub target_addr: SocketAddr,
	command_tx: Sender<VrcOscOutputCommand>,
	join: Option<JoinHandle<()>>,
}

impl VrcOscOutputWorkerHandle {
	pub fn send(&self, frame: impl Into<VrcOscOutputFrame>) -> Result<(), std::sync::mpsc::SendError<VrcOscOutputCommand>> {
		self.command_tx.send(VrcOscOutputCommand::Send(frame.into()))
	}

	pub fn shutdown(&self) {
		let _ = self.command_tx.send(VrcOscOutputCommand::Shutdown);
	}

	pub fn join(mut self) -> thread::Result<()> {
		self.shutdown();
		if let Some(join) = self.join.take() { join.join() } else { Ok(()) }
	}
}

impl Drop for VrcOscOutputWorkerHandle {
	fn drop(&mut self) {
		let _ = self.command_tx.send(VrcOscOutputCommand::Shutdown);
	}
}

pub fn spawn_vrc_osc_output_worker(
	config: VrcOscOutputConfig,
	event_tx: Sender<VrcOscOutputEvent>,
) -> anyhow::Result<VrcOscOutputWorkerHandle> {
	let mut worker = VrcOscOutputWorker::bind(config)?;
	let target_addr = worker.target_addr();
	info!(
		target: "un_motion_runtime::vrc_osc_output",
		target_addr = %target_addr,
		"VRC OSC output worker bound",
	);
	let (command_tx, command_rx) = mpsc::channel();
	let join = thread::spawn(move || run_vrc_osc_output_worker(&mut worker, command_rx, event_tx));
	Ok(VrcOscOutputWorkerHandle {
		target_addr,
		command_tx,
		join: Some(join),
	})
}

fn run_vrc_osc_output_worker(
	worker: &mut VrcOscOutputWorker,
	command_rx: Receiver<VrcOscOutputCommand>,
	event_tx: Sender<VrcOscOutputEvent>,
) {
	const SEND_LOG_INTERVAL: u64 = 30;
	let target_addr = worker.target_addr();
	for command in command_rx {
		match command {
			VrcOscOutputCommand::Send(frame) => match worker.send_frame(frame) {
				Ok(event) => {
					let stats = worker.stats();
					if stats.sent_datagrams == 1 || stats.sent_datagrams % SEND_LOG_INTERVAL == 0 {
						debug!(
							target: "un_motion_runtime::vrc_osc_output",
							target_addr = %target_addr,
							sent_datagrams = stats.sent_datagrams,
							sent_packets = stats.sent_packets,
							skipped_frames = stats.skipped_frames,
							vrchat_detected = worker.vrchat_detected(),
							"VRC OSC output event",
						);
					}
					let _ = event_tx.send(event);
				}
				Err(error) => {
					warn!(
						target: "un_motion_runtime::vrc_osc_output",
						target_addr = %target_addr,
						error = %error,
						"VRC OSC datagram send failed",
					);
					let _ = event_tx.send(VrcOscOutputEvent::Error {
						target_addr,
						message: error.to_string(),
					});
				}
			},
			VrcOscOutputCommand::Shutdown => break,
		}
	}
	info!(
		target: "un_motion_runtime::vrc_osc_output",
		target_addr = %target_addr,
		sent_datagrams = worker.stats().sent_datagrams,
		sent_packets = worker.stats().sent_packets,
		skipped_frames = worker.stats().skipped_frames,
		"VRC OSC output worker stopped",
	);
	let _ = event_tx.send(worker.stopped_event());
}

fn vrchat_process_is_running() -> bool {
	#[cfg(windows)]
	{
		let Ok(output) = Command::new("tasklist").args(["/FI", "IMAGENAME eq VRChat.exe", "/NH"]).output() else {
			return false;
		};
		String::from_utf8_lossy(&output.stdout)
			.lines()
			.any(|line| line.trim_start().to_ascii_lowercase().starts_with("vrchat.exe"))
	}
	#[cfg(not(windows))]
	{
		Command::new("pgrep")
			.args(["-x", "VRChat"])
			.output()
			.map(|output| output.status.success())
			.unwrap_or(false)
	}
}

pub fn spawn_file_output_worker(config: FileOutputConfig, event_tx: Sender<FileOutputEvent>) -> anyhow::Result<FileOutputWorkerHandle> {
	let mut worker = FileOutputWorker::open(config)?;
	let path = worker.path().clone();
	let (command_tx, command_rx) = mpsc::channel();
	let join = thread::spawn(move || run_file_output_worker(&mut worker, command_rx, event_tx));
	Ok(FileOutputWorkerHandle {
		path,
		command_tx,
		join: Some(join),
	})
}

fn run_vmc_output_worker(worker: &mut VmcOutputWorker, command_rx: Receiver<VmcOutputCommand>, event_tx: Sender<VmcOutputEvent>) {
	// Phase E e2e Step B: 1 秒に 1 回程度 (30 fps 想定で 30 datagram 毎) で send が
	// 進んでいるか debug log に流す。「送信側で詰まっているか」の切り分けに使う。
	const SEND_LOG_INTERVAL: u64 = 30;
	let target_addr = worker.target_addr();
	for command in command_rx {
		match command {
			VmcOutputCommand::Send(frame) => match worker.send_frame(frame) {
				Ok(event) => {
					let stats = worker.stats();
					if stats.sent_datagrams == 1 || stats.sent_datagrams % SEND_LOG_INTERVAL == 0 {
						debug!(
							target: "un_motion_runtime::vmc_output",
							target_addr = %target_addr,
							sent_datagrams = stats.sent_datagrams,
							sent_packets = stats.sent_packets,
							"VMC datagram sent",
						);
					}
					let _ = event_tx.send(event);
				}
				Err(error) => {
					warn!(
						target: "un_motion_runtime::vmc_output",
						target_addr = %target_addr,
						error = %error,
						"VMC datagram send failed",
					);
					let _ = event_tx.send(VmcOutputEvent::Error {
						target_addr,
						message: error.to_string(),
					});
				}
			},
			VmcOutputCommand::Shutdown => break,
		}
	}
	info!(
		target: "un_motion_runtime::vmc_output",
		target_addr = %target_addr,
		sent_datagrams = worker.stats().sent_datagrams,
		sent_packets = worker.stats().sent_packets,
		"VMC output worker stopped",
	);
	let _ = event_tx.send(worker.stopped_event());
}

fn run_file_output_worker(worker: &mut FileOutputWorker, command_rx: Receiver<FileOutputCommand>, event_tx: Sender<FileOutputEvent>) {
	for command in command_rx {
		match command {
			FileOutputCommand::WriteFrame(frame) => match worker.write_frame(&frame) {
				Ok(event) => {
					let _ = event_tx.send(event);
				}
				Err(error) => {
					let _ = event_tx.send(FileOutputEvent::Error {
						path: worker.path().clone(),
						message: error.to_string(),
					});
				}
			},
			FileOutputCommand::Flush => match worker.flush() {
				Ok(event) => {
					let _ = event_tx.send(event);
				}
				Err(error) => {
					let _ = event_tx.send(FileOutputEvent::Error {
						path: worker.path().clone(),
						message: error.to_string(),
					});
				}
			},
			FileOutputCommand::Shutdown => break,
		}
	}
	if let Err(error) = worker.flush() {
		let _ = event_tx.send(FileOutputEvent::Error {
			path: worker.path().clone(),
			message: error.to_string(),
		});
	}
	let _ = event_tx.send(worker.stopped_event());
}

#[cfg(test)]
mod tests {
	use std::fs::{self, File};
	use std::net::UdpSocket;
	use std::sync::mpsc;
	use std::time::{Duration, SystemTime, UNIX_EPOCH};

	use rosc::{OscMessage, OscPacket, OscType, decoder};
	use un_motion_frame::UNMotionFrame;
	use un_motion_record::decode_framed_stream;

	use super::*;

	fn temp_record_path(label: &str) -> PathBuf {
		let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("system time").as_nanos();
		std::env::temp_dir().join(format!("un-motion-runtime-{label}-{}-{nonce}.unmf", std::process::id()))
	}

	fn recv_messages(receiver: &UdpSocket) -> Vec<OscMessage> {
		let mut buf = [0_u8; 65535];
		let (len, _) = receiver.recv_from(&mut buf).expect("recv");
		let (_, packet) = decoder::decode_udp(&buf[..len]).expect("decode");
		let mut messages = Vec::new();
		collect_messages(packet, &mut messages);
		messages
	}

	fn collect_messages(packet: OscPacket, messages: &mut Vec<OscMessage>) {
		match packet {
			OscPacket::Message(message) => messages.push(message),
			OscPacket::Bundle(bundle) => {
				for packet in bundle.content {
					collect_messages(packet, messages);
				}
			}
		}
	}

	fn message_with_name<'a>(messages: &'a [OscMessage], addr: &str, name: &str) -> &'a OscMessage {
		messages
			.iter()
			.find(|message| message.addr == addr && message.args.first() == Some(&OscType::String(name.to_string())))
			.expect("message")
	}

	#[test]
	fn spawned_output_worker_reports_sent_and_stopped_events() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let target = receiver.local_addr().expect("target");
		let (event_tx, event_rx) = mpsc::channel();
		let handle = spawn_vmc_output_worker(VmcOutputConfig::new(target), event_tx).expect("spawn output");

		handle.send(UNMotionFrame::new(1)).expect("send command");
		let sent = event_rx.recv_timeout(Duration::from_secs(1)).expect("sent event");
		handle.shutdown();
		let stopped = event_rx.recv_timeout(Duration::from_secs(1)).expect("stopped event");

		assert!(matches!(
			sent,
			VmcOutputEvent::Sent {
				datagrams: 1,
				packets: 1,
				..
			}
		));
		assert!(matches!(stopped, VmcOutputEvent::Stopped { stats, .. } if stats.sent_datagrams == 1));
		assert_eq!(recv_messages(&receiver)[0].addr, "/VMC/Ext/OK");
	}

	#[test]
	fn vmc_output_sends_unmotion_frame_directly() {
		// Phase E-α-4: UNMotionFrame variant を経由しても VMC OSC bundle が生成され、
		// receiver 側で `/VMC/Ext/OK` および bone packet が受信できることを確認する。
		use un_motion_frame::{
			BodyMotion, BoneSample, HumanoidBone, HumanoidPose, SampleState, TrackingState, TransformSample as FrameTransform,
		};

		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let target = receiver.local_addr().expect("target");
		let mut worker = VmcOutputWorker::bind(VmcOutputConfig::new(target)).expect("worker");

		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![BoneSample {
					bone: HumanoidBone::Head,
					transform: FrameTransform {
						translation: Some(un_motion_frame::Vec3f { x: 0.0, y: 0.0, z: 0.0 }),
						rotation: Some(un_motion_frame::Quatf {
							x: 0.0,
							y: 0.0,
							z: 0.0,
							w: 1.0,
						}),
						scale: None,
						linear_velocity: None,
						angular_velocity: None,
					},
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				}],
			}),
		});

		let event = worker
			.send_frame(VmcOutputFrame::UnmotionFrame(frame))
			.expect("send unmotion frame");
		assert!(matches!(event, VmcOutputEvent::Sent { datagrams: 1, .. }));

		let messages = recv_messages(&receiver);
		assert!(messages.iter().any(|m| m.addr == "/VMC/Ext/OK"));
		// `vmc_packets_for_frame` は `direct_humanoid_pose_packets` を呼び、Head のような
		// bone について `/VMC/Ext/Bone/Pos` を出す。
		assert!(messages.iter().any(|m| m.addr == "/VMC/Ext/Bone/Pos"));
	}

	#[test]
	fn vrc_osc_output_sends_face_parameter_when_process_gate_disabled() {
		use un_motion_frame::{ExpressionSample, FaceMotion, SampleState, TrackingState};

		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let target = receiver.local_addr().expect("target");
		let mut worker =
			VrcOscOutputWorker::bind(VrcOscOutputConfig::new(target).with_process_gate(false, Duration::from_secs(10))).expect("worker");

		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "jawOpen".to_string(),
				value: 0.5,
				confidence: 1.0,
				source_index: None,
				state: SampleState::Valid,
			}],
		});

		let event = worker
			.send_frame(VrcOscOutputFrame::UnmotionFrame(frame))
			.expect("send vrc osc frame");
		assert!(matches!(
			event,
			VrcOscOutputEvent::Sent {
				datagrams: 1,
				packets: 5,
				..
			}
		));

		let messages = recv_messages(&receiver);
		assert!(
			messages
				.iter()
				.any(|m| m.addr == "/avatar/parameters/v2/JawOpen" && m.args.first() == Some(&OscType::Float(0.5)))
		);
	}

	#[test]
	fn vrc_osc_output_reports_skipped_empty_frame() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		let target = receiver.local_addr().expect("target");
		let mut worker =
			VrcOscOutputWorker::bind(VrcOscOutputConfig::new(target).with_process_gate(false, Duration::from_secs(10))).expect("worker");

		let event = worker
			.send_frame(VrcOscOutputFrame::UnmotionFrame(UNMotionFrame::new(1)))
			.expect("send empty frame");

		assert!(matches!(
			event,
			VrcOscOutputEvent::Skipped {
				process_gate_blocked: false,
				..
			}
		));
		assert_eq!(worker.stats().skipped_frames, 1);
	}

	#[test]
	fn vmc_output_worker_keeps_smoothing_state_between_frames() {
		use crate::{SmoothingConfig, SmoothingPreset};
		use un_motion_frame::{
			BodyMotion, BoneSample, HumanoidBone, HumanoidPose, Quatf, SampleState, TrackingState, TransformSample as FrameTransform, Vec3f,
		};

		fn frame_with_left_hand(sequence: u64, rotation: Quatf) -> UNMotionFrame {
			let mut frame = UNMotionFrame::new(sequence);
			frame.header.frame_timestamp_ns = sequence * 16_666_667;
			frame.body = Some(BodyMotion {
				tracking_state: TrackingState::Valid,
				confidence: 1.0,
				humanoid: Some(HumanoidPose {
					root: None,
					bones: vec![BoneSample {
						bone: HumanoidBone::LeftHand,
						transform: FrameTransform {
							translation: Some(Vec3f { x: 0.0, y: 0.0, z: 0.0 }),
							rotation: Some(rotation),
							scale: None,
							linear_velocity: None,
							angular_velocity: None,
						},
						confidence: 1.0,
						source_index: Some(0),
						state: SampleState::Valid,
					}],
				}),
			});
			frame
		}

		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let target = receiver.local_addr().expect("target");
		let modifier = ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		};
		let mut worker = VmcOutputWorker::bind(VmcOutputConfig::new(target).with_ok_packet(false).with_modifier(modifier)).expect("worker");

		worker
			.send_frame(VmcOutputFrame::UnmotionFrame(frame_with_left_hand(
				1,
				Quatf {
					x: 0.0,
					y: 0.0,
					z: 0.0,
					w: 1.0,
				},
			)))
			.expect("send first");
		let _ = recv_messages(&receiver);

		worker
			.send_frame(VmcOutputFrame::UnmotionFrame(frame_with_left_hand(
				2,
				Quatf {
					x: 1.0,
					y: 0.0,
					z: 0.0,
					w: 0.0,
				},
			)))
			.expect("send second");
		let messages = recv_messages(&receiver);
		let left_hand = message_with_name(&messages, "/VMC/Ext/Bone/Pos", "LeftHand");
		let OscType::Float(x) = left_hand.args[4] else {
			panic!("LeftHand rotation x should be float");
		};
		assert!(x > 0.1 && x < 0.95, "expected smoothed rotation x, got raw-ish x={x}");
	}

	#[test]
	fn vmc_output_worker_smooths_signal_derived_finger_bones() {
		use crate::{SmoothingConfig, SmoothingPreset};
		use un_motion_frame::{MotionSignal, MotionSignalValue, SampleState};

		fn scalar(name: &str, value: f32) -> MotionSignal {
			MotionSignal {
				name: name.to_string(),
				value: MotionSignalValue::Scalar(value),
				confidence: 1.0,
				source_index: None,
				state: SampleState::Valid,
			}
		}

		fn finger_frame(sequence: u64, curl: f32) -> UNMotionFrame {
			let mut frame = UNMotionFrame::new(sequence);
			frame.header.frame_timestamp_ns = sequence * 16_666_667;
			frame.signals.push(scalar("hand.right.index.curl", curl));
			frame.signals.push(scalar("hand.right.index.mcp.curl", curl));
			frame.signals.push(scalar("hand.right.index.pip.curl", curl));
			frame.signals.push(scalar("hand.right.index.dip.curl", curl));
			frame
		}

		fn right_index_intermediate_z(messages: &[OscMessage]) -> f32 {
			let message = message_with_name(messages, "/VMC/Ext/Bone/Pos", "RightIndexIntermediate");
			let OscType::Float(z) = message.args[6] else {
				panic!("RightIndexIntermediate rotation z should be float");
			};
			z
		}

		let smooth_receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		smooth_receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let modifier = ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		};
		let mut smooth_worker = VmcOutputWorker::bind(
			VmcOutputConfig::new(smooth_receiver.local_addr().expect("target"))
				.with_ok_packet(false)
				.with_modifier(modifier),
		)
		.expect("worker");
		smooth_worker
			.send_frame(VmcOutputFrame::UnmotionFrame(finger_frame(1, 0.0)))
			.expect("send first");
		let _ = recv_messages(&smooth_receiver);
		smooth_worker
			.send_frame(VmcOutputFrame::UnmotionFrame(finger_frame(2, 1.0)))
			.expect("send second");
		let smoothed = right_index_intermediate_z(&recv_messages(&smooth_receiver)).abs();

		let raw_receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		raw_receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let mut raw_worker =
			VmcOutputWorker::bind(VmcOutputConfig::new(raw_receiver.local_addr().expect("target")).with_ok_packet(false)).expect("worker");
		raw_worker
			.send_frame(VmcOutputFrame::UnmotionFrame(finger_frame(2, 1.0)))
			.expect("send raw");
		let raw = right_index_intermediate_z(&recv_messages(&raw_receiver)).abs();

		assert!(smoothed > 0.01, "expected smoothed finger to move");
		assert!(
			smoothed < raw * 0.95,
			"expected smoothing to attenuate signal-derived finger bone: smoothed={smoothed} raw={raw}"
		);
	}

	#[test]
	fn vmc_output_unmotion_frame_respects_send_ok_packet_false() {
		// `send_ok_packet=false` のときは `/VMC/Ext/OK` を含めない。
		use un_motion_frame::{BodyMotion, HumanoidPose, TrackingState};

		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let target = receiver.local_addr().expect("target");
		let mut worker = VmcOutputWorker::bind(VmcOutputConfig::new(target).with_ok_packet(false)).expect("worker");

		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose { root: None, bones: vec![] }),
		});

		// 空の body だが OK packet が無い前提なら何も送らない (packets = 0)。
		let event = worker
			.send_frame(VmcOutputFrame::UnmotionFrame(frame))
			.expect("send unmotion frame");
		assert!(matches!(
			event,
			VmcOutputEvent::Sent {
				datagrams: 0,
				packets: 0,
				..
			}
		));
	}

	#[test]
	fn vmc_output_unmotion_frame_filters_signal_fallback_bones() {
		use un_motion_frame::{BodyMotion, HumanoidBone, HumanoidPose, TrackingState};

		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver.set_read_timeout(Some(Duration::from_millis(500))).expect("timeout");
		let target = receiver.local_addr().expect("target");
		let modifier = ModifierConfig {
			head_enabled: false,
			face_enabled: false,
			hands_enabled: true,
			arms_ik_enabled: true,
			torso_enabled: false,
			legs_enabled: false,
			feet_enabled: false,
			..ModifierConfig::default()
		};
		let mut worker = VmcOutputWorker::bind(VmcOutputConfig::new(target).with_ok_packet(false).with_modifier(modifier)).expect("worker");
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![
					filter_test_bone(HumanoidBone::LeftUpperArm),
					filter_test_bone(HumanoidBone::Head),
					filter_test_bone(HumanoidBone::Chest),
					filter_test_bone(HumanoidBone::LeftFoot),
				],
			}),
		});

		worker.send_frame(VmcOutputFrame::UnmotionFrame(frame)).expect("send frame");

		let messages = recv_messages(&receiver);
		assert!(
			messages
				.iter()
				.any(|m| m.addr == "/VMC/Ext/Bone/Pos" && m.args.first() == Some(&OscType::String("LeftUpperArm".to_string())))
		);
		assert!(
			!messages
				.iter()
				.any(|m| m.addr == "/VMC/Ext/Bone/Pos" && m.args.first() == Some(&OscType::String("Head".to_string())))
		);
		assert!(
			!messages
				.iter()
				.any(|m| m.addr == "/VMC/Ext/Bone/Pos" && m.args.first() == Some(&OscType::String("Chest".to_string())))
		);
		assert!(
			!messages
				.iter()
				.any(|m| m.addr == "/VMC/Ext/Bone/Pos" && m.args.first() == Some(&OscType::String("LeftFoot".to_string())))
		);
		assert!(!messages.iter().any(|m| m.addr == "/VMC/Ext/Blend/Val"));
	}

	fn filter_test_bone(bone: un_motion_frame::HumanoidBone) -> un_motion_frame::BoneSample {
		use un_motion_frame::{BoneSample, Quatf, SampleState, TransformSample as FrameTransform, Vec3f};

		BoneSample {
			bone,
			transform: FrameTransform {
				translation: Some(Vec3f { x: 0.0, y: 0.0, z: 0.0 }),
				rotation: Some(Quatf {
					x: 0.0,
					y: 0.0,
					z: 0.0,
					w: 1.0,
				}),
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			},
			confidence: 1.0,
			source_index: Some(0),
			state: SampleState::Valid,
		}
	}

	#[test]
	fn writes_unmotion_frame_file_output() {
		let path = temp_record_path("direct");
		let mut worker = FileOutputWorker::open(FileOutputConfig::new(&path)).expect("file worker");

		worker.write_frame(&UNMotionFrame::new(7)).expect("write 7");
		worker.write_frame(&UNMotionFrame::new(8)).expect("write 8");
		worker.flush().expect("flush");

		let frames = decode_framed_stream(File::open(&path).expect("open output")).expect("decode output");
		assert_eq!(frames.iter().map(|frame| frame.header.sequence).collect::<Vec<_>>(), vec![7, 8]);
		assert_eq!(worker.stats().written_frames, 2);
		let _ = fs::remove_file(path);
	}

	#[test]
	fn spawned_file_output_worker_reports_written_flush_and_stopped_events() {
		let path = temp_record_path("spawned");
		let (event_tx, event_rx) = mpsc::channel();
		let handle = spawn_file_output_worker(FileOutputConfig::new(&path), event_tx).expect("spawn file output");

		handle.write_frame(UNMotionFrame::new(42)).expect("write command");
		let written = event_rx.recv_timeout(Duration::from_secs(1)).expect("written event");
		handle.flush().expect("flush command");
		let flushed = event_rx.recv_timeout(Duration::from_secs(1)).expect("flushed event");
		handle.shutdown();
		let stopped = event_rx.recv_timeout(Duration::from_secs(1)).expect("stopped event");
		handle.join().expect("join");

		assert!(matches!(written, FileOutputEvent::Written { frames: 1, .. }));
		assert!(matches!(flushed, FileOutputEvent::Flushed { flushes: 1, .. }));
		assert!(matches!(stopped, FileOutputEvent::Stopped { stats, .. } if stats.written_frames == 1));
		let frames = decode_framed_stream(File::open(&path).expect("open output")).expect("decode output");
		assert_eq!(frames[0].header.sequence, 42);
		let _ = fs::remove_file(path);
	}
}
