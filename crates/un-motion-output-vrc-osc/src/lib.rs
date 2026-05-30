use std::collections::{BTreeMap, BTreeSet};
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
	pub avatar_parameters: Option<VrcOscAvatarParameters>,
}

impl Default for VrcOscOutputOptions {
	fn default() -> Self {
		Self {
			parameter_prefix: String::new(),
			emit_binary_parameters: true,
			avatar_parameters: None,
		}
	}
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VrcOscAvatarParameters {
	#[serde(default)]
	float_parameters: BTreeSet<String>,
	#[serde(default)]
	bool_parameters: BTreeSet<String>,
}

impl VrcOscAvatarParameters {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn insert_float(&mut self, parameter: impl Into<String>) {
		self.float_parameters.insert(normalize_avatar_parameter_key(parameter.into()));
	}

	pub fn insert_bool(&mut self, parameter: impl Into<String>) {
		self.bool_parameters.insert(normalize_avatar_parameter_key(parameter.into()));
	}

	pub fn supports_float(&self, parameter: &str) -> bool {
		self.float_parameters.contains(&normalize_avatar_parameter_key(parameter))
	}

	pub fn supports_bool(&self, parameter: &str) -> bool {
		self.bool_parameters.contains(&normalize_avatar_parameter_key(parameter))
	}

