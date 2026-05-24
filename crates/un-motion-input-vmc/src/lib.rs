use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};

use rosc::{OscMessage, OscPacket, OscType, decoder};

const MAX_DATAGRAMS_PER_POLL: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmcInputConfig {
	pub source_id: String,
	pub listen_addr: SocketAddr,
}

pub type OscMotionInputConfig = VmcInputConfig;

impl VmcInputConfig {
	pub fn new(source_id: impl Into<String>, listen_addr: SocketAddr) -> Self {
		Self {
			source_id: source_id.into(),
			listen_addr,
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct VmcTransform {
	pub name: String,
	pub position: [f32; 3],
	pub rotation: [f32; 4],
}

pub type OscMotionTransform = VmcTransform;

#[derive(Clone, Debug, PartialEq)]
pub struct VmcBoneSample {
	pub name: String,
	pub position: [f32; 3],
	pub rotation: [f32; 4],
}

pub type OscMotionBoneSample = VmcBoneSample;

#[derive(Clone, Debug, PartialEq)]
pub struct VmcBlendshapeSample {
	pub name: String,
	pub value: f32,
}

pub type OscMotionBlendshapeSample = VmcBlendshapeSample;

#[derive(Clone, Debug, PartialEq)]
pub struct VmcInputFrame {
	pub source_id: String,
	pub received_timestamp_ns: u64,
	pub raw_datagram: Option<Vec<u8>>,
	pub ok: Option<i32>,
	pub root: Option<VmcTransform>,
	pub bones: Vec<VmcBoneSample>,
	pub blendshapes: Vec<VmcBlendshapeSample>,
	pub blend_apply: bool,
	pub message_count: usize,
}

pub type OscMotionInputFrame = VmcInputFrame;

impl VmcInputFrame {
	fn empty(source_id: impl Into<String>, received_timestamp_ns: u64) -> Self {
		Self {
			source_id: source_id.into(),
			received_timestamp_ns,
			raw_datagram: None,
			ok: None,
			root: None,
			bones: Vec::new(),
			blendshapes: Vec::new(),
			blend_apply: false,
			message_count: 0,
		}
	}

	pub fn is_empty(&self) -> bool {
		self.ok.is_none() && self.root.is_none() && self.bones.is_empty() && self.blendshapes.is_empty() && !self.blend_apply
	}

	pub fn has_vmc_payload(&self) -> bool {
		self.ok.is_some() || self.root.is_some() || !self.bones.is_empty() || !self.blendshapes.is_empty() || self.blend_apply
	}

	pub fn protocol_summary(&self) -> &'static str {
		if self.has_vmc_payload() { "vmc" } else { "-" }
	}

	pub fn summary(&self) -> String {
		format!(
			"source={} protocols={} ok={} root={} bones={} blendshapes={} apply={} messages={}",
			self.source_id,
			self.protocol_summary(),
			self.ok.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
			if self.root.is_some() { 1 } else { 0 },
			self.bones.len(),
			self.blendshapes.len(),
			self.blend_apply,
			self.message_count
		)
	}
}

#[derive(Clone, Debug)]
pub struct VmcPacketDecoder {
	source_id: String,
	pending_blendshapes: BTreeMap<String, f32>,
}

pub type OscMotionPacketDecoder = VmcPacketDecoder;

impl VmcPacketDecoder {
	pub fn new(source_id: impl Into<String>) -> Self {
		Self {
			source_id: source_id.into(),
			pending_blendshapes: BTreeMap::new(),
		}
	}

	pub fn source_id(&self) -> &str {
		&self.source_id
	}

	pub fn decode_packet(&mut self, packet: &OscPacket, received_timestamp_ns: u64) -> Option<VmcInputFrame> {
		let mut frame = VmcInputFrame::empty(self.source_id.clone(), received_timestamp_ns);
		self.visit_packet(packet, &mut frame);
		if frame.is_empty() { None } else { Some(frame) }
	}

	pub fn decode_datagram(&mut self, data: &[u8], received_timestamp_ns: u64) -> anyhow::Result<Option<VmcInputFrame>> {
		if is_unsupported_waidayo_mp_datagram(data) {
			return Ok(None);
		}
		let (_remaining, packet) = decoder::decode_udp(data)?;
		Ok(self.decode_packet(&packet, received_timestamp_ns).map(|mut frame| {
			frame.raw_datagram = Some(data.to_vec());
			frame
		}))
	}

	fn visit_packet(&mut self, packet: &OscPacket, frame: &mut VmcInputFrame) {
		match packet {
			OscPacket::Message(message) => self.visit_message(message, frame),
			OscPacket::Bundle(bundle) => {
				for packet in &bundle.content {
					self.visit_packet(packet, frame);
				}
			}
		}
	}

	fn visit_message(&mut self, message: &OscMessage, frame: &mut VmcInputFrame) {
		frame.message_count += 1;
		match message.addr.as_str() {
			"/VMC/Ext/OK" => {
				frame.ok = message.args.first().and_then(osc_int);
			}
			"/VMC/Ext/Root/Pos" => {
				frame.root = parse_transform(&message.args);
			}
			"/VMC/Ext/Bone/Pos" => {
				if let Some(transform) = parse_transform(&message.args) {
					frame.bones.push(VmcBoneSample {
						name: transform.name,
						position: transform.position,
						rotation: transform.rotation,
					});
				}
			}
			"/VMC/Ext/Blend/Val" => {
				if let (Some(name), Some(value)) = (message.args.first().and_then(osc_string), message.args.get(1).and_then(osc_float)) {
					self.pending_blendshapes.insert(name.to_string(), value);
					frame.blendshapes.push(VmcBlendshapeSample {
						name: name.to_string(),
						value,
					});
				}
			}
			"/VMC/Ext/Blend/Apply" => {
				frame.blend_apply = true;
				frame.blendshapes = self
					.pending_blendshapes
					.iter()
					.map(|(name, value)| VmcBlendshapeSample {
						name: name.clone(),
						value: *value,
					})
					.collect();
				self.pending_blendshapes.clear();
			}
			_ => {}
		}
	}
}

pub struct VmcInputSource {
	socket: UdpSocket,
	decoder: VmcPacketDecoder,
}

pub type OscMotionInputSource = VmcInputSource;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VmcPollBatch {
	pub frames: Vec<VmcInputFrame>,
	pub decode_errors: u64,
	pub decode_error_examples: Vec<String>,
	/// この poll サイクルで socket から `recv_from` が成功した raw UDP datagram の総数。
	/// `frames.len()` (decode 成功) や `decode_errors` (parse 失敗) と合わせて、
	/// 「受信したが silent drop された (例: Waidayo の `/MP/` 拡張) 数」が
	/// `received_datagrams - frames.len() - decode_errors - non_vmc_dropped` で
	/// 算出できる。bind は成功しているのに「何も受けていない」のか「受けているが
	/// 全部 drop されている」のかを観測する用途で使う。
	pub received_datagrams: u64,
	/// `/MP/` で始まる Waidayo "MotionPath" 拡張など、VMC 仕様外で明示的に捨てた
	/// datagram 数 (`is_unsupported_waidayo_mp_datagram` で検出されたもの)。
	pub non_vmc_dropped: u64,
}

pub type OscMotionPollBatch = VmcPollBatch;

impl VmcInputSource {
	pub fn bind(config: VmcInputConfig) -> anyhow::Result<Self> {
		let socket = UdpSocket::bind(config.listen_addr)?;
		socket.set_nonblocking(true)?;
		Ok(Self {
			socket,
			decoder: VmcPacketDecoder::new(config.source_id),
		})
	}

	pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
		Ok(self.socket.local_addr()?)
	}

	pub fn source_id(&self) -> &str {
		self.decoder.source_id()
	}

	pub fn poll_frames(&mut self) -> anyhow::Result<Vec<VmcInputFrame>> {
		Ok(self.poll_batch()?.frames)
	}

	pub fn poll_batch(&mut self) -> anyhow::Result<VmcPollBatch> {
		let mut frames = Vec::new();
		let mut decode_errors = 0;
		let mut decode_error_examples = Vec::new();
		let mut non_vmc_dropped = 0_u64;
		let mut buf = [0_u8; 65535];
		let mut received = 0_usize;
		loop {
			match self.socket.recv_from(&mut buf) {
				Ok((len, _addr)) => {
					received += 1;
					let data = &buf[..len];
					let was_dropped_as_non_vmc = is_unsupported_waidayo_mp_datagram(data);
					match self.decoder.decode_datagram(data, now_unix_ns()) {
						Ok(Some(frame)) => frames.push(frame),
						Ok(None) => {
							if was_dropped_as_non_vmc {
								non_vmc_dropped = non_vmc_dropped.saturating_add(1);
							}
						}
						Err(error) => {
							decode_errors += 1;
							if decode_error_examples.len() < 3 {
								decode_error_examples.push(format_decode_error(error, data));
							}
						}
					}
					if received >= MAX_DATAGRAMS_PER_POLL {
						return Ok(VmcPollBatch {
							frames,
							decode_errors,
							decode_error_examples,
							received_datagrams: received as u64,
							non_vmc_dropped,
						});
					}
				}
				Err(error) if error.kind() == ErrorKind::WouldBlock => {
					return Ok(VmcPollBatch {
						frames,
						decode_errors,
						decode_error_examples,
						received_datagrams: received as u64,
						non_vmc_dropped,
					});
				}
				Err(error) => return Err(error.into()),
			}
		}
	}
}

fn format_decode_error(error: impl std::fmt::Display, bytes: &[u8]) -> String {
	let hex = bytes
		.iter()
		.take(24)
		.map(|byte| format!("{byte:02X}"))
		.collect::<Vec<_>>()
		.join(" ");
	let ascii = bytes
		.iter()
		.take(24)
		.map(|byte| {
			if byte.is_ascii_graphic() || *byte == b' ' {
				char::from(*byte)
			} else {
				'.'
			}
		})
		.collect::<String>();
	format!("len={} error={} hex={} ascii={}", bytes.len(), error, hex, ascii)
}

fn parse_transform(args: &[OscType]) -> Option<VmcTransform> {
	let name = args.first().and_then(osc_string)?;
	let position = [
		args.get(1).and_then(osc_float)?,
		args.get(2).and_then(osc_float)?,
		args.get(3).and_then(osc_float)?,
	];
	let rotation = [
		args.get(4).and_then(osc_float)?,
		args.get(5).and_then(osc_float)?,
		args.get(6).and_then(osc_float)?,
		args.get(7).and_then(osc_float)?,
	];
	Some(VmcTransform {
		name: name.to_string(),
		position,
		rotation,
	})
}

fn osc_string(value: &OscType) -> Option<&str> {
	match value {
		OscType::String(value) => Some(value),
		_ => None,
	}
}

fn osc_float(value: &OscType) -> Option<f32> {
	match value {
		OscType::Float(value) => Some(*value),
		OscType::Double(value) => Some(*value as f32),
		OscType::Int(value) => Some(*value as f32),
		OscType::Long(value) => Some(*value as f32),
		_ => None,
	}
}

fn osc_int(value: &OscType) -> Option<i32> {
	match value {
		OscType::Int(value) => Some(*value),
		OscType::Long(value) => i32::try_from(*value).ok(),
		_ => None,
	}
}

fn now_unix_ns() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_nanos() as u64)
		.unwrap_or(0)
}

