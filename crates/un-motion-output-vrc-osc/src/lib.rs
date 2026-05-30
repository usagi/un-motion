use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};

use anyhow::Context;
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use serde::{Deserialize, Serialize};
use un_motion_frame::{MotionSignalValue, SampleState, UNMotionFrame};

const AVATAR_PARAMETER_PREFIX: &str = "/avatar/parameters/";
const MAX_OSC_DATAGRAM_BYTES: usize = 4096;
const BINARY_PARAMETER_DEADZONE: f32 = 0.15;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VrcOscOutputOptions {
	pub parameter_prefix: String,
	pub emit_binary_parameters: bool,
}

impl Default for VrcOscOutputOptions {
	fn default() -> Self {
		Self {
			parameter_prefix: String::new(),
			emit_binary_parameters: false,
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct VrcOscParameter {
	pub name: String,
	pub value: VrcOscParameterValue,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VrcOscParameterValue {
	Float(f32),
	Bool(bool),
}

#[derive(Debug)]
pub struct VrcOscOutputSink {
	socket: UdpSocket,
	target: SocketAddr,
	options: VrcOscOutputOptions,
	last_values: BTreeMap<String, VrcOscParameterValue>,
}

impl VrcOscOutputSink {
	pub fn new(target: SocketAddr) -> anyhow::Result<Self> {
		let socket = UdpSocket::bind("0.0.0.0:0").context("VRC OSC UDP socket bind failed")?;
		Ok(Self {
			socket,
			target,
			options: VrcOscOutputOptions::default(),
			last_values: BTreeMap::new(),
		})
	}

	pub fn with_options(mut self, options: VrcOscOutputOptions) -> Self {
		self.options = options;
		self
	}

	pub fn send(&mut self, frame: &UNMotionFrame) -> anyhow::Result<(u64, u64)> {
		let packets = self.changed_packets_for_frame(frame);
		if packets.is_empty() {
			return Ok((0, 0));
		}
		let packet_count = packets.len() as u64;
		let datagrams = encode_packet_chunks(packets)?;
		let datagram_count = datagrams.len() as u64;
		for encoded in datagrams {
			self.socket
				.send_to(&encoded, self.target)
				.with_context(|| format!("VRC OSC UDP send failed: {}", self.target))?;
		}
		Ok((datagram_count, packet_count))
	}

	fn changed_packets_for_frame(&mut self, frame: &UNMotionFrame) -> Vec<OscPacket> {
		let parameters = vrc_osc_parameters_for_frame(frame, &self.options);
		let mut packets = Vec::new();
		for parameter in parameters {
			let old = self.last_values.get(&parameter.name).copied();
			if old.is_some_and(|old| parameter_value_unchanged(old, parameter.value)) {
				continue;
			}
			self.last_values.insert(parameter.name.clone(), parameter.value);
			packets.push(vrc_osc_packet_for_parameter(parameter));
		}
		packets
	}
}

fn parameter_value_unchanged(old: VrcOscParameterValue, new: VrcOscParameterValue) -> bool {
	match (old, new) {
		(VrcOscParameterValue::Bool(old), VrcOscParameterValue::Bool(new)) => old == new,
		(VrcOscParameterValue::Float(old), VrcOscParameterValue::Float(new)) => (old - new).abs() < 0.0005,
		_ => false,
	}
}

fn encode_packet_chunks(packets: Vec<OscPacket>) -> anyhow::Result<Vec<Vec<u8>>> {
	let mut datagrams = Vec::new();
	let mut chunk = Vec::<OscPacket>::new();
	for packet in packets {
		let mut candidate = chunk.clone();
		candidate.push(packet.clone());
		let encoded = encode_bundle(candidate).context("VRC OSC encode failed")?;
		if encoded.len() <= MAX_OSC_DATAGRAM_BYTES || chunk.is_empty() {
			chunk.push(packet);
			continue;
		}
		datagrams.push(encode_bundle(std::mem::take(&mut chunk)).context("VRC OSC encode failed")?);
		chunk.push(packet);
	}
	if !chunk.is_empty() {
		datagrams.push(encode_bundle(chunk).context("VRC OSC encode failed")?);
	}
	Ok(datagrams)
}

fn encode_bundle(content: Vec<OscPacket>) -> Result<Vec<u8>, rosc::OscError> {
	encoder::encode(&OscPacket::Bundle(OscBundle {
		timetag: OscTime { seconds: 0, fractional: 1 },
		content,
	}))
}

pub fn vrc_osc_packets_for_frame(frame: &UNMotionFrame, options: &VrcOscOutputOptions) -> Vec<OscPacket> {
	vrc_osc_parameters_for_frame(frame, options)
		.into_iter()
		.map(vrc_osc_packet_for_parameter)
		.collect()
}

fn vrc_osc_packet_for_parameter(parameter: VrcOscParameter) -> OscPacket {
	let arg = match parameter.value {
		VrcOscParameterValue::Float(value) => OscType::Float(value),
		VrcOscParameterValue::Bool(value) => OscType::Bool(value),
	};
	OscPacket::Message(OscMessage {
		addr: format!("{AVATAR_PARAMETER_PREFIX}{}", parameter.name),
		args: vec![arg],
	})
}

pub fn vrc_osc_parameters_for_frame(frame: &UNMotionFrame, options: &VrcOscOutputOptions) -> Vec<VrcOscParameter> {
	let mut parameters = BTreeMap::<String, f32>::new();
	let mut face_is_live = false;
	if let Some(face) = &frame.face {
		if face.tracking_state != un_motion_frame::TrackingState::Lost {
			face_is_live = true;
			for expression in &face.expressions {
				if sample_state_is_usable(expression.state)
					&& let Some((name, value)) = map_expression_name(&expression.name, expression.value, options)
				{
					parameters.insert(name, value);
				}
			}
		}
	}
	for signal in &frame.signals {
		if !sample_state_is_usable(signal.state) {
			continue;
		}
		match signal.value {
			MotionSignalValue::Scalar(value) => {
				if let Some((name, value)) = map_expression_name(&signal.name, value, options) {
					parameters.entry(name).or_insert(value);
				}
			}
			MotionSignalValue::Bool(value) => {
				if let Some(name) = normalize_vrcft_parameter_name(&signal.name, options) {
					parameters.entry(name).or_insert(if value { 1.0 } else { 0.0 });
				}
			}
			_ => {}
		}
	}
	let mut output = Vec::new();
	for (name, value) in parameters {
		output.push(VrcOscParameter {
			name: name.clone(),
			value: VrcOscParameterValue::Float(value),
		});
		if options.emit_binary_parameters {
			output.extend(binary_parameters_for(&name, value));
		}
	}
	if face_is_live {
		output.push(VrcOscParameter {
			name: "EyeTrackingActive".to_string(),
			value: VrcOscParameterValue::Bool(true),
		});
		output.push(VrcOscParameter {
			name: "ExpressionTrackingActive".to_string(),
			value: VrcOscParameterValue::Bool(true),
		});
		output.push(VrcOscParameter {
			name: "LipTrackingActive".to_string(),
			value: VrcOscParameterValue::Bool(true),
		});
		output.push(VrcOscParameter {
			name: "FacialExpressionsDisabled".to_string(),
			value: VrcOscParameterValue::Bool(false),
		});
	}
	output
}

fn sample_state_is_usable(state: SampleState) -> bool {
	matches!(state, SampleState::Valid | SampleState::Held | SampleState::Decayed)
}

fn map_expression_name(name: &str, value: f32, options: &VrcOscOutputOptions) -> Option<(String, f32)> {
	if let Some(parameter) = normalize_vrcft_parameter_name(name, options) {
		return Some((parameter, clamp_for_parameter(name, value)));
	}
	let normalized = normalize_input_name(name).to_ascii_lowercase();
	let route = arkit_to_vrcft_route(&normalized)?;
	let mut value = value * route.value_scale;
	if route.invert_openness {
		value = 0.75 * (1.0 - value.clamp(0.0, 1.0));
	}
	Some((
		apply_parameter_prefix(route.parameter, &options.parameter_prefix),
		clamp_for_parameter(route.parameter, value),
	))
}

fn normalize_vrcft_parameter_name(name: &str, options: &VrcOscOutputOptions) -> Option<String> {
	let name = name.trim().trim_start_matches(AVATAR_PARAMETER_PREFIX).trim_start_matches('/');
	if name == "EyeTrackingActive" || name == "ExpressionTrackingActive" || name == "LipTrackingActive" {
		return Some(name.to_string());
	}
	if name.starts_with("v2/") {
		return Some(apply_parameter_prefix(name, &options.parameter_prefix));
	}
	if name.contains("/v2/") {
		return Some(name.to_string());
	}
	None
}

fn normalize_input_name(name: &str) -> &str {
	name.trim()
		.trim_start_matches("face.")
		.trim_start_matches("expression.")
		.trim_start_matches("face.expression.")
}

fn apply_parameter_prefix(parameter: &str, prefix: &str) -> String {
	let parameter = parameter.trim().trim_matches('/');
	let prefix = prefix.trim().trim_matches('/');
	if prefix.is_empty() || parameter.contains("/v2/") {
		parameter.to_string()
	} else {
		format!("{prefix}/{parameter}")
	}
}

fn binary_parameters_for(parameter: &str, value: f32) -> Vec<VrcOscParameter> {
	if !parameter.contains("/v2/") && !parameter.starts_with("v2/") {
		return Vec::new();
	}
	let magnitude = value.abs().clamp(0.0, 1.0);
	let quantized = if magnitude < BINARY_PARAMETER_DEADZONE {
		0
	} else if magnitude >= 0.99999 {
		15
	} else {
		(magnitude * 15.0).floor() as u8
	};
	let mut parameters = Vec::with_capacity(5);
	parameters.push(VrcOscParameter {
		name: format!("{parameter}Negative"),
		value: VrcOscParameterValue::Bool(value < 0.0 && quantized > 0),
	});
	for (bit, suffix) in [(0, "1"), (1, "2"), (2, "4"), (3, "8")] {
		parameters.push(VrcOscParameter {
			name: format!("{parameter}{suffix}"),
			value: VrcOscParameterValue::Bool(((quantized >> bit) & 1) == 1),
		});
	}
	parameters
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VrcftRoute {
	parameter: &'static str,
	invert_openness: bool,
	value_scale: f32,
}

impl VrcftRoute {
	const fn new(parameter: &'static str) -> Self {
		Self {
			parameter,
			invert_openness: false,
			value_scale: 1.0,
		}
	}

	const fn signed(parameter: &'static str, value_scale: f32) -> Self {
		Self {
			parameter,
			invert_openness: false,
			value_scale,
		}
	}

	const fn inverted_openness(parameter: &'static str) -> Self {
		Self {
			parameter,
			invert_openness: true,
			value_scale: 1.0,
		}
	}
}

fn arkit_to_vrcft_route(name: &str) -> Option<VrcftRoute> {
	let route = match name {
		"eyeblinkleft" | "blink_l" => VrcftRoute::inverted_openness("v2/EyeLidLeft"),
		"eyeblinkright" | "blink_r" => VrcftRoute::inverted_openness("v2/EyeLidRight"),
		"eyelookoutleft" | "eyelookinright" | "eye.left.yaw" => VrcftRoute::new("v2/EyeLeftX"),
		"eyelookinleft" | "eyelookoutright" | "eye.right.yaw" => VrcftRoute::new("v2/EyeRightX"),
		"eyelookupleft" | "eye.left.pitch" => VrcftRoute::new("v2/EyeLeftY"),
		"eyelookupright" | "eye.right.pitch" => VrcftRoute::new("v2/EyeRightY"),
		"eyesquintleft" => VrcftRoute::new("v2/EyeSquintLeft"),
		"eyesquintright" => VrcftRoute::new("v2/EyeSquintRight"),
		"eyewideleft" => VrcftRoute::new("v2/EyeWideLeft"),
		"eyewideright" => VrcftRoute::new("v2/EyeWideRight"),
		"browdownleft" => VrcftRoute::new("v2/BrowLowererLeft"),
		"browdownright" => VrcftRoute::new("v2/BrowLowererRight"),
		"browinnerup" => VrcftRoute::new("v2/BrowInnerUp"),
		"browouterupleft" => VrcftRoute::new("v2/BrowOuterUpLeft"),
		"browouterupright" => VrcftRoute::new("v2/BrowOuterUpRight"),
		"cheekpuff" => VrcftRoute::new("v2/CheekPuffSuck"),
		"cheeksquintleft" => VrcftRoute::new("v2/CheekSquintLeft"),
		"cheeksquintright" => VrcftRoute::new("v2/CheekSquintRight"),
		"jawopen" => VrcftRoute::new("v2/JawOpen"),
		"jawforward" => VrcftRoute::new("v2/JawForward"),
		"jawleft" => VrcftRoute::signed("v2/JawX", -1.0),
		"jawright" => VrcftRoute::new("v2/JawX"),
		"mouthclose" => VrcftRoute::new("v2/MouthClosed"),
		"mouthfunnel" => VrcftRoute::new("v2/LipFunnel"),
		"mouthpucker" => VrcftRoute::new("v2/LipPucker"),
		"mouthleft" => VrcftRoute::signed("v2/MouthX", -1.0),
		"mouthright" => VrcftRoute::new("v2/MouthX"),
		"mouthsmileleft" => VrcftRoute::new("v2/SmileFrownLeft"),
		"mouthsmileright" => VrcftRoute::new("v2/SmileFrownRight"),
		"mouthfrownleft" => VrcftRoute::signed("v2/SmileFrownLeft", -1.0),
		"mouthfrownright" => VrcftRoute::signed("v2/SmileFrownRight", -1.0),
		"mouthdimpleleft" => VrcftRoute::new("v2/MouthDimpleLeft"),
		"mouthdimpleright" => VrcftRoute::new("v2/MouthDimpleRight"),
		"mouthstretchleft" => VrcftRoute::new("v2/MouthStretchLeft"),
		"mouthstretchright" => VrcftRoute::new("v2/MouthStretchRight"),
		"mouthrolllower" => VrcftRoute::new("v2/LipSuckLower"),
		"mouthrollupper" => VrcftRoute::new("v2/LipSuckUpper"),
		"mouthshruglower" => VrcftRoute::new("v2/MouthRaiserLower"),
		"mouthshrugupper" => VrcftRoute::new("v2/MouthRaiserUpper"),
		"mouthpressleft" => VrcftRoute::new("v2/MouthPressLeft"),
		"mouthpressright" => VrcftRoute::new("v2/MouthPressRight"),
		"mouthlowerdownleft" => VrcftRoute::new("v2/MouthLowerDownLeft"),
		"mouthlowerdownright" => VrcftRoute::new("v2/MouthLowerDownRight"),
		"mouthupperupleft" => VrcftRoute::new("v2/MouthUpperUpLeft"),
		"mouthupperupright" => VrcftRoute::new("v2/MouthUpperUpRight"),
		"nosesneerleft" => VrcftRoute::new("v2/NoseSneerLeft"),
		"nosesneerright" => VrcftRoute::new("v2/NoseSneerRight"),
		"tongueout" => VrcftRoute::new("v2/TongueOut"),
		_ => return None,
	};
	Some(route)
}

fn clamp_for_parameter(parameter: &str, value: f32) -> f32 {
	let parameter = parameter.trim_start_matches(AVATAR_PARAMETER_PREFIX);
	if parameter_supports_negative(parameter) {
		value.clamp(-1.0, 1.0)
	} else {
		value.clamp(0.0, 1.0)
	}
}

fn parameter_supports_negative(parameter: &str) -> bool {
	let parameter = parameter.rsplit('/').next().unwrap_or(parameter);
	matches!(
		parameter,
		"EyeLeftX"
			| "EyeLeftY"
			| "EyeRightX"
			| "EyeRightY"
			| "EyeX" | "EyeY"
			| "JawX" | "JawZ"
			| "JawForward"
			| "CheekPuffSuck"
			| "CheekPuffSuckLeft"
			| "CheekPuffSuckRight"
			| "MouthX"
			| "MouthUpperX"
			| "MouthLowerX"
			| "SmileFrown"
			| "SmileFrownLeft"
			| "SmileFrownRight"
			| "SmileSad"
			| "SmileSadLeft"
			| "SmileSadRight"
			| "TongueX"
			| "TongueY"
			| "TongueArchY"
			| "TongueShape"
	)
}

#[cfg(test)]
mod tests {
	use rosc::OscPacket;
	use un_motion_frame::{ExpressionSample, FaceMotion, MotionSignal, SampleState, TrackingState, UNMotionFrame};

	use super::*;

	fn expression(name: &str, value: f32) -> ExpressionSample {
		ExpressionSample {
			name: name.to_string(),
			value,
			confidence: 1.0,
			source_index: None,
			state: SampleState::Valid,
		}
	}

	fn scalar(name: &str, value: f32) -> MotionSignal {
		MotionSignal {
			name: name.to_string(),
			value: MotionSignalValue::Scalar(value),
			confidence: 1.0,
			source_index: None,
			state: SampleState::Valid,
		}
	}

	fn packet_value(packet: &OscPacket) -> (&str, f32) {
		let OscPacket::Message(message) = packet else {
			panic!("packet should be message");
		};
		let OscType::Float(value) = message.args[0] else {
			panic!("arg should be float");
		};
		(&message.addr, value)
	}

	fn bool_packet_value(packet: &OscPacket) -> (&str, bool) {
		let OscPacket::Message(message) = packet else {
			panic!("packet should be message");
		};
		let OscType::Bool(value) = message.args[0] else {
			panic!("arg should be bool");
		};
		(&message.addr, value)
	}

	fn try_bool_packet_value(packet: &OscPacket) -> Option<(&str, bool)> {
		let OscPacket::Message(message) = packet else {
			return None;
		};
		let OscType::Bool(value) = message.args[0] else {
			return None;
		};
		Some((&message.addr, value))
	}

	#[test]
	fn maps_arkit_expression_to_vrcft_avatar_parameter() {
		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![expression("JawOpen", 0.42)],
		});

		let packets = vrc_osc_packets_for_frame(&frame, &VrcOscOutputOptions::default());

		assert_eq!(packets.len(), 5);
		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/v2/JawOpen", 0.42));
	}

	#[test]
	fn applies_parameter_prefix() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.mouthSmileLeft", 0.6));

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "/ExamplePrefix/".to_string(),
				..VrcOscOutputOptions::default()
			},
		);

		assert_eq!(
			packet_value(&packets[0]),
			("/avatar/parameters/ExamplePrefix/v2/SmileFrownLeft", 0.6)
		);
	}