	fn binary_spec(&self, parameter: &str) -> Option<BinaryParameterSpec> {
		let parameter = normalize_avatar_parameter_key(parameter);
		let bits = [1_u8, 2, 4, 8]
			.into_iter()
			.filter(|bit| self.bool_parameters.contains(&format!("{parameter}{bit}")))
			.count() as u8;
		if bits == 0 {
			return None;
		}
		Some(BinaryParameterSpec {
			bits,
			negative: self.bool_parameters.contains(&format!("{parameter}Negative")),
		})
	}
}

fn normalize_avatar_parameter_key(parameter: impl AsRef<str>) -> String {
	parameter
		.as_ref()
		.trim()
		.trim_start_matches(AVATAR_PARAMETER_PREFIX)
		.trim_start_matches('/')
		.to_string()
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

	pub fn set_avatar_parameters(&mut self, avatar_parameters: Option<VrcOscAvatarParameters>) {
		if self.options.avatar_parameters != avatar_parameters {
			self.options.avatar_parameters = avatar_parameters;
			self.last_values.clear();
		}
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
				if sample_state_is_usable(expression.state) {
					merge_expression_parameters(&mut parameters, &expression.name, expression.value, options);
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
				merge_expression_parameters(&mut parameters, &signal.name, value, options);
			}
			MotionSignalValue::Bool(value) => {
				if let Some(name) = normalize_vrcft_parameter_name(&signal.name, options) {
					merge_parameter_value(&mut parameters, name, if value { 1.0 } else { 0.0 });
				}
			}
			_ => {}
		}
	}
	augment_combined_eye_axes(&mut parameters, options);
	let has_parameters = !parameters.is_empty();
	let mut output = Vec::new();
	for (name, value) in &parameters {
		if options.avatar_parameters.as_ref().is_none_or(|avatar| avatar.supports_float(&name)) {
			output.push(VrcOscParameter {
				name: name.clone(),
				value: VrcOscParameterValue::Float(*value),
			});
		}
		if options.avatar_parameters.as_ref().is_some_and(|avatar| avatar.supports_bool(&name)) {
			output.push(VrcOscParameter {
				name: name.clone(),
				value: VrcOscParameterValue::Bool(base_bool_value_for_parameter(name, *value)),
			});
		}
		if options.emit_binary_parameters {
			output.extend(binary_parameters_for(
				&binary_parameter_name_for(&name),
				*value,
				options.avatar_parameters.as_ref(),
			));
		}
	}
	if has_parameters || face_is_live {
		append_eye_blink_binary_fallbacks(&mut output, &parameters, options);
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

fn merge_expression_parameters(parameters: &mut BTreeMap<String, f32>, name: &str, value: f32, options: &VrcOscOutputOptions) {
	let normalized = normalize_input_name(name).to_ascii_lowercase();
	if matches!(normalized.as_str(), "blink" | "eyeblink") {
		let left = apply_parameter_prefix("v2/EyeLidLeft", &options.parameter_prefix);
		let right = apply_parameter_prefix("v2/EyeLidRight", &options.parameter_prefix);
		let value = clamp_for_parameter("v2/EyeLidLeft", 0.75 * (1.0 - value.clamp(0.0, 1.0)));
		merge_parameter_value(parameters, left, value);
		merge_parameter_value(parameters, right, value);
		return;
	}
	if let Some((name, value)) = map_expression_name(name, value, options) {
		merge_parameter_value(parameters, name, value);
	}
}

fn append_eye_blink_binary_fallbacks(output: &mut Vec<VrcOscParameter>, parameters: &BTreeMap<String, f32>, options: &VrcOscOutputOptions) {
	append_eye_blink_binary_fallback(output, parameters, options, "v2/EyeLidLeft", "v2/EyeSquintLeft");
	append_eye_blink_binary_fallback(output, parameters, options, "v2/EyeLidRight", "v2/EyeSquintRight");
}

fn append_eye_blink_binary_fallback(
	output: &mut Vec<VrcOscParameter>,
	parameters: &BTreeMap<String, f32>,
	options: &VrcOscOutputOptions,
	eye_lid_parameter: &str,
	eye_squint_parameter: &str,
) {
	let Some(avatar_parameters) = options.avatar_parameters.as_ref() else {
		return;
	};
	let eye_lid_parameter = apply_parameter_prefix(eye_lid_parameter, &options.parameter_prefix);
	let eye_squint_parameter = apply_parameter_prefix(eye_squint_parameter, &options.parameter_prefix);
	let closure = parameters
		.get(&eye_lid_parameter)
		.map(|openness| (1.0 - (*openness / 0.75)).clamp(0.0, 1.0))
		.unwrap_or(0.0);
	append_binary_value_if_avatar_supports(output, avatar_parameters, &eye_squint_parameter, closure);
}

fn append_binary_value_if_avatar_supports(
	output: &mut Vec<VrcOscParameter>,
	avatar_parameters: &VrcOscAvatarParameters,
	parameter: &str,
	value: f32,
) {
	let Some(spec) = avatar_parameters.binary_spec(&parameter) else {
		return;
	};
	let magnitude = value.abs().clamp(0.0, 1.0);
	let quantized = if magnitude < BINARY_PARAMETER_DEADZONE {
		0
	} else if magnitude >= 0.99999 {
		(1 << spec.bits) - 1
	} else {
		(magnitude * (1 << spec.bits) as f32).floor() as u8
	};
	if spec.negative {
		output.push(VrcOscParameter {
			name: format!("{parameter}Negative"),
			value: VrcOscParameterValue::Bool(value < 0.0 && quantized > 0),
		});
	}
	for bit in 0..spec.bits {
		output.push(VrcOscParameter {
			name: format!("{parameter}{}", 1 << bit),
			value: VrcOscParameterValue::Bool(((quantized >> bit) & 1) == 1),
		});
	}
}

fn augment_combined_eye_axes(parameters: &mut BTreeMap<String, f32>, options: &VrcOscOutputOptions) {
	let Some(avatar_parameters) = options.avatar_parameters.as_ref() else {
		return;
	};
	let eye_x = apply_parameter_prefix("v2/EyeX", &options.parameter_prefix);
	let eye_left_x = apply_parameter_prefix("v2/EyeLeftX", &options.parameter_prefix);
	let eye_right_x = apply_parameter_prefix("v2/EyeRightX", &options.parameter_prefix);
	let eye_y = apply_parameter_prefix("v2/EyeY", &options.parameter_prefix);
	let eye_left_y = apply_parameter_prefix("v2/EyeLeftY", &options.parameter_prefix);
	let eye_right_y = apply_parameter_prefix("v2/EyeRightY", &options.parameter_prefix);
	if avatar_parameters.supports_float(&eye_x)
		&& !parameters.contains_key(&eye_x)
		&& let Some(value) = average_existing_values(parameters, &eye_left_x, &eye_right_x)
	{
		parameters.insert(eye_x, value);
	}
	if avatar_parameters.supports_float(&eye_y)
		&& !parameters.contains_key(&eye_y)
		&& let Some(value) = average_existing_values(parameters, &eye_left_y, &eye_right_y)
	{
		parameters.insert(eye_y, value);
	}
}

fn average_existing_values(parameters: &BTreeMap<String, f32>, left: &str, right: &str) -> Option<f32> {
	match (parameters.get(left), parameters.get(right)) {
		(Some(left), Some(right)) => Some((left + right) * 0.5),
		(Some(value), None) | (None, Some(value)) => Some(*value),
		(None, None) => None,
	}
}

fn merge_parameter_value(parameters: &mut BTreeMap<String, f32>, name: String, value: f32) {
	parameters
		.entry(name.clone())
		.and_modify(|current| {
			if vrcft_parameter_prefers_lower_value(&name) {
				if value < *current {
					*current = value;
				}
			} else if value.abs() > current.abs() {
				*current = value;
			}
		})
		.or_insert(value);
}

fn vrcft_parameter_prefers_lower_value(parameter: &str) -> bool {
	matches!(
		parameter.rsplit('/').next().unwrap_or(parameter),
		"EyeLidLeft" | "EyeLidRight" | "EyeLid"
	)
}

fn base_bool_value_for_parameter(_parameter: &str, value: f32) -> bool {
	value < 0.5
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

fn binary_parameters_for(parameter: &str, value: f32, avatar_parameters: Option<&VrcOscAvatarParameters>) -> Vec<VrcOscParameter> {
	let Some(spec) = binary_parameter_spec(parameter, avatar_parameters) else {
		return Vec::new();
	};
	let magnitude = value.abs().clamp(0.0, 1.0);
	let quantized = if magnitude < BINARY_PARAMETER_DEADZONE {
		0
	} else if magnitude >= 0.99999 {
		(1 << spec.bits) - 1
	} else {
		(magnitude * (1 << spec.bits) as f32).floor() as u8
	};
	let mut parameters = Vec::with_capacity(spec.bits as usize + usize::from(spec.negative));
	if spec.negative {
		parameters.push(VrcOscParameter {
			name: format!("{parameter}Negative"),
			value: VrcOscParameterValue::Bool(value < 0.0 && quantized > 0),
		});
	}
	for bit in 0..spec.bits {
		let suffix = 1 << bit;
		parameters.push(VrcOscParameter {
			name: format!("{parameter}{suffix}"),
			value: VrcOscParameterValue::Bool(((quantized >> bit) & 1) == 1),
		});
	}
	parameters
}

fn binary_parameter_name_for(parameter: &str) -> String {
	let Some((prefix, base)) = parameter.rsplit_once('/') else {
		return parameter.to_string();
	};
	let binary_base = match base {
		"BrowLowererLeft" | "BrowOuterUpLeft" | "BrowInnerUp" => "BrowExpressionLeft",
		"BrowLowererRight" | "BrowOuterUpRight" => "BrowExpressionRight",
		"CheekPuffSuck" | "CheekPuffSuckLeft" | "CheekPuffSuckRight" => "CheekPuffLeft",
		"MouthLowerDownLeft" | "MouthLowerDownRight" => "MouthLowerDown",
		"MouthUpperUpLeft" | "MouthUpperUpRight" => "MouthUpperUp",
		"MouthPressLeft" | "MouthPressRight" => "MouthPress",
		"NoseSneerLeft" | "NoseSneerRight" => "NoseSneer",
		other => other,
	};
	format!("{prefix}/{binary_base}")
}

#[derive(Clone, Copy)]
struct BinaryParameterSpec {
	bits: u8,
	negative: bool,
}

fn binary_parameter_spec(parameter: &str, avatar_parameters: Option<&VrcOscAvatarParameters>) -> Option<BinaryParameterSpec> {
	let parameter_base = parameter.rsplit('/').next().unwrap_or(parameter);
	if matches!(parameter_base, "EyeSquintLeft" | "EyeSquintRight") {
		return None;
	}
	if let Some(avatar_parameters) = avatar_parameters {
		return avatar_parameters.binary_spec(parameter);
	}
	let negative = matches!(
		parameter_base,
		"BrowExpressionLeft" | "BrowExpressionRight" | "JawX" | "MouthX" | "SmileFrownLeft" | "SmileFrownRight"
	);
	let bits = match parameter_base {
		"MouthX" => 4,
		_ => 3,
	};
	let supported = matches!(
		parameter_base,
		"BrowExpressionLeft"
			| "BrowExpressionRight"
			| "CheekPuffLeft"
			| "JawForward"
			| "JawX" | "LipFunnel"
			| "LipPucker"
			| "LipSuckLower"
			| "LipSuckUpper"
			| "MouthLowerDown"
			| "MouthPress"
			| "MouthRaiserLower"
			| "MouthRaiserUpper"
			| "MouthStretchLeft"
			| "MouthStretchRight"
			| "MouthX"
			| "NoseSneer"
			| "SmileFrownLeft"
			| "SmileFrownRight"
			| "TongueOut"
	);
	supported.then_some(BinaryParameterSpec { bits, negative })
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
		"eyelookinleft" | "eye.left.yaw" => VrcftRoute::new("v2/EyeLeftX"),
		"eyelookoutleft" => VrcftRoute::signed("v2/EyeLeftX", -1.0),
		"eyelookoutright" | "eye.right.yaw" => VrcftRoute::new("v2/EyeRightX"),
		"eyelookinright" => VrcftRoute::signed("v2/EyeRightX", -1.0),
		"eyelookupleft" | "eye.left.pitch" => VrcftRoute::new("v2/EyeLeftY"),
		"eyelookdownleft" => VrcftRoute::signed("v2/EyeLeftY", -1.0),
		"eyelookupright" | "eye.right.pitch" => VrcftRoute::new("v2/EyeRightY"),
		"eyelookdownright" => VrcftRoute::signed("v2/EyeRightY", -1.0),
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

	fn try_packet_value(packet: &OscPacket) -> Option<(&str, f32)> {
		let OscPacket::Message(message) = packet else {
			return None;
		};
		let OscType::Float(value) = message.args[0] else {
			return None;
		};
		Some((&message.addr, value))
	}

	fn packet_address(packet: &OscPacket) -> &str {
		let OscPacket::Message(message) = packet else {
			panic!("packet should be message");
		};
		&message.addr
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
				..VrcOscOutputOptions::default()
			},
		);

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/FT/v2/SmileFrownRight", 0.6));
		assert_eq!(
			bool_packet_value(&packets[1]),
			("/avatar/parameters/FT/v2/SmileFrownRightNegative", false)
		);
		assert_eq!(bool_packet_value(&packets[2]), ("/avatar/parameters/FT/v2/SmileFrownRight1", false));
		assert_eq!(bool_packet_value(&packets[3]), ("/avatar/parameters/FT/v2/SmileFrownRight2", false));
		assert_eq!(bool_packet_value(&packets[4]), ("/avatar/parameters/FT/v2/SmileFrownRight4", true));
	}

	#[test]
	fn emits_binary_fallback_by_default_for_binary_only_vrcft_shapes() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.tongueOut", 0.42));
		frame.signals.push(scalar("face.cheekPuff", 0.5));

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				..VrcOscOutputOptions::default()
			},
		);

		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/TongueOut2", true)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/CheekPuffLeft4", true)))
		);
		assert!(
			packets
				.iter()
				.all(|packet| try_bool_packet_value(packet) != Some(("/avatar/parameters/FT/v2/JawOpen1", true)))
		);
	}

	#[test]
	fn maps_arkit_eye_gaze_without_squint_side_effects() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.eyeLookInLeft", 0.4));
		frame.signals.push(scalar("face.eyeLookOutRight", 0.35));

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				..VrcOscOutputOptions::default()
			},
		);

		assert!(
			packets
				.iter()
				.any(|packet| try_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeLeftX", 0.4)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeRightX", 0.35)))
		);
		assert!(packets.iter().all(|packet| !packet_address(packet).contains("EyeSquint")));
	}

	#[test]
	fn uses_avatar_parameter_capabilities_for_float_and_binary_shapes() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.eyeSquintLeft", 0.24));
		frame.signals.push(scalar("face.tongueOut", 0.7));
		frame.signals.push(scalar("face.mouthFrownLeft", 0.6));
		frame.signals.push(scalar("face.jawOpen", 0.4));

		let mut avatar = VrcOscAvatarParameters::new();
		avatar.insert_float("FT/v2/JawOpen");
		avatar.insert_bool("FT/v2/EyeSquintLeft1");
		avatar.insert_bool("FT/v2/EyeSquintLeft2");
		avatar.insert_bool("FT/v2/EyeSquintLeft4");
		avatar.insert_bool("FT/v2/TongueOut1");
		avatar.insert_bool("FT/v2/TongueOut2");
		avatar.insert_bool("FT/v2/SmileFrownLeft1");
		avatar.insert_bool("FT/v2/SmileFrownLeft2");
		avatar.insert_bool("FT/v2/SmileFrownLeft4");
		avatar.insert_bool("FT/v2/SmileFrownLeftNegative");

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				avatar_parameters: Some(avatar),
				..VrcOscOutputOptions::default()
			},
		);

		assert!(
			packets
				.iter()
				.any(|packet| try_packet_value(packet) == Some(("/avatar/parameters/FT/v2/JawOpen", 0.4)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/TongueOut2", true)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/SmileFrownLeftNegative", true)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeSquintLeft1", false)))
		);
		assert!(
			packets
				.iter()
				.filter(|packet| packet_address(packet).contains("EyeSquint"))
				.all(|packet| { try_bool_packet_value(packet).map(|(_, value)| !value).unwrap_or(false) })
		);
		assert!(packets.iter().all(|packet| !packet_address(packet).contains("TongueOut4")));
		assert!(packets.iter().all(|packet| !packet_address(packet).contains("JawOpen1")));
	}

	#[test]
	fn drives_eye_squint_binary_only_from_blink_openness() {
		let mut frame = UNMotionFrame::new(1);
		frame.signals.push(scalar("face.eyeBlinkLeft", 1.0));
		frame.signals.push(scalar("face.eyeBlinkRight", 0.0));
		frame.signals.push(scalar("face.eyeSquintRight", 1.0));

		let mut avatar = VrcOscAvatarParameters::new();
		avatar.insert_float("FT/v2/EyeLidLeft");
		avatar.insert_float("FT/v2/EyeLidRight");
		avatar.insert_bool("FT/v2/EyeSquintLeft1");
		avatar.insert_bool("FT/v2/EyeSquintLeft2");
		avatar.insert_bool("FT/v2/EyeSquintLeft4");
		avatar.insert_bool("FT/v2/EyeSquintRight1");
		avatar.insert_bool("FT/v2/EyeSquintRight2");
		avatar.insert_bool("FT/v2/EyeSquintRight4");

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				avatar_parameters: Some(avatar),
				..VrcOscOutputOptions::default()
			},
		);

		assert!(
			packets
				.iter()
				.any(|packet| try_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeLidLeft", 0.0)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeLidRight", 0.75)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeSquintLeft4", true)))
		);
		assert!(
			packets
				.iter()
				.filter(|packet| packet_address(packet).contains("EyeSquintRight"))
				.all(|packet| { try_bool_packet_value(packet).map(|(_, value)| !value).unwrap_or(false) })
		);
	}

	#[test]
	fn maps_combined_vrm_blink_to_both_eyelids() {
		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![expression("Blink", 1.0)],
		});

		let mut avatar = VrcOscAvatarParameters::new();
		avatar.insert_float("FT/v2/EyeLidLeft");
		avatar.insert_float("FT/v2/EyeLidRight");
		avatar.insert_bool("FT/v2/EyeSquintLeft1");
		avatar.insert_bool("FT/v2/EyeSquintLeft2");
		avatar.insert_bool("FT/v2/EyeSquintLeft4");
		avatar.insert_bool("FT/v2/EyeSquintRight1");
		avatar.insert_bool("FT/v2/EyeSquintRight2");
		avatar.insert_bool("FT/v2/EyeSquintRight4");

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				avatar_parameters: Some(avatar),
				..VrcOscOutputOptions::default()
			},
		);

		assert!(
			packets
				.iter()
				.any(|packet| try_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeLidLeft", 0.0)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeLidRight", 0.0)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeSquintLeft4", true)))
		);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeSquintRight4", true)))
		);
	}

	#[test]
	fn emits_same_name_bool_parameters_like_vrcft_eparam() {
		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![expression("Blink_L", 1.0)],
		});

		let mut avatar = VrcOscAvatarParameters::new();
		avatar.insert_bool("FT/v2/EyeLidLeft");

		let packets = vrc_osc_packets_for_frame(
			&frame,
			&VrcOscOutputOptions {
				parameter_prefix: "FT".to_string(),
				avatar_parameters: Some(avatar),
				..VrcOscOutputOptions::default()
			},
		);

		assert_eq!(packets.len(), 5);
		assert!(
			packets
				.iter()
				.any(|packet| try_bool_packet_value(packet) == Some(("/avatar/parameters/FT/v2/EyeLidLeft", true)))
		);
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
				..VrcOscOutputOptions::default()
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
				..VrcOscOutputOptions::default()
			},
		);

		assert_eq!(packet_value(&packets[0]), ("/avatar/parameters/FT/v2/NoseSneerLeft", 0.08));
		assert_eq!(bool_packet_value(&packets[1]), ("/avatar/parameters/FT/v2/NoseSneer1", false));
		assert_eq!(bool_packet_value(&packets[2]), ("/avatar/parameters/FT/v2/NoseSneer2", false));
		assert_eq!(bool_packet_value(&packets[3]), ("/avatar/parameters/FT/v2/NoseSneer4", false));
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
