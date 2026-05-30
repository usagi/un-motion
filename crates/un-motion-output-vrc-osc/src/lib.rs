use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};

use anyhow::Context;
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use serde::{Deserialize, Serialize};
use un_motion_frame::{MotionSignalValue, SampleState, UNMotionFrame};

const AVATAR_PARAMETER_PREFIX: &str = "/avatar/parameters/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VrcOscOutputOptions {
	pub parameter_prefix: String,
}

impl Default for VrcOscOutputOptions {
	fn default() -> Self {
		Self {
			parameter_prefix: String::new(),
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct VrcOscParameter {
	pub name: String,
	pub value: f32,
}

#[derive(Debug)]
pub struct VrcOscOutputSink {
	socket: UdpSocket,
	target: SocketAddr,
	options: VrcOscOutputOptions,
}

impl VrcOscOutputSink {
	pub fn new(target: SocketAddr) -> anyhow::Result<Self> {
		let socket = UdpSocket::bind("0.0.0.0:0").context("VRC OSC UDP socket bind failed")?;
		Ok(Self {
			socket,
			target,
			options: VrcOscOutputOptions::default(),
		})
	}

	pub fn with_options(mut self, options: VrcOscOutputOptions) -> Self {
		self.options = options;
		self
	}

	pub fn send(&self, frame: &UNMotionFrame) -> anyhow::Result<(u64, u64)> {
		let packets = vrc_osc_packets_for_frame(frame, &self.options);
		if packets.is_empty() {
			return Ok((0, 0));
		}
		let packet_count = packets.len() as u64;
		let encoded = encoder::encode(&OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content: packets,
		}))
		.context("VRC OSC encode failed")?;
		self.socket
			.send_to(&encoded, self.target)
			.with_context(|| format!("VRC OSC UDP send failed: {}", self.target))?;
		Ok((1, packet_count))
	}
}

pub fn vrc_osc_packets_for_frame(frame: &UNMotionFrame, options: &VrcOscOutputOptions) -> Vec<OscPacket> {
	vrc_osc_parameters_for_frame(frame, options)
		.into_iter()
		.map(|parameter| {
			OscPacket::Message(OscMessage {
				addr: format!("{AVATAR_PARAMETER_PREFIX}{}", parameter.name),
				args: vec![OscType::Float(parameter.value)],
			})
		})
		.collect()
}

pub fn vrc_osc_parameters_for_frame(frame: &UNMotionFrame, options: &VrcOscOutputOptions) -> Vec<VrcOscParameter> {
	let mut parameters = BTreeMap::<String, f32>::new();
	if let Some(face) = &frame.face {
		if face.tracking_state != un_motion_frame::TrackingState::Lost {
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
	parameters
		.into_iter()
		.map(|(name, value)| VrcOscParameter { name, value })
		.collect()
}

fn sample_state_is_usable(state: SampleState) -> bool {
	matches!(state, SampleState::Valid | SampleState::Held | SampleState::Decayed)
}

fn map_expression_name(name: &str, value: f32, options: &VrcOscOutputOptions) -> Option<(String, f32)> {
	if let Some(parameter) = normalize_vrcft_parameter_name(name, options) {
		return Some((parameter, clamp_for_parameter(name, value)));
	}
	let normalized = normalize_input_name(name);
	let route = arkit_to_vrcft_route(normalized)?;
	let mut value = value;
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct VrcftRoute {
	parameter: &'static str,
	invert_openness: bool,
}

impl VrcftRoute {
	const fn new(parameter: &'static str) -> Self {
		Self {
			parameter,
			invert_openness: false,
		}
	}

	const fn inverted_openness(parameter: &'static str) -> Self {
		Self {
			parameter,
			invert_openness: true,
		}
	}
}

fn arkit_to_vrcft_route(name: &str) -> Option<VrcftRoute> {
	let route = match name {
		"eyeBlinkLeft" => VrcftRoute::inverted_openness("v2/EyeLidLeft"),
		"eyeBlinkRight" => VrcftRoute::inverted_openness("v2/EyeLidRight"),
		"eyeLookOutLeft" | "eyeLookInRight" | "eye.left.yaw" => VrcftRoute::new("v2/EyeLeftX"),
		"eyeLookInLeft" | "eyeLookOutRight" | "eye.right.yaw" => VrcftRoute::new("v2/EyeRightX"),
		"eyeLookUpLeft" | "eye.left.pitch" => VrcftRoute::new("v2/EyeLeftY"),
		"eyeLookUpRight" | "eye.right.pitch" => VrcftRoute::new("v2/EyeRightY"),
		"eyeSquintLeft" => VrcftRoute::new("v2/EyeSquintLeft"),
		"eyeSquintRight" => VrcftRoute::new("v2/EyeSquintRight"),
		"browDownLeft" => VrcftRoute::new("v2/BrowLowererLeft"),
		"browDownRight" => VrcftRoute::new("v2/BrowLowererRight"),
		"browInnerUp" => VrcftRoute::new("v2/BrowInnerUp"),
		"browOuterUpLeft" => VrcftRoute::new("v2/BrowOuterUpLeft"),
		"browOuterUpRight" => VrcftRoute::new("v2/BrowOuterUpRight"),
		"cheekPuff" => VrcftRoute::new("v2/CheekPuffSuck"),
		"cheekSquintLeft" => VrcftRoute::new("v2/CheekSquintLeft"),
		"cheekSquintRight" => VrcftRoute::new("v2/CheekSquintRight"),
		"jawOpen" => VrcftRoute::new("v2/JawOpen"),
		"jawForward" => VrcftRoute::new("v2/JawZ"),
		"jawLeft" | "jawRight" => VrcftRoute::new("v2/JawX"),
		"mouthClose" => VrcftRoute::new("v2/MouthClosed"),
		"mouthFunnel" => VrcftRoute::new("v2/LipFunnel"),
		"mouthPucker" => VrcftRoute::new("v2/LipPucker"),
		"mouthLeft" | "mouthRight" => VrcftRoute::new("v2/MouthX"),
		"mouthSmileLeft" => VrcftRoute::new("v2/MouthSmileLeft"),
		"mouthSmileRight" => VrcftRoute::new("v2/MouthSmileRight"),
		"mouthFrownLeft" => VrcftRoute::new("v2/MouthSadLeft"),
		"mouthFrownRight" => VrcftRoute::new("v2/MouthSadRight"),
		"mouthDimpleLeft" => VrcftRoute::new("v2/MouthDimpleLeft"),
		"mouthDimpleRight" => VrcftRoute::new("v2/MouthDimpleRight"),
		"mouthStretchLeft" => VrcftRoute::new("v2/MouthStretchLeft"),
		"mouthStretchRight" => VrcftRoute::new("v2/MouthStretchRight"),
		"mouthRollLower" => VrcftRoute::new("v2/LipSuckLower"),
		"mouthRollUpper" => VrcftRoute::new("v2/LipSuckUpper"),
		"mouthShrugLower" => VrcftRoute::new("v2/MouthRaiserLower"),
		"mouthShrugUpper" => VrcftRoute::new("v2/MouthRaiserUpper"),
		"mouthPressLeft" => VrcftRoute::new("v2/MouthPressLeft"),
		"mouthPressRight" => VrcftRoute::new("v2/MouthPressRight"),
		"mouthLowerDownLeft" => VrcftRoute::new("v2/MouthLowerDownLeft"),
		"mouthLowerDownRight" => VrcftRoute::new("v2/MouthLowerDownRight"),
		"mouthUpperUpLeft" => VrcftRoute::new("v2/MouthUpperUpLeft"),
		"mouthUpperUpRight" => VrcftRoute::new("v2/MouthUpperUpRight"),
		"noseSneerLeft" => VrcftRoute::new("v2/NoseSneerLeft"),
		"noseSneerRight" => VrcftRoute::new("v2/NoseSneerRight"),
		"tongueOut" => VrcftRoute::new("v2/TongueOut"),
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

	#[test]
	fn maps_arkit_expression_to_vrcft_avatar_parameter() {
		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![expression("jawOpen", 0.42)],
		});

		let packets = vrc_osc_packets_for_frame(&frame, &VrcOscOutputOptions::default());

		assert_eq!(packets.len(), 1);
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
			},
		);

		assert_eq!(
			packet_value(&packets[0]),
			("/avatar/parameters/ExamplePrefix/v2/MouthSmileLeft", 0.6)
		);
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
		frame.signals.push(scalar("face.eyeBlinkLeft", 1.0));

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
