use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};

use un_motion_frame::{
	BodyMotion, BoneSample, CoordinateSpace, EyeMotion, FaceMotion, GazeSample, Handedness, HumanoidBone, HumanoidPose, LengthUnit,
	MotionHeader, MotionSignal, MotionSignalValue, MotionSourceInfo, MotionSourceKind, Quatf, SampleState, TimestampBasis, TrackingState,
	TransformSample, UNMotionFrame, Vec3f,
};

pub const IFACIALMOCAP_UDP_PORT: u16 = 49983;
pub const IFACIALMOCAP_TCP_PORT: u16 = 49986;
const TCP_REASSEMBLY_MAX_BYTES: usize = 64 * 1024;
const TCP_FRAME_DELIMITER: &str = "___iFacialMocap";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfacialMocapTransport {
	Udp,
	Tcp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfacialMocapInputConfig {
	pub source_id: String,
	pub bind_addr: SocketAddr,
	pub remote_addr: Option<SocketAddr>,
	pub transport: IfacialMocapTransport,
	pub start_command: Option<String>,
}

impl IfacialMocapInputConfig {
	pub fn udp(bind_addr: SocketAddr) -> Self {
		Self {
			source_id: "ifacialmocap:udp".to_string(),
			bind_addr,
			remote_addr: None,
			transport: IfacialMocapTransport::Udp,
			start_command: Some("iFacialMocap_sahne".to_string()),
		}
	}

	pub fn tcp(remote_addr: SocketAddr) -> Self {
		Self {
			source_id: "ifacialmocap:tcp".to_string(),
			bind_addr: "0.0.0.0:0".parse().expect("valid wildcard addr"),
			remote_addr: Some(remote_addr),
			transport: IfacialMocapTransport::Tcp,
			start_command: None,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EulerDeg {
	pub yaw: f32,
	pub pitch: f32,
	pub roll: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfacialMocapFrame {
	pub source_id: String,
	pub received_timestamp_ns: u64,
	pub head: EulerDeg,
	pub left_eye: EulerDeg,
	pub right_eye: EulerDeg,
	pub confidence: f32,
	pub expressions: Vec<(String, f32)>,
}

impl IfacialMocapFrame {
	pub fn to_unmotion_frame(&self, sequence: u64) -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(sequence);
		frame.header = MotionHeader {
			magic: MotionHeader::MAGIC,
			version_major: 1,
			version_minor: 1,
			sequence,
			timestamp_basis: TimestampBasis::UnixEpoch,
			capture_timestamp_ns: self.received_timestamp_ns,
			frame_timestamp_ns: self.received_timestamp_ns,
			processed_timestamp_ns: now_unix_ns(),
			coordinate_space: CoordinateSpace::UNMotion,
			handedness: Handedness::Unknown,
			length_unit: LengthUnit::Normalized,
			stream_id: Some(self.source_id.clone()),
			expected_dt_ns: None,
		};
		frame.sources.push(MotionSourceInfo {
			source_id: self.source_id.clone(),
			source_kind: MotionSourceKind::FaceId,
			display_name: Some("iFacialMocap".to_string()),
			confidence: self.confidence,
			latency_ns: None,
			state: tracking_state(self.confidence),
		});

		let source_index = Some(0);
		frame.face = Some(FaceMotion {
			tracking_state: tracking_state(self.confidence),
			confidence: self.confidence,
			head: Some(transform_from_euler(self.head)),
			expressions: self
				.expressions
				.iter()
				.map(|(name, value)| un_motion_frame::ExpressionSample {
					name: name.clone(),
					value: value.clamp(0.0, 1.0),
					confidence: self.confidence,
					source_index,
					state: SampleState::Valid,
				})
				.collect(),
		});

		frame.eyes = Some(EyeMotion {
			tracking_state: tracking_state(self.confidence),
			confidence: self.confidence,
			left_gaze: Some(gaze_from_euler(self.left_eye, self.confidence)),
			right_gaze: Some(gaze_from_euler(self.right_eye, self.confidence)),
			combined_gaze: Some(gaze_from_euler(average_euler(self.left_eye, self.right_eye), self.confidence)),
			blink_left: expression_value(&self.expressions, "eyeBlinkLeft"),
			blink_right: expression_value(&self.expressions, "eyeBlinkRight"),
		});

		frame.body = Some(BodyMotion {
			tracking_state: tracking_state(self.confidence),
			confidence: self.confidence,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![
					bone_sample(HumanoidBone::Head, self.head, self.confidence, source_index),
					bone_sample(HumanoidBone::LeftEye, self.left_eye, self.confidence, source_index),
					bone_sample(HumanoidBone::RightEye, self.right_eye, self.confidence, source_index),
				],
			}),
		});

		push_scalar_signal(&mut frame, "head.yaw", self.head.yaw, self.confidence, source_index);
		push_scalar_signal(&mut frame, "head.pitch", self.head.pitch, self.confidence, source_index);
		push_scalar_signal(&mut frame, "head.roll", self.head.roll, self.confidence, source_index);
		push_scalar_signal(&mut frame, "eye.left.yaw", self.left_eye.yaw, self.confidence, source_index);
		push_scalar_signal(&mut frame, "eye.left.pitch", self.left_eye.pitch, self.confidence, source_index);
		push_scalar_signal(&mut frame, "eye.right.yaw", self.right_eye.yaw, self.confidence, source_index);
		push_scalar_signal(&mut frame, "eye.right.pitch", self.right_eye.pitch, self.confidence, source_index);
		for (name, value) in &self.expressions {
			push_scalar_signal(
				&mut frame,
				format!("face.{name}"),
				value.clamp(0.0, 1.0),
				self.confidence,
				source_index,
			);
		}
		frame.metadata.producer = Some("un-motion-input-ifacialmocap".to_string());
		frame.metadata.notes.push("source_protocol=ifacialmocap".to_string());
		frame
	}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IfacialMocapPollBatch {
	pub frames: Vec<IfacialMocapFrame>,
	pub decode_errors: u64,
	pub decode_error_examples: Vec<String>,
}

pub struct IfacialMocapInputSource {
	config: IfacialMocapInputConfig,
	udp_socket: Option<UdpSocket>,
	tcp_stream: Option<TcpStream>,
	tcp_reassembly_buffer: Vec<u8>,
	receive_buffer: [u8; 8192],
	tcp_read_buffer: [u8; 4096],
}

impl IfacialMocapInputSource {
	pub fn bind(config: IfacialMocapInputConfig) -> anyhow::Result<Self> {
		let mut source = Self {
			config,
			udp_socket: None,
			tcp_stream: None,
			tcp_reassembly_buffer: Vec::with_capacity(4096),
			receive_buffer: [0; 8192],
			tcp_read_buffer: [0; 4096],
		};
		match source.config.transport {
			IfacialMocapTransport::Udp => source.bind_udp()?,
			IfacialMocapTransport::Tcp => source.connect_tcp()?,
		}
		Ok(source)
	}

	pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
		match self.config.transport {
			IfacialMocapTransport::Udp => Ok(self
				.udp_socket
				.as_ref()
				.ok_or_else(|| anyhow::anyhow!("UDP socket is not bound"))?
				.local_addr()?),
			IfacialMocapTransport::Tcp => Ok(self
				.tcp_stream
				.as_ref()
				.ok_or_else(|| anyhow::anyhow!("TCP stream is not connected"))?
				.local_addr()?),
		}
	}

	pub fn source_id(&self) -> &str {
		&self.config.source_id
	}

	pub fn poll_frames(&mut self) -> anyhow::Result<Vec<IfacialMocapFrame>> {
		Ok(self.poll_batch()?.frames)
	}

	pub fn poll_batch(&mut self) -> anyhow::Result<IfacialMocapPollBatch> {
		match self.config.transport {
			IfacialMocapTransport::Udp => Ok(self.poll_udp()),
			IfacialMocapTransport::Tcp => self.poll_tcp(),
		}
	}

	fn bind_udp(&mut self) -> anyhow::Result<()> {
		let socket = UdpSocket::bind(self.config.bind_addr)?;
		socket.set_nonblocking(true)?;
		if let (Some(remote_addr), Some(command)) = (self.config.remote_addr, self.config.start_command.as_deref()) {
			socket.send_to(command.as_bytes(), remote_addr)?;
		}
		self.udp_socket = Some(socket);
		Ok(())
	}

	fn connect_tcp(&mut self) -> anyhow::Result<()> {
		let remote_addr = self
			.config
			.remote_addr
			.ok_or_else(|| anyhow::anyhow!("TCP mode requires remote_addr"))?;
		let stream = TcpStream::connect(remote_addr)?;
		stream.set_nonblocking(true)?;
		self.tcp_stream = Some(stream);
		Ok(())
	}

	fn poll_udp(&mut self) -> IfacialMocapPollBatch {
		let mut batch = IfacialMocapPollBatch::default();
		let Some(socket) = self.udp_socket.as_ref().and_then(|socket| socket.try_clone().ok()) else {
			return batch;
		};

		loop {
			match socket.recv_from(&mut self.receive_buffer) {
				Ok((len, _addr)) => self.parse_payload_bytes(&mut batch, &self.receive_buffer[..len]),
				Err(error) if error.kind() == ErrorKind::WouldBlock => break,
				Err(error) => {
					push_decode_error(&mut batch, format!("UDP receive failed: {error}"));
					break;
				}
			}
		}
		batch
	}

	fn poll_tcp(&mut self) -> anyhow::Result<IfacialMocapPollBatch> {
		let mut batch = IfacialMocapPollBatch::default();
		let Some(mut stream) = self.tcp_stream.as_ref().and_then(|stream| stream.try_clone().ok()) else {
			return Ok(batch);
		};

		loop {
			match stream.read(&mut self.tcp_read_buffer) {
				Ok(0) => {
					push_decode_error(&mut batch, "TCP stream closed by peer".to_string());
					break;
				}
				Ok(len) => {
					self.tcp_reassembly_buffer.extend_from_slice(&self.tcp_read_buffer[..len]);
					self.trim_tcp_reassembly_buffer(&mut batch);
					self.parse_reassembled_tcp_frames(&mut batch);
				}
				Err(error) if error.kind() == ErrorKind::WouldBlock => break,
				Err(error) => {
					push_decode_error(&mut batch, format!("TCP receive failed: {error}"));
					break;
				}
			}
		}
		Ok(batch)
	}

	fn parse_payload_bytes(&self, batch: &mut IfacialMocapPollBatch, bytes: &[u8]) {
		let payload = normalize_payload(bytes);
		if payload.is_empty()
			|| self
				.config
				.start_command
				.as_deref()
				.is_some_and(|command| payload.eq_ignore_ascii_case(command.trim()))
		{
			return;
		}
		match parse_ifacialmocap_frame(payload, &self.config.source_id, now_unix_ns()) {
			Ok(frame) => batch.frames.push(frame),
			Err(error) => push_decode_error(batch, error.to_string()),
		}
	}

	fn trim_tcp_reassembly_buffer(&mut self, batch: &mut IfacialMocapPollBatch) {
		if self.tcp_reassembly_buffer.len() <= TCP_REASSEMBLY_MAX_BYTES {
			return;
		}
		let overflow = self.tcp_reassembly_buffer.len() - TCP_REASSEMBLY_MAX_BYTES;
		self.tcp_reassembly_buffer.drain(..overflow);
		push_decode_error(batch, format!("TCP reassembly buffer overflow: dropped {overflow} byte(s)"));
	}

	fn parse_reassembled_tcp_frames(&mut self, batch: &mut IfacialMocapPollBatch) {
		while let Some(delimiter_start) = find_tcp_frame_delimiter(&self.tcp_reassembly_buffer) {
			let delimiter_len = tcp_delimiter_len(&self.tcp_reassembly_buffer[delimiter_start..]);
			let raw_frame: Vec<u8> = self.tcp_reassembly_buffer.drain(..delimiter_start).collect();
			self.tcp_reassembly_buffer.drain(..delimiter_len);
			self.parse_payload_bytes(batch, &raw_frame);
		}
	}
}

pub fn parse_ifacialmocap_frame(packet: &str, source_id: &str, received_timestamp_ns: u64) -> anyhow::Result<IfacialMocapFrame> {
	let mut head = None;
	let mut left_eye = None;
	let mut right_eye = None;
	let mut confidence = 1.0_f32;
	let mut expressions = Vec::new();

	for segment in packet.split('|') {
		let trimmed = segment.trim();
		if trimmed.is_empty() {
			continue;
		}
		let normalized = trimmed
			.trim_start_matches(|character| matches!(character, '=' | '/' | ':' | ';' | ','))
			.trim();
		let Some((field_name, raw_value)) = normalized.split_once('#') else {
			if let Some((name, value)) = parse_blendshape_segment(normalized) {
				expressions.push((name.to_string(), value));
			}
			continue;
		};
		let field_name = field_name.trim();
		let raw_value = raw_value.trim();

		if field_name.eq_ignore_ascii_case("head") {
			head = Some(parse_triplet(raw_value)?);
		} else if field_name.eq_ignore_ascii_case("leftEye") {
			left_eye = Some(parse_triplet(raw_value)?);
		} else if field_name.eq_ignore_ascii_case("rightEye") {
			right_eye = Some(parse_triplet(raw_value)?);
		} else if field_name.eq_ignore_ascii_case("confidence") {
			confidence = raw_value.parse::<f32>().unwrap_or(confidence).clamp(0.0, 1.0);
		} else if let Ok(value) = raw_value.parse::<f32>() {
			expressions.push((field_name.to_string(), normalize_blendshape_value(value)));
		}
	}

	Ok(IfacialMocapFrame {
		source_id: source_id.to_string(),
		received_timestamp_ns,
		head: head.ok_or_else(|| anyhow::anyhow!("head field is missing"))?,
		left_eye: left_eye.ok_or_else(|| anyhow::anyhow!("leftEye field is missing"))?,
		right_eye: right_eye.ok_or_else(|| anyhow::anyhow!("rightEye field is missing"))?,
		confidence,
		expressions,
	})
}

fn parse_triplet(raw: &str) -> anyhow::Result<EulerDeg> {
	let mut values = raw.split(',').map(str::trim);
	let yaw = parse_next_triplet_value(&mut values, raw, "x")?;
	let pitch = parse_next_triplet_value(&mut values, raw, "y")?;
	let roll = parse_next_triplet_value(&mut values, raw, "z")?;
	Ok(EulerDeg { yaw, pitch, roll })
}

fn parse_next_triplet_value<'a>(values: &mut impl Iterator<Item = &'a str>, raw: &str, label: &str) -> anyhow::Result<f32> {
	values
		.next()
		.ok_or_else(|| anyhow::anyhow!("invalid triplet '{raw}': missing {label}"))?
		.parse::<f32>()
		.map_err(|error| anyhow::anyhow!("invalid triplet '{raw}': {error}"))
}

fn normalize_payload(bytes: &[u8]) -> &str {
	std::str::from_utf8(bytes)
		.unwrap_or("")
		.trim_matches(|character| matches!(character, '\r' | '\n' | '\0'))
		.trim()
}

fn find_tcp_frame_delimiter(bytes: &[u8]) -> Option<usize> {
	if let Some(index) = find_subslice(bytes, TCP_FRAME_DELIMITER.as_bytes()) {
		return Some(index);
	}
	bytes.iter().position(|byte| matches!(*byte, b'\n' | b'\0' | b'\r'))
}

fn tcp_delimiter_len(bytes: &[u8]) -> usize {
	if bytes.starts_with(TCP_FRAME_DELIMITER.as_bytes()) {
		TCP_FRAME_DELIMITER.len()
	} else {
		1
	}
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
	if needle.is_empty() || haystack.len() < needle.len() {
		return None;
	}
	haystack.windows(needle.len()).position(|window| window == needle)
}

fn parse_blendshape_segment(segment: &str) -> Option<(&str, f32)> {
	let (name, raw_value) = if let Some((name, raw_value)) = segment.split_once('&') {
		(name.trim(), raw_value.trim())
	} else {
		let (name, raw_value) = segment.rsplit_once('-')?;
		(name.trim(), raw_value.trim())
	};
	if name.is_empty() {
		return None;
	}
	let value = raw_value.parse::<f32>().ok()?;
	Some((name, normalize_blendshape_value(value)))
}

fn normalize_blendshape_value(value: f32) -> f32 {
	if value.is_finite() {
		if value.abs() > 1.0 {
			(value / 100.0).clamp(-1.0, 1.0)
		} else {
			value.clamp(-1.0, 1.0)
		}
	} else {
		0.0
	}
}

fn push_decode_error(batch: &mut IfacialMocapPollBatch, message: String) {
	batch.decode_errors = batch.decode_errors.saturating_add(1);
	if batch.decode_error_examples.len() < 3 {
		batch.decode_error_examples.push(message);
	}
}

fn tracking_state(confidence: f32) -> TrackingState {
	if confidence > 0.2 {
		TrackingState::Valid
	} else {
		TrackingState::Lost
	}
}

fn transform_from_euler(euler: EulerDeg) -> TransformSample {
	TransformSample {
		translation: None,
		rotation: Some(quat_from_euler_yxz(
			euler.yaw.to_radians(),
			euler.pitch.to_radians(),
			euler.roll.to_radians(),
		)),
		scale: None,
		linear_velocity: None,
		angular_velocity: None,
	}
}

fn bone_sample(bone: HumanoidBone, euler: EulerDeg, confidence: f32, source_index: Option<u16>) -> BoneSample {
	BoneSample {
		bone,
		transform: transform_from_euler(euler),
		confidence,
		source_index,
		state: SampleState::Valid,
	}
}

fn gaze_from_euler(euler: EulerDeg, confidence: f32) -> GazeSample {
	let yaw = euler.yaw.to_radians();
	let pitch = euler.pitch.to_radians();
	GazeSample {
		origin: None,
		direction: Vec3f {
			x: yaw.sin() * pitch.cos(),
			y: pitch.sin(),
			z: -yaw.cos() * pitch.cos(),
		},
		target: None,
		confidence,
		source_index: Some(0),
	}
}

fn average_euler(left: EulerDeg, right: EulerDeg) -> EulerDeg {
	EulerDeg {
		yaw: (left.yaw + right.yaw) * 0.5,
		pitch: (left.pitch + right.pitch) * 0.5,
		roll: (left.roll + right.roll) * 0.5,
	}
}

fn expression_value(expressions: &[(String, f32)], name: &str) -> Option<f32> {
	expressions
		.iter()
		.find(|(expression_name, _)| expression_name.eq_ignore_ascii_case(name))
		.map(|(_, value)| *value)
}

fn push_scalar_signal(frame: &mut UNMotionFrame, name: impl Into<String>, value: f32, confidence: f32, source_index: Option<u16>) {
	frame.signals.push(MotionSignal {
		name: name.into(),
		value: MotionSignalValue::Scalar(value),
		confidence,
		source_index,
		state: SampleState::Valid,
	});
}

fn quat_from_euler_yxz(yaw: f32, pitch: f32, roll: f32) -> Quatf {
	let (sy, cy) = (yaw * 0.5).sin_cos();
	let (sx, cx) = (pitch * 0.5).sin_cos();
	let (sz, cz) = (roll * 0.5).sin_cos();
	let y = Quatf {
		x: 0.0,
		y: sy,
		z: 0.0,
		w: cy,
	};
	let x = Quatf {
		x: sx,
		y: 0.0,
		z: 0.0,
		w: cx,
	};
	let z = Quatf {
		x: 0.0,
		y: 0.0,
		z: sz,
		w: cz,
	};
	quat_normalize(quat_mul(quat_mul(y, x), z))
}

fn quat_mul(a: Quatf, b: Quatf) -> Quatf {
	Quatf {
		x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
		y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
		z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
		w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
	}
}

fn quat_normalize(q: Quatf) -> Quatf {
	let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
	if len > 0.0 && len.is_finite() {
		Quatf {
			x: q.x / len,
			y: q.y / len,
			z: q.z / len,
			w: q.w / len,
		}
	} else {
		Quatf {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: 1.0,
		}
	}
}

fn now_unix_ns() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_nanos() as u64)
		.unwrap_or(0)
}