	#[test]
	fn emits_vrcft_binary_parameters_for_avatar_bool_setups() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.mouthSmileRight", 0.6));

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				emit_binary_parameters: true,
			},
		);

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/FT/v2/SmileFrownRight", 0.6));
		assert_eq!(
			bool_packet_value(&packets[1]),
			("/avatar/parameters/FT/v2/SmileFrownRightNegative", false)
		);
		assert_eq!(bool_packet_value(&packets[2]), ("/avatar/parameters/FT/v2/SmileFrownRight1", true));
		assert_eq!(bool_packet_value(&packets[3]), ("/avatar/parameters/FT/v2/SmileFrownRight2", false));
		assert_eq!(bool_packet_value(&packets[4]), ("/avatar/parameters/FT/v2/SmileFrownRight4", false));
		assert_eq!(bool_packet_value(&packets[5]), ("/avatar/parameters/FT/v2/SmileFrownRight8", true));
	}

	#[test]
	fn emits_tracking_active_and_expression_enable_flags() {
		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![expression("JawOpen", 0.42)],
		});

		let packets = vrc_osc_packets_for_frame(&frame, &VrcOscOutputOptions::default());

		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/EyeTrackingActive", true)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/ExpressionTrackingActive", true)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/LipTrackingActive", true)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FacialExpressionsDisabled", false)))
		);
	}

	#[test]
	fn emits_negative_binary_parameters_for_signed_shapes() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.mouthFrownLeft", 0.5));

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				emit_binary_parameters: true,
			},
		);

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/FT/v2/SmileFrownLeft", -0.5));
		assert_eq!(
			bool_packet_value(&packets[1]),
			("/avatar/parameters/FT/v2/SmileFrownLeftNegative", true)
		);
	}

	#[test]
	fn suppresses_low_level_binary_parameter_jitter() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.noseSneerLeft", 0.08));

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				emit_binary_parameters: true,
			},
		);

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/FT/v2/NoseSneerLeft", 0.08));
		assert_eq!(
			bool_packet_value(&packets[1]),
			("/avatar/parameters/FT/v2/NoseSneerLeftNegative", false)
		);
		assert_eq!(bool_packet_value(&packets[2]), ("/avatar/parameters/FT/v2/NoseSneerLeft1", false));
		assert_eq!(bool_packet_value(&packets[3]), ("/avatar/parameters/FT/v2/NoseSneerLeft2", false));
		assert_eq!(bool_packet_value(&packets[4]), ("/avatar/parameters/FT/v2/NoseSneerLeft4", false));
		assert_eq!(bool_packet_value(&packets[5]), ("/avatar/parameters/FT/v2/NoseSneerLeft8", false));
	}

	#[test]
	fn chunks_large_vrcft_frames_under_datagram_limit() {
		let mut packets = Vec::new();
		for index in 0..300 {
			packets.push(OscPacket::Message(OscMessage {
				addr: format!("/avatar/parameters/FT/v2/TestParameter{index}"),
				args: vec![OscType::Float(0.5)],
			}));
		}

		let datagrams = encode_packet_chunks(packets).expect("encode chunks");

		assert!(datagrams.len() > 1);
		assert!(datagrams.iter().all(|datagram| datagram.len() <= MAX_OSC_DATAGRAM_BYTES));
	}

	#[test]
	fn passes_through_vrcft_parameter_names() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("v2/JawX", -0.5));

		let packets = vrc_osc_packets_for_frame(&frame, &VrcOscOutputOptions::default());

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/v2/JawX", -0.5));
	}

	#[test]
	fn converts_blink_to_vrcft_eye_lid_openness() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("Blink_L", 1.0));

		let packets = vrc_osc_packets_for_frame(&frame, &VrcOscOutputOptions::default());

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/v2/EyeLidLeft", 0.0));
	}

	#[test]
	fn clamps_regular_parameters_to_positive_range() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.jawOpen", 2.0));

		let packets = vrc_osc_packets_for_frame(&frame, &VrcOscOutputOptions::default());

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/v2/JawOpen", 1.0));
	}

	#[test]
	fn emits_no_packets_for_unknown_or_empty_frame() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("head.yaw", 0.5));

		assert!(vrc_osc_packets_for_frame(&frame, &VrcOscOutputOptions::default()).is_empty());
	}
}