fn is_unsupported_waidayo_mp_datagram(data: &[u8]) -> bool {
	data.starts_with(b"/MP/")
}

#[cfg(test)]
mod tests {
	use super::*;
	use rosc::{OscBundle, OscMessage, OscTime, encoder};
	use serde_json::Value;
	use std::net::UdpSocket;
	use std::time::Duration;

	fn packet_message(addr: &str, args: Vec<OscType>) -> OscPacket {
		OscPacket::Message(OscMessage {
			addr: addr.to_string(),
			args,
		})
	}

	fn bundle(content: Vec<OscPacket>) -> OscPacket {
		OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content,
		})
	}

	fn encoded_bone_packet(name: &str, x: f32) -> Vec<u8> {
		encoder::encode(&packet_message(
			"/VMC/Ext/Bone/Pos",
			vec![
				OscType::String(name.to_string()),
				OscType::Float(x),
				OscType::Float(0.0),
				OscType::Float(0.0),
				OscType::Float(0.0),
				OscType::Float(0.0),
				OscType::Float(0.0),
				OscType::Float(1.0),
			],
		))
		.expect("encode")
	}

	#[test]
	fn decodes_vmc_bone_root_and_ok_messages() {
		let packet = bundle(vec![
			packet_message("/VMC/Ext/OK", vec![OscType::Int(1)]),
			packet_message(
				"/VMC/Ext/Root/Pos",
				vec![
					OscType::String("root".to_string()),
					OscType::Float(1.0),
					OscType::Float(2.0),
					OscType::Float(3.0),
					OscType::Float(0.0),
					OscType::Float(0.1),
					OscType::Float(0.2),
					OscType::Float(0.9),
				],
			),
			packet_message(
				"/VMC/Ext/Bone/Pos",
				vec![
					OscType::String("Head".to_string()),
					OscType::Float(0.0),
					OscType::Float(0.5),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(0.2),
					OscType::Float(0.0),
					OscType::Float(0.98),
				],
			),
		]);
		let mut decoder = VmcPacketDecoder::new("vmc:test");

		let frame = decoder.decode_packet(&packet, 123).expect("frame");

		assert_eq!(frame.source_id, "vmc:test");
		assert_eq!(frame.received_timestamp_ns, 123);
		assert_eq!(frame.ok, Some(1));
		assert_eq!(frame.root.expect("root").position, [1.0, 2.0, 3.0]);
		assert_eq!(frame.bones.len(), 1);
		assert_eq!(frame.bones[0].name, "Head");
		assert_eq!(frame.bones[0].rotation, [0.0, 0.2, 0.0, 0.98]);
		assert_eq!(frame.message_count, 3);
	}

	#[test]
	fn coalesces_blendshape_values_until_apply() {
		let mut decoder = VmcPacketDecoder::new("vmc:face");
		let val = packet_message(
			"/VMC/Ext/Blend/Val",
			vec![OscType::String("jawOpen".to_string()), OscType::Float(0.4)],
		);
		let apply = bundle(vec![
			packet_message(
				"/VMC/Ext/Blend/Val",
				vec![OscType::String("eyeBlinkLeft".to_string()), OscType::Float(0.8)],
			),
			packet_message("/VMC/Ext/Blend/Apply", Vec::new()),
		]);

		let val_frame = decoder.decode_packet(&val, 1).expect("val frame");
		assert!(!val_frame.blend_apply);
		assert_eq!(
			val_frame.blendshapes,
			vec![VmcBlendshapeSample {
				name: "jawOpen".to_string(),
				value: 0.4,
			}]
		);
		let frame = decoder.decode_packet(&apply, 2).expect("apply frame");

		assert!(frame.blend_apply);
		assert_eq!(
			frame.blendshapes,
			vec![
				VmcBlendshapeSample {
					name: "eyeBlinkLeft".to_string(),
					value: 0.8,
				},
				VmcBlendshapeSample {
					name: "jawOpen".to_string(),
					value: 0.4,
				},
			]
		);
	}

	#[test]
	fn preserves_raw_blendshape_value_datagrams_for_passthrough() {
		let packet = packet_message(
			"/VMC/Ext/Blend/Val",
			vec![OscType::String("eyeBlinkLeft".to_string()), OscType::Float(0.7)],
		);
		let encoded = encoder::encode(&packet).expect("encode");
		let mut decoder = VmcPacketDecoder::new("vmc:face");

		let frame = decoder.decode_datagram(&encoded, 10).expect("decode").expect("frame");

		assert_eq!(frame.raw_datagram, Some(encoded));
		assert_eq!(frame.blendshapes.len(), 1);
		assert_eq!(frame.blendshapes[0].name, "eyeBlinkLeft");
		assert!(!frame.blend_apply);
	}

	#[test]
	fn decodes_udp_datagram() {
		let packet = packet_message(
			"/VMC/Ext/Bone/Pos",
			vec![
				OscType::String("LeftHand".to_string()),
				OscType::Float(0.1),
				OscType::Float(0.2),
				OscType::Float(0.3),
				OscType::Float(0.4),
				OscType::Float(0.5),
				OscType::Float(0.6),
				OscType::Float(0.7),
			],
		);
		let encoded = encoder::encode(&packet).expect("encode");
		let mut decoder = VmcPacketDecoder::new("vmc:udp");

		let frame = decoder.decode_datagram(&encoded, 777).expect("decode").expect("frame");

		assert_eq!(frame.bones[0].name, "LeftHand");
		assert_eq!(frame.bones[0].position, [0.1, 0.2, 0.3]);
	}

	#[test]
	fn poll_batch_limits_datagrams_to_keep_worker_responsive() {
		let mut source = VmcInputSource::bind(VmcInputConfig::new("vmc:burst", "127.0.0.1:0".parse().expect("listen addr"))).expect("bind");
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender bind");
		let target = source.local_addr().expect("source addr");
		for index in 0..(MAX_DATAGRAMS_PER_POLL + 3) {
			let packet = encoded_bone_packet("Head", index as f32);
			sender.send_to(&packet, target).expect("send datagram");
		}
		std::thread::sleep(Duration::from_millis(20));

		let first = source.poll_batch().expect("first poll");
		let second = source.poll_batch().expect("second poll");

		assert_eq!(first.frames.len(), MAX_DATAGRAMS_PER_POLL);
		assert_eq!(second.frames.len(), 3);
	}

	#[test]
	fn ignores_valid_unknown_osc_messages() {
		let packet = packet_message("/Ignored/Address", vec![OscType::Float(1.0)]);
		let encoded = encoder::encode(&packet).expect("encode");
		let mut decoder = VmcPacketDecoder::new("osc:unknown");

		let frame = decoder.decode_datagram(&encoded, 1).expect("decode");

		assert!(frame.is_none());
	}

	#[test]
	fn ignores_waidayo_send_motion_mp_osc_messages() {
		let mut blob = Vec::new();
		for value in [0.1_f32, 0.2, 0.3, 0.4, 0.5, 0.6] {
			blob.extend(value.to_le_bytes());
		}
		let packet = bundle(vec![
			packet_message("/MP/AUX", vec![OscType::Float(0.5)]),
			packet_message("/MP/BS", vec![OscType::Blob(blob)]),
		]);
		let mut decoder = VmcPacketDecoder::new("waidayo:send-motion");

		let frame = decoder.decode_packet(&packet, 1);

		assert!(frame.is_none());
	}

	#[test]
	fn ignores_waidayo_send_motion_raw_mp_datagrams_without_decode_error() {
		let bytes = [
			0x2F, 0x4D, 0x50, 0x2F, 0x41, 0x55, 0x58, 0x00, 0x2C, 0x66, 0x00, 0x00, 0x3F, 0x10, 0x00, 0x00,
		];
		let mut decoder = VmcPacketDecoder::new("waidayo:send-motion");

		let frame = decoder.decode_datagram(&bytes, 1).expect("decode");

		assert!(frame.is_none());
	}

	#[test]
	fn decodes_real_vmc_mini_fixtures() {
		for fixture in [
			(
				"waidayo",
				include_str!("../fixtures/waidayo-vmc-mini.jsonl"),
				true,
				"EyeBlinkLeft",
				"JawOpen",
			),
			(
				"warudo",
				include_str!("../fixtures/warudo-vmc-mini.jsonl"),
				true,
				"eyeBlinkLeft",
				"jawOpen",
			),
			(
				"wmc",
				include_str!("../fixtures/wmc-vmc-mini.jsonl"),
				false,
				"EyeBlinkLeft",
				"JawOpen",
			),
			(
				"vseeface",
				include_str!("../fixtures/vseeface-vmc-mini.jsonl"),
				true,
				"EyeBlinkLeft",
				"JawOpen",
			),
		] {
			let (name, jsonl, expects_root, blink_name, jaw_name) = fixture;
			let packet = capture_fixture_packet(jsonl);
			let mut decoder = VmcPacketDecoder::new(format!("fixture:{name}"));

			let frame = decoder.decode_packet(&packet, 123).expect("fixture frame");

			assert_eq!(frame.root.is_some(), expects_root, "{name} root presence");
			assert!(frame.bones.iter().any(|bone| bone.name == "Head"), "{name} Head bone");
			assert!(frame.bones.iter().any(|bone| bone.name == "LeftHand"), "{name} LeftHand bone");
			assert!(frame.blend_apply, "{name} blend apply");
			assert!(
				frame.blendshapes.iter().any(|blendshape| blendshape.name == blink_name),
				"{name} blink blendshape"
			);
			assert!(
				frame.blendshapes.iter().any(|blendshape| blendshape.name == jaw_name),
				"{name} jaw blendshape"
			);
		}
	}

	fn capture_fixture_packet(jsonl: &str) -> OscPacket {
		bundle(
			jsonl
				.lines()
				.filter(|line| !line.trim().is_empty())
				.map(capture_line_packet)
				.collect(),
		)
	}

	fn capture_line_packet(line: &str) -> OscPacket {
		let value: Value = serde_json::from_str(line).expect("fixture json line");
		let addr = value["addr"].as_str().expect("fixture addr").to_string();
		let args = value["args"].as_array().expect("fixture args").iter().map(capture_arg).collect();
		packet_message(&addr, args)
	}

	fn capture_arg(value: &Value) -> OscType {
		match value["type"].as_str().expect("fixture arg type") {
			"string" => OscType::String(value["value"].as_str().expect("fixture string").to_string()),
			"float" => OscType::Float(value["value"].as_f64().expect("fixture float") as f32),
			"int" => OscType::Int(value["value"].as_i64().expect("fixture int") as i32),
			other => panic!("unsupported fixture arg type: {other}"),
		}
	}
}