pub fn resolve_socket_addr(addr: impl ToSocketAddrs) -> anyhow::Result<SocketAddr> {
	addr.to_socket_addrs()?
		.next()
		.ok_or_else(|| anyhow::anyhow!("address did not resolve"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Write;
	use std::net::TcpListener;
	use std::thread;
	use std::time::Duration;

	#[test]
	fn parses_head_eyes_confidence_and_expressions() {
		let packet = "head#2.5,-1.5,0.4|leftEye#1.2,-0.4,0.0|rightEye#1.6,-0.6,0.0|eyeBlinkLeft#0.7|mouthSmile_R-65|confidence#0.92";

		let frame = parse_ifacialmocap_frame(packet, "ifm:test", 1234).expect("parse");

		assert_eq!(frame.source_id, "ifm:test");
		assert_eq!(frame.head.yaw, 2.5);
		assert_eq!(frame.left_eye.pitch, -0.4);
		assert_eq!(frame.confidence, 0.92);
		assert_eq!(
			frame.expressions,
			vec![("eyeBlinkLeft".to_string(), 0.7), ("mouthSmile_R".to_string(), 0.65)]
		);
	}

	#[test]
	fn parses_facemotion3d_v2_ampersand_blendshapes() {
		let packet = "mouthSmile_R & -25|head#2.5,-1.5,0.4|leftEye#1.2,-0.4,0.0|rightEye#1.6,-0.6,0.0";

		let frame = parse_ifacialmocap_frame(packet, "ifm:test", 1234).expect("parse");

		assert_eq!(frame.expressions, vec![("mouthSmile_R".to_string(), -0.25)]);
	}

	#[test]
	fn accepts_prefixed_head_segment() {
		let packet = "=head#2.5,-1.5,0.4|leftEye#1.2,-0.4,0.0|rightEye#1.6,-0.6,0.0";

		let frame = parse_ifacialmocap_frame(packet, "ifm:test", 1).expect("parse");

		assert_eq!(frame.head.roll, 0.4);
	}

	#[test]
	fn requires_head_and_both_eyes() {
		let packet = "head#2.5,-1.5,0.4|leftEye#1.2,-0.4,0.0";

		let error = parse_ifacialmocap_frame(packet, "ifm:test", 1).expect_err("missing rightEye should fail");

		assert!(error.to_string().contains("rightEye field is missing"));
	}

	#[test]
	fn converts_to_unmotion_frame() {
		let packet = "head#2.5,-1.5,0.4|leftEye#1.2,-0.4,0.0|rightEye#1.6,-0.6,0.0|eyeBlinkLeft#0.7|confidence#0.92";
		let input = parse_ifacialmocap_frame(packet, "ifm:test", 1234).expect("parse");

		let frame = input.to_unmotion_frame(42);

		assert_eq!(frame.header.sequence, 42);
		assert_eq!(frame.sources[0].source_kind, MotionSourceKind::FaceId);
		assert_eq!(frame.face.as_ref().expect("face").expressions.len(), 1);
		assert_eq!(
			frame.body.as_ref().expect("body").humanoid.as_ref().expect("humanoid").bones.len(),
			3
		);
		assert!(frame.eyes.as_ref().expect("eyes").left_gaze.is_some());
		assert!(frame.signals.iter().any(|signal| signal.name == "face.eyeBlinkLeft"));
	}

	#[test]
	fn parses_real_ifacialmocap_udp_mini_fixture() {
		let fixture = include_str!("../fixtures/ifacialmocap-udp-mini.txt");
		let mut frame_count = 0;
		for (index, packet) in fixture.lines().enumerate() {
			let input = parse_ifacialmocap_frame(packet, "ifm:fixture", index as u64).expect("fixture packet");
			let frame = input.to_unmotion_frame(index as u64);

			assert_eq!(input.expressions.len(), 54);
			assert!(input.expressions.iter().any(|(name, _)| name == "jawOpen"));
			assert!(input.expressions.iter().any(|(name, _)| name == "eyeBlink_L"));
			assert!(frame.signals.iter().any(|signal| signal.name == "face.jawOpen"));
			assert_eq!(frame.face.as_ref().expect("face").expressions.len(), 54);
			assert_eq!(
				frame.body.as_ref().expect("body").humanoid.as_ref().expect("humanoid").bones.len(),
				3
			);
			frame_count += 1;
		}
		assert_eq!(frame_count, 3);
	}

	#[test]
	fn udp_receiver_continues_after_broken_frame() {
		let mut source = IfacialMocapInputSource::bind(IfacialMocapInputConfig {
			source_id: "ifm:udp".to_string(),
			bind_addr: "127.0.0.1:0".parse().expect("addr"),
			remote_addr: None,
			transport: IfacialMocapTransport::Udp,
			start_command: None,
		})
		.expect("bind udp");
		let target = source.local_addr().expect("local addr");
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender");

		sender.send_to(b"broken", target).expect("send broken");
		sender
			.send_to(
				b"head#6.0,-2.0,0.3|leftEye#3.0,-1.1,0.0|rightEye#3.4,-1.3,0.0|confidence#0.85",
				target,
			)
			.expect("send valid");
		thread::sleep(Duration::from_millis(20));

		let batch = source.poll_batch().expect("poll");

		assert_eq!(batch.frames.len(), 1);
		assert_eq!(batch.decode_errors, 1);
		assert_eq!(batch.frames[0].head.yaw, 6.0);
	}

	#[test]
	fn tcp_receiver_reassembles_split_frames() {
		let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
		let remote_addr = listener.local_addr().expect("listener addr");
		let writer = thread::spawn(move || {
			let (mut stream, _) = listener.accept().expect("accept");
			stream.write_all(b"head#3.0,-1.5,0.2|leftEye#1.0,-0.6,0.0|").expect("write 1");
			thread::sleep(Duration::from_millis(10));
			stream
				.write_all(b"rightEye#1.4,-0.8,0.0|confidence#0.9___iFacialMocap")
				.expect("write 2");
		});
		let mut source = IfacialMocapInputSource::bind(IfacialMocapInputConfig::tcp(remote_addr)).expect("connect tcp");

		let mut frames = Vec::new();
		for _ in 0..20 {
			let batch = source.poll_batch().expect("poll");
			frames.extend(batch.frames);
			if !frames.is_empty() {
				break;
			}
			thread::sleep(Duration::from_millis(10));
		}
		writer.join().expect("writer");

		assert_eq!(frames.len(), 1);
		assert_eq!(frames[0].head.yaw, 3.0);
		assert_eq!(frames[0].confidence, 0.9);
	}
}
