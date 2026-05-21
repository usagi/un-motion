use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};

use anyhow::Context;
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use un_motion_frame::{Finger, HandMotion, HumanoidBone, MotionSignalValue, Quatf, TransformSample, UNMotionFrame, Vec3f};
use un_motion_interfaces::OutputSink;

#[derive(Clone, Debug, PartialEq)]
pub struct BlendshapeRoute {
	pub blendshape_name: String,
	pub scale: f32,
	pub offset: f32,
	pub clamp_min: Option<f32>,
	pub clamp_max: Option<f32>,
}

impl BlendshapeRoute {
	pub fn simple(name: impl Into<String>) -> Self {
		Self {
			blendshape_name: name.into(),
			scale: 1.0,
			offset: 0.0,
			clamp_min: None,
			clamp_max: None,
		}
	}

	fn map_value(&self, value: f32) -> f32 {
		let mut out = (value * self.scale) + self.offset;
		if let Some(min) = self.clamp_min {
			out = out.max(min);
		}
		if let Some(max) = self.clamp_max {
			out = out.min(max);
		}
		out
	}
}

#[derive(Debug)]
pub struct VmcOutputSink {
	socket: UdpSocket,
	target: SocketAddr,
	send_ok_packet: bool,
	blendshape_map: HashMap<String, BlendshapeRoute>,
	chest_stabilization: ChestStabilizationOptions,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChestStabilizationOptions {
	pub enabled: bool,
	pub strength: f32,
}

impl ChestStabilizationOptions {
	pub fn disabled() -> Self {
		Self {
			enabled: false,
			strength: 0.0,
		}
	}

	pub fn new(enabled: bool, strength: f32) -> Self {
		Self {
			enabled,
			strength: strength.clamp(0.0, 1.0),
		}
	}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VmcFramePose {
	pub root: Option<VmcPoseTransform>,
	pub bones: Vec<VmcPoseTransform>,
	pub blendshapes: Vec<(String, f32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VmcPoseTransform {
	pub name: String,
	pub position: [f32; 3],
	pub rotation: [f32; 4],
}

impl VmcPoseTransform {
	pub fn new(name: impl Into<String>, position: [f32; 3], rotation: [f32; 4]) -> Self {
		Self {
			name: name.into(),
			position,
			rotation,
		}
	}
}

pub fn vmc_frame_pose_for_unmotion_frame(frame: &UNMotionFrame) -> VmcFramePose {
	vmc_frame_pose_for_unmotion_frame_with_options(frame, &default_blendshape_map(), ChestStabilizationOptions::disabled())
}

pub fn vmc_signal_pose_for_unmotion_frame(frame: &UNMotionFrame) -> VmcFramePose {
	vmc_signal_pose_for_unmotion_frame_with_options(frame, &default_blendshape_map(), ChestStabilizationOptions::disabled())
}

pub fn vmc_signal_pose_for_unmotion_frame_with_options(
	frame: &UNMotionFrame,
	blendshape_map: &HashMap<String, BlendshapeRoute>,
	chest_stabilization: ChestStabilizationOptions,
) -> VmcFramePose {
	let mut pose = VmcFramePose::default();
	if frame_has_vmc_bone_signals(frame) || frame_has_hand_motion(frame) {
		pose.root = Some(default_root_pose());
		pose.bones.extend(HUMANOID_BONE_REST_POSES.iter().map(|rest| {
			VmcPoseTransform::new(
				rest.name,
				rest.position,
				stabilize_chest_bone_rotation(rest.name, bone_rotation(frame, rest.name), chest_stabilization),
			)
		}));
	}
	if let Some(face) = &frame.face {
		for expression in &face.expressions {
			if let Some(route) = blendshape_map.get(&expression.name) {
				pose.blendshapes
					.push((route.blendshape_name.clone(), route.map_value(expression.value)));
			} else {
				pose.blendshapes.push((expression.name.clone(), expression.value.clamp(0.0, 1.0)));
			}
		}
	}
	for signal in &frame.signals {
		if let Some(route) = blendshape_map.get(&signal.name) {
			if let MotionSignalValue::Scalar(value) = signal.value {
				if !pose.blendshapes.iter().any(|(name, _)| name == &route.blendshape_name) {
					pose.blendshapes.push((route.blendshape_name.clone(), route.map_value(value)));
				}
			}
		}
	}
	pose
}

pub fn vmc_frame_pose_for_unmotion_frame_with_options(
	frame: &UNMotionFrame,
	blendshape_map: &HashMap<String, BlendshapeRoute>,
	chest_stabilization: ChestStabilizationOptions,
) -> VmcFramePose {
	let mut pose = VmcFramePose::default();
	if frame_has_direct_humanoid_pose(frame) {
		apply_direct_humanoid_pose(frame, &mut pose, chest_stabilization);
	} else if frame_has_hand_motion(frame) {
		pose.root = Some(default_root_pose());
		pose.bones.extend(rest_humanoid_pose_transforms());
		for transform in hand_motion_transforms(frame) {
			upsert_pose_transform(&mut pose.bones, transform);
		}
	}

	if let Some(face) = &frame.face {
		for expression in &face.expressions {
			if let Some(route) = blendshape_map.get(&expression.name) {
				pose.blendshapes
					.push((route.blendshape_name.clone(), route.map_value(expression.value)));
			} else {
				pose.blendshapes.push((expression.name.clone(), expression.value.clamp(0.0, 1.0)));
			}
		}
	}
	pose
}

impl VmcOutputSink {
	pub fn new(target: SocketAddr) -> anyhow::Result<Self> {
		let socket = UdpSocket::bind("0.0.0.0:0").context("VMC UDP socket bind failed")?;
		Ok(Self {
			socket,
			target,
			send_ok_packet: true,
			blendshape_map: default_blendshape_map(),
			chest_stabilization: ChestStabilizationOptions::disabled(),
		})
	}

	pub fn with_ok_packet(mut self, enabled: bool) -> Self {
		self.send_ok_packet = enabled;
		self
	}

	pub fn with_blendshape_map(mut self, map: HashMap<String, BlendshapeRoute>) -> Self {
		self.blendshape_map = map;
		self
	}

	pub fn with_chest_stabilization(mut self, enabled: bool, strength: f32) -> Self {
		self.chest_stabilization = ChestStabilizationOptions::new(enabled, strength);
		self
	}

	pub fn set_chest_stabilization(&mut self, enabled: bool, strength: f32) {
		self.chest_stabilization = ChestStabilizationOptions::new(enabled, strength);
	}

	fn send_packet(&self, packet: OscPacket) -> anyhow::Result<()> {
		let encoded = encoder::encode(&packet).context("OSC encode failed")?;
		self.socket
			.send_to(&encoded, self.target)
			.with_context(|| format!("VMC UDP send failed: {}", self.target))?;
		Ok(())
	}

	pub fn send_raw_datagram(&self, datagram: &[u8]) -> anyhow::Result<()> {
		self.socket
			.send_to(datagram, self.target)
			.with_context(|| format!("VMC UDP raw send failed: {}", self.target))?;
		Ok(())
	}

	fn send_packets_as_bundle(&self, packets: Vec<OscPacket>) -> anyhow::Result<()> {
		self.send_packet(OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content: packets,
		}))
	}
}

impl OutputSink<UNMotionFrame> for VmcOutputSink {
	fn send(&mut self, frame: &UNMotionFrame) -> anyhow::Result<()> {
		let packets = vmc_packets_for_frame_with_options(frame, self.send_ok_packet, &self.blendshape_map, self.chest_stabilization);
		if packets.is_empty() {
			return Ok(());
		}
		self.send_packets_as_bundle(packets)
	}
}

pub fn vmc_packets_for_frame(frame: &UNMotionFrame) -> Vec<OscPacket> {
	vmc_packets_for_frame_with_options(frame, true, &default_blendshape_map(), ChestStabilizationOptions::disabled())
}

pub fn vmc_packets_for_frame_without_ok(frame: &UNMotionFrame) -> Vec<OscPacket> {
	vmc_packets_for_frame_with_options(frame, false, &default_blendshape_map(), ChestStabilizationOptions::disabled())
}

fn vmc_packets_for_frame_with_options(
	frame: &UNMotionFrame,
	send_ok_packet: bool,
	blendshape_map: &HashMap<String, BlendshapeRoute>,
	chest_stabilization: ChestStabilizationOptions,
) -> Vec<OscPacket> {
	let mut packets = Vec::new();
	if send_ok_packet {
		packets.push(OscPacket::Message(OscMessage {
			addr: "/VMC/Ext/OK".to_string(),
			args: vec![OscType::Int(1)],
		}));
	}

	if frame_has_direct_humanoid_pose(frame) {
		packets.extend(direct_humanoid_pose_packets(frame, chest_stabilization));
	} else if frame_has_hand_motion(frame) {
		packets.push(root_pose_packet());
		packets.extend(hand_only_pose_packets(frame));
	}

	let mut sent_blend_value = false;
	if let Some(face) = &frame.face {
		for expression in &face.expressions {
			let (name, value) = if let Some(route) = blendshape_map.get(&expression.name) {
				(route.blendshape_name.clone(), route.map_value(expression.value))
			} else {
				(expression.name.clone(), expression.value.clamp(0.0, 1.0))
			};
			packets.push(OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Val".to_string(),
				args: vec![OscType::String(name), OscType::Float(value)],
			}));
			sent_blend_value = true;
		}
	}

	if sent_blend_value {
		packets.push(OscPacket::Message(OscMessage {
			addr: "/VMC/Ext/Blend/Apply".to_string(),
			args: Vec::new(),
		}));
	}

	packets
}

fn root_pose_packet() -> OscPacket {
	OscPacket::Message(OscMessage {
		addr: "/VMC/Ext/Root/Pos".to_string(),
		args: vec![
			OscType::String("root".to_string()),
			OscType::Float(0.0),
			OscType::Float(0.0),
			OscType::Float(0.0),
			OscType::Float(0.0),
			OscType::Float(0.0),
			OscType::Float(0.0),
			OscType::Float(1.0),
		],
	})
}

fn direct_humanoid_pose_packets(frame: &UNMotionFrame, chest_stabilization: ChestStabilizationOptions) -> Vec<OscPacket> {
	let Some(humanoid) = frame.body.as_ref().and_then(|body| body.humanoid.as_ref()) else {
		return Vec::new();
	};
	let mut packets = Vec::new();
	if let Some(root) = &humanoid.root {
		packets.push(root_transform_packet(root));
	} else {
		packets.push(root_pose_packet());
	}
	for bone in &humanoid.bones {
		let name = humanoid_bone_name(bone.bone);
		let rest = HUMANOID_BONE_REST_POSES
			.iter()
			.find(|rest| rest.name == name)
			.map(|rest| rest.position)
			.unwrap_or([0.0, 0.0, 0.0]);
		let mut transform = transform_to_vmc_pose(name, &bone.transform, rest);
		transform.rotation = stabilize_chest_bone_rotation(name, transform.rotation, chest_stabilization);
		packets.push(bone_pose_packet(transform.name, transform.position, transform.rotation));
	}
	packets.extend(hand_motion_packets(frame));
	packets
}

fn apply_direct_humanoid_pose(frame: &UNMotionFrame, pose: &mut VmcFramePose, chest_stabilization: ChestStabilizationOptions) {
	let Some(humanoid) = frame.body.as_ref().and_then(|body| body.humanoid.as_ref()) else {
		return;
	};
	pose.root = Some(
		humanoid
			.root
			.as_ref()
			.map(root_transform_to_vmc_pose)
			.unwrap_or_else(default_root_pose),
	);
	for bone in &humanoid.bones {
		let name = humanoid_bone_name(bone.bone);
		let rest = HUMANOID_BONE_REST_POSES
			.iter()
			.find(|rest| rest.name == name)
			.map(|rest| rest.position)
			.unwrap_or([0.0, 0.0, 0.0]);
		let mut transform = transform_to_vmc_pose(name, &bone.transform, rest);
		transform.rotation = stabilize_chest_bone_rotation(name, transform.rotation, chest_stabilization);
		pose.bones.push(transform);
	}
	for packet in hand_motion_transforms(frame) {
		upsert_pose_transform(&mut pose.bones, packet);
	}
}

fn hand_only_pose_packets(frame: &UNMotionFrame) -> Vec<OscPacket> {
	let mut transforms = rest_humanoid_pose_transforms();
	for transform in hand_motion_transforms(frame) {
		upsert_pose_transform(&mut transforms, transform);
	}
	transforms
		.into_iter()
		.map(|transform| bone_pose_packet(transform.name, transform.position, transform.rotation))
		.collect()
}

fn rest_humanoid_pose_transforms() -> Vec<VmcPoseTransform> {
	HUMANOID_BONE_REST_POSES
		.iter()
		.map(|rest| VmcPoseTransform::new(rest.name, rest.position, IDENTITY_QUAT))
		.collect()
}

fn upsert_pose_transform(transforms: &mut Vec<VmcPoseTransform>, transform: VmcPoseTransform) {
	if let Some(existing) = transforms.iter_mut().find(|existing| existing.name == transform.name) {
		*existing = transform;
	} else {
		transforms.push(transform);
	}
}

fn hand_motion_packets(frame: &UNMotionFrame) -> Vec<OscPacket> {
	hand_motion_transforms(frame)
		.into_iter()
		.map(|transform| bone_pose_packet(transform.name, transform.position, transform.rotation))
		.collect()
}

fn hand_motion_transforms(frame: &UNMotionFrame) -> Vec<VmcPoseTransform> {
	let mut out = Vec::new();
	if let Some(hand) = frame.left_hand.as_ref() {
		out.extend(hand_motion_transforms_for_side(BodySide::Left, hand));
	}
	if let Some(hand) = frame.right_hand.as_ref() {
		out.extend(hand_motion_transforms_for_side(BodySide::Right, hand));
	}
	out
}

fn hand_motion_transforms_for_side(side: BodySide, hand: &HandMotion) -> Vec<VmcPoseTransform> {
	let mut out = Vec::new();
	if let Some(wrist) = hand.wrist.as_ref() {
		let name = match side {
			BodySide::Left => "LeftHand",
			BodySide::Right => "RightHand",
		};
		let rest = rest_position_for_bone(name);
		out.push(transform_to_vmc_pose(name, wrist, rest));
	}
	for finger in &hand.fingers {
		let prefix = match (side, finger.finger) {
			(BodySide::Left, Finger::Thumb) => "LeftThumb",
			(BodySide::Left, Finger::Index) => "LeftIndex",
			(BodySide::Left, Finger::Middle) => "LeftMiddle",
			(BodySide::Left, Finger::Ring) => "LeftRing",
			(BodySide::Left, Finger::Little) => "LeftLittle",
			(BodySide::Right, Finger::Thumb) => "RightThumb",
			(BodySide::Right, Finger::Index) => "RightIndex",
			(BodySide::Right, Finger::Middle) => "RightMiddle",
			(BodySide::Right, Finger::Ring) => "RightRing",
			(BodySide::Right, Finger::Little) => "RightLittle",
		};
		for (index, joint) in finger.joints.iter().enumerate() {
			let segment = match index {
				0 => "Proximal",
				1 => "Intermediate",
				2 => "Distal",
				_ => continue,
			};
			let name = format!("{prefix}{segment}");
			out.push(finger_transform_to_vmc_pose(&name, joint, rest_position_for_bone(&name)));
		}
	}
	out
}

fn rest_position_for_bone(name: &str) -> [f32; 3] {
	HUMANOID_BONE_REST_POSES
		.iter()
		.find(|rest| rest.name == name)
		.map(|rest| rest.position)
		.unwrap_or([0.0, 0.0, 0.0])
}

fn root_transform_packet(transform: &TransformSample) -> OscPacket {
	let position = transform.translation.map(vec3_to_array).unwrap_or([0.0, 0.0, 0.0]);
	let rotation = transform.rotation.map(quat_to_array).unwrap_or(IDENTITY_QUAT);
	OscPacket::Message(OscMessage {
		addr: "/VMC/Ext/Root/Pos".to_string(),
		args: vec![
			OscType::String("root".to_string()),
			OscType::Float(position[0]),
			OscType::Float(position[1]),
			OscType::Float(position[2]),
			OscType::Float(rotation[0]),
			OscType::Float(rotation[1]),
			OscType::Float(rotation[2]),
			OscType::Float(rotation[3]),
		],
	})
}

fn root_transform_to_vmc_pose(transform: &TransformSample) -> VmcPoseTransform {
	let position = transform.translation.map(vec3_to_array).unwrap_or([0.0, 0.0, 0.0]);
	let rotation = transform.rotation.map(quat_to_array).unwrap_or(IDENTITY_QUAT);
	VmcPoseTransform::new("root", position, rotation)
}

fn transform_to_vmc_pose(name: impl Into<String>, transform: &TransformSample, fallback_position: [f32; 3]) -> VmcPoseTransform {
	let position = transform.translation.map(vec3_to_array).unwrap_or(fallback_position);
	let rotation = transform.rotation.map(quat_to_array).unwrap_or(IDENTITY_QUAT);
	VmcPoseTransform::new(name, position, rotation)
}

fn finger_transform_to_vmc_pose(name: impl Into<String>, transform: &TransformSample, fallback_position: [f32; 3]) -> VmcPoseTransform {
	let mut pose = transform_to_vmc_pose(name, transform, fallback_position);
	pose.rotation = unmotion_finger_rotation_to_vmc(pose.rotation);
	pose
}

fn unmotion_finger_rotation_to_vmc(rotation: [f32; 4]) -> [f32; 4] {
	normalize_quat([-rotation[0], -rotation[1], rotation[2], rotation[3]])
}

fn default_root_pose() -> VmcPoseTransform {
	VmcPoseTransform::new("root", [0.0, 0.0, 0.0], IDENTITY_QUAT)
}

fn vec3_to_array(value: Vec3f) -> [f32; 3] {
	[value.x, value.y, value.z]
}

fn quat_to_array(value: Quatf) -> [f32; 4] {
	[value.x, value.y, value.z, value.w]
}

fn humanoid_bone_name(bone: HumanoidBone) -> &'static str {
	match bone {
		HumanoidBone::Hips => "Hips",
		HumanoidBone::Spine => "Spine",
		HumanoidBone::Chest => "Chest",
		HumanoidBone::UpperChest => "UpperChest",
		HumanoidBone::Neck => "Neck",
		HumanoidBone::Head => "Head",
		HumanoidBone::LeftShoulder => "LeftShoulder",
		HumanoidBone::LeftUpperArm => "LeftUpperArm",
		HumanoidBone::LeftLowerArm => "LeftLowerArm",
		HumanoidBone::LeftHand => "LeftHand",
		HumanoidBone::RightShoulder => "RightShoulder",
		HumanoidBone::RightUpperArm => "RightUpperArm",
		HumanoidBone::RightLowerArm => "RightLowerArm",
		HumanoidBone::RightHand => "RightHand",
		HumanoidBone::LeftUpperLeg => "LeftUpperLeg",
		HumanoidBone::LeftLowerLeg => "LeftLowerLeg",
		HumanoidBone::LeftFoot => "LeftFoot",
		HumanoidBone::LeftToes => "LeftToes",
		HumanoidBone::RightUpperLeg => "RightUpperLeg",
		HumanoidBone::RightLowerLeg => "RightLowerLeg",
		HumanoidBone::RightFoot => "RightFoot",
		HumanoidBone::RightToes => "RightToes",
		HumanoidBone::LeftEye => "LeftEye",
		HumanoidBone::RightEye => "RightEye",
		HumanoidBone::Jaw => "Jaw",
	}
}

fn bone_pose_packet(bone: impl AsRef<str>, position: [f32; 3], rotation: [f32; 4]) -> OscPacket {
	OscPacket::Message(OscMessage {
		addr: "/VMC/Ext/Bone/Pos".to_string(),
		args: vec![
			OscType::String(bone.as_ref().to_string()),
			OscType::Float(position[0]),
			OscType::Float(position[1]),
			OscType::Float(position[2]),
			OscType::Float(rotation[0]),
			OscType::Float(rotation[1]),
			OscType::Float(rotation[2]),
			OscType::Float(rotation[3]),
		],
	})
}

#[derive(Clone, Copy, PartialEq)]
enum BodySide {
	Left,
	Right,
}

impl BodySide {
	fn signal_prefix(self) -> &'static str {
		match self {
			Self::Left => "left",
			Self::Right => "right",
		}
	}

	fn opposite(self) -> Self {
		match self {
			Self::Left => Self::Right,
			Self::Right => Self::Left,
		}
	}
}

#[derive(Clone, Copy)]
struct BoneRestPose {
	name: &'static str,
	position: [f32; 3],
}

fn bone_rotation(frame: &UNMotionFrame, bone: &str) -> [f32; 4] {
	match bone {
		"Hips" => hips_rotation(frame),
		"Spine" => spine_rotation(frame),
		"Chest" => chest_rotation(frame),
		"UpperChest" => upper_chest_rotation(frame),
		"Head" => head_rotation(frame),
		"LeftShoulder" => shoulder_rotation(frame, BodySide::Left),
		"RightShoulder" => shoulder_rotation(frame, BodySide::Right),
		"LeftUpperLeg" => upper_leg_rotation(frame, BodySide::Left),
		"RightUpperLeg" => upper_leg_rotation(frame, BodySide::Right),
		"LeftLowerLeg" => lower_leg_rotation(frame, BodySide::Left),
		"RightLowerLeg" => lower_leg_rotation(frame, BodySide::Right),
		"LeftFoot" => foot_rotation(frame, BodySide::Left),
		"RightFoot" => foot_rotation(frame, BodySide::Right),
		"LeftToes" => toes_rotation(frame, BodySide::Left),
		"RightToes" => toes_rotation(frame, BodySide::Right),
		"LeftUpperArm" => upper_arm_rotation(frame, BodySide::Left),
		"RightUpperArm" => upper_arm_rotation(frame, BodySide::Right),
		"LeftLowerArm" => lower_arm_rotation(frame, BodySide::Left),
		"RightLowerArm" => lower_arm_rotation(frame, BodySide::Right),
		"LeftHand" => hand_rotation(frame, BodySide::Left),
		"RightHand" => hand_rotation(frame, BodySide::Right),
		_ => typed_finger_rotation(frame, bone)
			.or_else(|| finger_rotation(frame, bone))
			.unwrap_or(IDENTITY_QUAT),
	}
}

fn head_rotation(frame: &UNMotionFrame) -> [f32; 4] {
	let yaw = scalar_signal(frame, "head.yaw").unwrap_or(0.0);
	let pitch = scalar_signal(frame, "head.pitch").unwrap_or(0.0);
	let roll = scalar_signal(frame, "head.roll").unwrap_or(0.0);
	euler_to_quat(-pitch, yaw, roll)
}

fn hips_rotation(frame: &UNMotionFrame) -> [f32; 4] {
	lower_body_global_rotation(frame).unwrap_or(IDENTITY_QUAT)
}

fn spine_rotation(_frame: &UNMotionFrame) -> [f32; 4] {
	IDENTITY_QUAT
}

fn chest_rotation(frame: &UNMotionFrame) -> [f32; 4] {
	let Some(global) = torso_global_rotation(frame) else {
		return IDENTITY_QUAT;
	};
	let parent = hips_rotation(frame);
	normalize_quat(quat_mul(quat_inverse(parent), global))
}

fn upper_chest_rotation(_frame: &UNMotionFrame) -> [f32; 4] {
	IDENTITY_QUAT
}

fn stabilize_chest_bone_rotation(bone: &str, rotation: [f32; 4], options: ChestStabilizationOptions) -> [f32; 4] {
	if !options.enabled || (bone != "Chest" && bone != "UpperChest") {
		return rotation;
	}
	dampen_rotation(rotation, options.strength)
}

fn shoulder_rotation(_frame: &UNMotionFrame, _side: BodySide) -> [f32; 4] {
	IDENTITY_QUAT
}

fn upper_leg_rotation(frame: &UNMotionFrame, side: BodySide) -> [f32; 4] {
	let Some(global) = upper_leg_global_rotation(frame, side) else {
		return IDENTITY_QUAT;
	};
	let parent = hips_rotation(frame);
	normalize_quat(quat_mul(quat_inverse(parent), global))
}

fn lower_leg_rotation(frame: &UNMotionFrame, side: BodySide) -> [f32; 4] {
	let Some(global) = lower_leg_global_rotation(frame, side) else {
		return IDENTITY_QUAT;
	};
	let Some(parent_global) = upper_leg_global_rotation(frame, side) else {
		return global;
	};
	normalize_quat(quat_mul(quat_inverse(parent_global), global))
}

fn foot_rotation(frame: &UNMotionFrame, side: BodySide) -> [f32; 4] {
	let Some(global) = foot_global_rotation(frame, side) else {
		return IDENTITY_QUAT;
	};
	let Some(parent_global) = lower_leg_global_rotation(frame, side) else {
		return global;
	};
	normalize_quat(quat_mul(quat_inverse(parent_global), global))
}

fn toes_rotation(frame: &UNMotionFrame, side: BodySide) -> [f32; 4] {
	let Some(global) = toes_global_rotation(frame, side) else {
		return IDENTITY_QUAT;
	};
	let Some(parent_global) = foot_global_rotation(frame, side) else {
		return global;
	};
	normalize_quat(quat_mul(quat_inverse(parent_global), global))
}

fn upper_arm_rotation(frame: &UNMotionFrame, side: BodySide) -> [f32; 4] {
	if forward_stop_palm_pose(frame, side) {
		return forward_stop_palm_upper_arm_rotation(side);
	}
	if hidden_back_hands_pose(frame) {
		return hidden_back_upper_arm_rotation(side);
	}
	let Some(global) = upper_arm_global_rotation(frame, side) else {
		return IDENTITY_QUAT;
	};
	let parent = shoulder_rotation(frame, side);
	normalize_quat(quat_mul(quat_inverse(parent), global))
}

fn lower_arm_rotation(frame: &UNMotionFrame, side: BodySide) -> [f32; 4] {
	if forward_stop_palm_pose(frame, side) {
		return forward_stop_palm_lower_arm_rotation(side);
	}
	if hidden_back_hands_pose(frame) {
		return hidden_back_lower_arm_rotation(side);
	}
	if let Some(mirrored) = mirrored_opposite_lower_arm_local_rotation(frame, side) {
		return mirrored;
	}
	let Some(global) = lower_arm_global_rotation(frame, side) else {
		return IDENTITY_QUAT;
	};
	let Some(parent_global) = upper_arm_global_rotation(frame, side) else {
		return global;
	};
	normalize_quat(quat_mul(quat_inverse(parent_global), global))
}

fn hand_rotation(frame: &UNMotionFrame, side: BodySide) -> [f32; 4] {
	if forward_stop_palm_pose(frame, side) {
		return forward_stop_palm_hand_rotation(side);
	}
	let prefix = side.signal_prefix();
	if let Some(global) = hand_palm_basis_rotation(frame, side) {
		return hand_local_rotation(frame, side, global);
	}
	if let Some(mirrored) = mirrored_opposite_hand_local_rotation(frame, side) {
		return mirrored;
	}
	let roll = scalar_signal(frame, &format!("hand.{prefix}.palm.roll")).unwrap_or(0.0);
	let z = scalar_signal(frame, &format!("hand.{prefix}.wrist.z")).unwrap_or(0.0);
	let pitch = roll.clamp(-1.0, 1.0) * 0.8 * side_sign(side);
	let yaw = z.clamp(-1.0, 1.0) * 0.25;
	euler_radians_to_quat(pitch, yaw, 0.0)
}

fn hidden_back_upper_arm_rotation(side: BodySide) -> [f32; 4] {
	normalize_quat([0.065, 0.050 * side_sign(side), 0.571 * side_sign(side), 0.817])
}

fn hidden_back_lower_arm_rotation(side: BodySide) -> [f32; 4] {
	normalize_quat([-0.079, -0.004 * side_sign(side), 0.0, 0.997])
}

fn forward_stop_palm_upper_arm_rotation(side: BodySide) -> [f32; 4] {
	match side {
		BodySide::Left => normalize_quat([0.036, 0.505, 0.408, 0.759]),
		BodySide::Right => normalize_quat([-0.142, -0.458, -0.396, 0.783]),
	}
}

fn forward_stop_palm_lower_arm_rotation(side: BodySide) -> [f32; 4] {
	match side {
		BodySide::Left => normalize_quat([-0.395, 0.224, 0.224, 0.862]),
		BodySide::Right => normalize_quat([0.210, -0.286, 0.130, 0.926]),
	}
}

fn forward_stop_palm_hand_rotation(side: BodySide) -> [f32; 4] {
	match side {
		BodySide::Left => normalize_quat([-0.462, -0.202, 0.367, 0.782]),
		BodySide::Right => normalize_quat([0.183, -0.127, 0.605, 0.764]),
	}
}

#[cfg(test)]
fn forward_stop_palms_pose(frame: &UNMotionFrame) -> bool {
	forward_stop_palm_pose(frame, BodySide::Left) && forward_stop_palm_pose(frame, BodySide::Right)
}

fn forward_stop_palm_pose(frame: &UNMotionFrame, side: BodySide) -> bool {
	let prefix = side.signal_prefix();
	let Some(lower) = arm_segment_direction(frame, prefix, "elbow", "wrist") else {
		return false;
	};
	let Some(forward) = signal_vec3(frame, &format!("hand.{prefix}.palm.forward")) else {
		return false;
	};
	let Some(across) = signal_vec3(frame, &format!("hand.{prefix}.palm.across")) else {
		return false;
	};
	if forward[1] < 0.90 || lower[2] < 0.75 || lower[1] < 0.15 || lower[1] > 0.45 || lower[0].abs() > 0.55 {
		return false;
	}
	across[2] < -0.10
}

fn hidden_back_hands_pose(frame: &UNMotionFrame) -> bool {
	if hand_palm_basis_rotation(frame, BodySide::Left).is_some() || hand_palm_basis_rotation(frame, BodySide::Right).is_some() {
		return false;
	}
	for side in [BodySide::Left, BodySide::Right] {
		let prefix = side.signal_prefix();
		let Some(shoulder) = arm_point(frame, prefix, "shoulder") else {
			return false;
		};
		let Some(elbow) = arm_point(frame, prefix, "elbow") else {
			return false;
		};
		let Some(wrist) = arm_point(frame, prefix, "wrist") else {
			return false;
		};
		if elbow[1] >= shoulder[1] - 0.08 || wrist[1] >= shoulder[1] - 0.20 {
			return false;
		}
		let wrist_toward_center = match side {
			BodySide::Left => wrist[0] > elbow[0],
			BodySide::Right => wrist[0] < elbow[0],
		};
		if !wrist_toward_center {
			return false;
		}
	}
	true
}

fn mirrored_opposite_hand_local_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	if !hands_are_close_overhead(frame) {
		return None;
	}
	let other = side.opposite();
	let other_global = hand_palm_basis_rotation(frame, other)?;
	let mirrored_global = mirror_horizontal_quat(other_global);
	let parent_global = mirrored_opposite_lower_arm_global_rotation(frame, side).or_else(|| lower_arm_global_rotation(frame, side))?;
	Some(normalize_quat(quat_mul(quat_inverse(parent_global), mirrored_global)))
}

fn mirrored_opposite_lower_arm_local_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let mirrored_global = mirrored_opposite_lower_arm_global_rotation(frame, side)?;
	let parent_global = upper_arm_global_rotation(frame, side)?;
	Some(normalize_quat(quat_mul(quat_inverse(parent_global), mirrored_global)))
}

fn mirrored_opposite_lower_arm_global_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	if !hands_are_close_overhead(frame) || hand_palm_basis_rotation(frame, side).is_some() {
		return None;
	}
	let other = side.opposite();
	hand_palm_basis_rotation(frame, other)?;
	let other_global = lower_arm_global_rotation(frame, other)?;
	Some(mirror_horizontal_quat(other_global))
}

fn hand_palm_basis_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let forward = signal_vec3(frame, &format!("hand.{prefix}.palm.forward"))?;
	let normal =
		signal_vec3(frame, &format!("hand.{prefix}.palm.normal")).or_else(|| signal_vec3(frame, &format!("hand.{prefix}.palm.across")))?;
	quat_from_basis(arm_rest_axis(side), [0.0, -1.0, 0.0], forward, normal)
}

#[cfg(test)]
fn left_extended_stop_palm_needs_across_inversion(frame: &UNMotionFrame, forward: [f32; 3]) -> bool {
	if forward[1] <= 0.70 {
		return false;
	}
	let Some(normal) = signal_vec3(frame, "hand.left.palm.normal") else {
		return false;
	};
	if normal[2] <= 0.70 || hand_finger_fold(frame, "left") >= 0.12 {
		return false;
	}
	let Some(upper) = arm_segment_direction(frame, "left", "shoulder", "elbow") else {
		return false;
	};
	let Some(lower) = arm_segment_direction(frame, "left", "elbow", "wrist") else {
		return false;
	};
	upper[1] < -0.65 && lower[1] > 0.10 && lower[1] < 0.50 && dot3(upper, lower) > 0.10
}

fn is_left_pointing_front_palm(forward: [f32; 3], normal: [f32; 3], finger_fold: f32) -> bool {
	normal[2] < -0.75 && forward[2] > 0.1 && finger_fold < 0.15
}

fn is_right_folded_front_palm(forward: [f32; 3], normal: [f32; 3], finger_fold: f32) -> bool {
	normal[2] > 0.75 && normal[2] < 0.95 && forward[2] < -0.2 && finger_fold > 0.25
}

fn hand_local_rotation(frame: &UNMotionFrame, side: BodySide, global: [f32; 4]) -> [f32; 4] {
	let Some(parent_global) = lower_arm_global_rotation(frame, side) else {
		return global;
	};
	normalize_quat(quat_mul(quat_inverse(parent_global), global))
}

fn upper_arm_global_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let target = arm_segment_direction(frame, prefix, "shoulder", "elbow")?;
	let rest = arm_rest_axis(side);
	if let Some(plane) = arm_plane_secondary(frame, side, ArmSegmentRole::Upper) {
		return quat_from_basis(rest, [0.0, 1.0, 0.0], target, plane).or_else(|| Some(quat_from_to(rest, target)));
	}
	Some(quat_from_to(rest, target))
}

fn lower_arm_global_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let target = overhead_lower_arm_direction(frame, side, arm_segment_direction(frame, prefix, "elbow", "wrist")?);
	if foldback_lower_arm_pose_is_unreliable(frame, side, target) {
		return upper_arm_global_rotation(frame, side);
	}
	let rest = arm_rest_axis(side);
	let arm_global = if let Some(plane) = arm_plane_secondary(frame, side, ArmSegmentRole::Lower) {
		quat_from_basis(rest, [0.0, 1.0, 0.0], target, plane).unwrap_or_else(|| quat_from_to(rest, target))
	} else {
		quat_from_to(rest, target)
	};
	if let Some(twisted) = lower_arm_palm_twist_rotation(frame, side, target) {
		return Some(twisted);
	}
	Some(arm_global)
}

fn lower_arm_palm_twist_rotation(frame: &UNMotionFrame, side: BodySide, target: [f32; 3]) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let normal = signal_vec3(frame, &format!("hand.{prefix}.palm.normal"))?;
	quat_from_basis(arm_rest_axis(side), [0.0, -1.0, 0.0], target, normal)
}

fn overhead_lower_arm_direction(frame: &UNMotionFrame, side: BodySide, lower: [f32; 3]) -> [f32; 3] {
	let hand_across_signal = format!("hand.{}.palm.across", side.signal_prefix());
	if !hands_are_close_overhead(frame) || signal_vec3(frame, &hand_across_signal).is_none() {
		return lower;
	}
	let depth_target = -side_sign(side) * 0.85;
	normalize3([lower[0] * 0.45, 0.40, depth_target]).unwrap_or(lower)
}

#[cfg(test)]
fn lower_arm_hand_basis_blend(_frame: &UNMotionFrame) -> f32 {
	0.0
}

#[cfg(test)]
fn close_front_prayer_pose(frame: &UNMotionFrame) -> bool {
	let Some(left_wrist) = arm_point(frame, "left", "wrist") else {
		return false;
	};
	let Some(right_wrist) = arm_point(frame, "right", "wrist") else {
		return false;
	};
	let delta = sub_vec3(left_wrist, right_wrist);
	let wrist_distance = ((delta[0] * delta[0]) + (delta[1] * delta[1]) + (delta[2] * delta[2])).sqrt();
	if wrist_distance > 0.16 || !(0.34..=0.58).contains(&left_wrist[1]) || !(0.34..=0.58).contains(&right_wrist[1]) {
		return false;
	}
	let mut has_front_palm = false;
	for side in [BodySide::Left, BodySide::Right] {
		let prefix = side.signal_prefix();
		let Some(upper) = arm_segment_direction(frame, prefix, "shoulder", "elbow") else {
			return false;
		};
		let Some(lower) = arm_segment_direction(frame, prefix, "elbow", "wrist") else {
			return false;
		};
		if upper[1] > -0.45 || lower[1] < 0.50 {
			return false;
		}
		let wrist_toward_center = match side {
			BodySide::Left => lower[0] > 0.25,
			BodySide::Right => lower[0] < -0.25,
		};
		if !wrist_toward_center {
			return false;
		}
		if let (Some(forward), Some(across)) = (
			signal_vec3(frame, &format!("hand.{prefix}.palm.forward")),
			signal_vec3(frame, &format!("hand.{prefix}.palm.across")),
		) {
			has_front_palm |= forward[1] > 0.90 && across[2] < -0.70;
		}
	}
	has_front_palm
}

#[cfg(test)]
fn outward_stop_palms_pose(frame: &UNMotionFrame) -> bool {
	for side in [BodySide::Left, BodySide::Right] {
		let prefix = side.signal_prefix();
		let Some(shoulder) = arm_point(frame, prefix, "shoulder") else {
			return false;
		};
		let Some(wrist) = arm_point(frame, prefix, "wrist") else {
			return false;
		};
		let Some(upper) = arm_segment_direction(frame, prefix, "shoulder", "elbow") else {
			return false;
		};
		let Some(lower) = arm_segment_direction(frame, prefix, "elbow", "wrist") else {
			return false;
		};
		let Some(forward) = signal_vec3(frame, &format!("hand.{prefix}.palm.forward")) else {
			return false;
		};
		let Some(across) = signal_vec3(frame, &format!("hand.{prefix}.palm.across")) else {
			return false;
		};
		let outward = match side {
			BodySide::Left => shoulder[0] - wrist[0],
			BodySide::Right => wrist[0] - shoulder[0],
		};
		let lateral_forearm = lower[0].abs() > 0.60 && lower[1].abs() < 0.30;
		let side_raised_forearm = lower[0].abs() < 0.35 && lower[1] > 0.55;
		if outward < 0.16 || upper[0].abs() < 0.65 || !(lateral_forearm || side_raised_forearm) {
			return false;
		}
		if forward[1] < 0.85 || across[2].abs() < 0.45 {
			return false;
		}
	}
	true
}

fn foldback_lower_arm_pose_is_unreliable(frame: &UNMotionFrame, side: BodySide, lower: [f32; 3]) -> bool {
	if side != BodySide::Left || hand_palm_basis_rotation(frame, side).is_some() {
		return false;
	}
	let prefix = side.signal_prefix();
	let Some(upper) = arm_segment_direction(frame, prefix, "shoulder", "elbow") else {
		return false;
	};
	let Some(shoulder) = arm_point(frame, prefix, "shoulder") else {
		return false;
	};
	let Some(wrist) = arm_point(frame, prefix, "wrist") else {
		return false;
	};
	let wrist_toward_center = match side {
		BodySide::Left => wrist[0] - shoulder[0],
		BodySide::Right => shoulder[0] - wrist[0],
	};
	upper[1] < -0.70 && lower[1] > 0.60 && dot3(upper, lower) < -0.10 && wrist_toward_center > 0.12
}

fn torso_global_rotation(frame: &UNMotionFrame) -> Option<[f32; 4]> {
	let left_shoulder = torso_point(frame, "left", "shoulder")?;
	let right_shoulder = torso_point(frame, "right", "shoulder")?;
	let left_hip = torso_point(frame, "left", "hip")?;
	let right_hip = torso_point(frame, "right", "hip")?;
	let shoulder_mid = midpoint3(left_shoulder, right_shoulder);
	let hip_mid = midpoint3(left_hip, right_hip);
	let up = normalize3(sub_vec3(shoulder_mid, hip_mid))?;
	let right = normalize3(sub_vec3(right_shoulder, left_shoulder))?;
	quat_from_basis([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], up, right)
}

fn lower_body_global_rotation(frame: &UNMotionFrame) -> Option<[f32; 4]> {
	let left_hip = leg_point(frame, "left", "hip")?;
	let right_hip = leg_point(frame, "right", "hip")?;
	let left_knee = leg_point(frame, "left", "knee").or_else(|| leg_point(frame, "left", "ankle"))?;
	let right_knee = leg_point(frame, "right", "knee").or_else(|| leg_point(frame, "right", "ankle"))?;
	let hip_mid = midpoint3(left_hip, right_hip);
	let knee_mid = midpoint3(left_knee, right_knee);
	let down = normalize3(sub_vec3(knee_mid, hip_mid))?;
	let right = normalize3(sub_vec3(right_hip, left_hip))?;
	quat_from_basis([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], down, right)
}

fn upper_leg_global_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let target = leg_segment_direction(frame, prefix, "hip", "knee")?;
	Some(quat_from_to([0.0, -1.0, 0.0], target))
}

fn lower_leg_global_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let target = leg_segment_direction(frame, prefix, "knee", "ankle")?;
	Some(quat_from_to([0.0, -1.0, 0.0], target))
}

fn foot_global_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let target =
		foot_segment_direction(frame, prefix, "ankle", "index").or_else(|| foot_segment_direction(frame, prefix, "heel", "index"))?;
	Some(quat_from_to([0.0, 0.0, 1.0], target))
}

fn toes_global_rotation(frame: &UNMotionFrame, side: BodySide) -> Option<[f32; 4]> {
	let prefix = side.signal_prefix();
	let target =
		foot_segment_direction(frame, prefix, "heel", "index").or_else(|| foot_segment_direction(frame, prefix, "ankle", "index"))?;
	Some(quat_from_to([0.0, 0.0, 1.0], target))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArmSegmentRole {
	Upper,
	Lower,
}

fn arm_plane_secondary(frame: &UNMotionFrame, side: BodySide, role: ArmSegmentRole) -> Option<[f32; 3]> {
	let prefix = side.signal_prefix();
	let upper = arm_segment_direction(frame, prefix, "shoulder", "elbow")?;
	let lower = arm_segment_direction(frame, prefix, "elbow", "wrist")?;
	let mut plane = normalize3(cross3(upper, lower))?;
	if matches!(side, BodySide::Left) {
		if let Some(normal) = signal_vec3(frame, "hand.left.palm.normal") {
			let pointing_front_palm = signal_vec3(frame, "hand.left.palm.forward")
				.map(|forward| is_left_pointing_front_palm(forward, normal, hand_finger_fold(frame, "left")))
				.unwrap_or(false);
			if normal[2] < -0.7 && !pointing_front_palm {
				plane = mix3(plane, [0.0, 0.0, -1.0], 0.25).unwrap_or(plane);
				return Some(scale3(plane, side_sign(side)));
			}
			if normal[2] > 0.7 {
				return Some(scale3(plane, side_sign(side)));
			}
		}
	} else if let (Some(forward), Some(normal)) = (
		signal_vec3(frame, "hand.right.palm.forward"),
		signal_vec3(frame, "hand.right.palm.normal"),
	) {
		if role == ArmSegmentRole::Upper && hands_are_close_overhead(frame) {
			if let Some(across) = signal_vec3(frame, "hand.right.palm.across") {
				return Some(across);
			}
		}
		if is_right_folded_front_palm(forward, normal, hand_finger_fold(frame, "right")) {
			plane = mix3(plane, [0.0, 0.0, 1.0], 0.25).unwrap_or(plane);
		}
	}
	let output_plane = scale3(plane, side_sign(side));
	if role == ArmSegmentRole::Upper
		&& let Some(normal) = signal_vec3(frame, &format!("hand.{prefix}.palm.normal"))
		&& upper_arm_can_share_wrist_twist(upper, lower, output_plane, normal)
	{
		return Some(mix3(output_plane, normal, 0.35).unwrap_or(output_plane));
	}
	Some(output_plane)
}

fn upper_arm_can_share_wrist_twist(upper: [f32; 3], lower: [f32; 3], plane: [f32; 3], normal: [f32; 3]) -> bool {
	if dot3(upper, lower) > 0.65 {
		return false;
	}
	projected_twist_angle_abs(plane, normal, upper).is_some_and(|angle| angle > std::f32::consts::FRAC_PI_4)
}

fn projected_twist_angle_abs(a: [f32; 3], b: [f32; 3], axis: [f32; 3]) -> Option<f32> {
	let axis = normalize3(axis)?;
	let a = project_onto_plane(a, axis)?;
	let b = project_onto_plane(b, axis)?;
	Some(dot3(axis, cross3(a, b)).atan2(dot3(a, b)).abs())
}

fn project_onto_plane(v: [f32; 3], normal: [f32; 3]) -> Option<[f32; 3]> {
	normalize3([
		v[0] - (normal[0] * dot3(v, normal)),
		v[1] - (normal[1] * dot3(v, normal)),
		v[2] - (normal[2] * dot3(v, normal)),
	])
}

fn arm_segment_direction(frame: &UNMotionFrame, prefix: &str, from: &str, to: &str) -> Option<[f32; 3]> {
	let from = arm_point(frame, prefix, from)?;
	let to = arm_point(frame, prefix, to)?;
	normalize3([to[0] - from[0], to[1] - from[1], to[2] - from[2]])
}

fn arm_point(frame: &UNMotionFrame, prefix: &str, joint: &str) -> Option<[f32; 3]> {
	Some([
		scalar_signal(frame, &format!("arm.{prefix}.{joint}.x"))?,
		scalar_signal(frame, &format!("arm.{prefix}.{joint}.y"))?,
		scalar_signal(frame, &format!("arm.{prefix}.{joint}.z")).unwrap_or(0.0),
	])
}

fn torso_point(frame: &UNMotionFrame, prefix: &str, joint: &str) -> Option<[f32; 3]> {
	Some([
		scalar_signal(frame, &format!("torso.{prefix}.{joint}.x"))?,
		scalar_signal(frame, &format!("torso.{prefix}.{joint}.y"))?,
		scalar_signal(frame, &format!("torso.{prefix}.{joint}.z")).unwrap_or(0.0),
	])
}

fn leg_segment_direction(frame: &UNMotionFrame, prefix: &str, from: &str, to: &str) -> Option<[f32; 3]> {
	let from = leg_point(frame, prefix, from)?;
	let to = leg_point(frame, prefix, to)?;
	normalize3(sub_vec3(to, from))
}

fn leg_point(frame: &UNMotionFrame, prefix: &str, joint: &str) -> Option<[f32; 3]> {
	Some([
		scalar_signal(frame, &format!("leg.{prefix}.{joint}.x"))?,
		scalar_signal(frame, &format!("leg.{prefix}.{joint}.y"))?,
		scalar_signal(frame, &format!("leg.{prefix}.{joint}.z")).unwrap_or(0.0),
	])
}

fn foot_segment_direction(frame: &UNMotionFrame, prefix: &str, from: &str, to: &str) -> Option<[f32; 3]> {
	let from = foot_point(frame, prefix, from)?;
	let to = foot_point(frame, prefix, to)?;
	normalize3(sub_vec3(to, from))
}

fn foot_point(frame: &UNMotionFrame, prefix: &str, joint: &str) -> Option<[f32; 3]> {
	Some([
		scalar_signal(frame, &format!("foot.{prefix}.{joint}.x"))?,
		scalar_signal(frame, &format!("foot.{prefix}.{joint}.y"))?,
		scalar_signal(frame, &format!("foot.{prefix}.{joint}.z")).unwrap_or(0.0),
	])
}

fn signal_vec3(frame: &UNMotionFrame, prefix: &str) -> Option<[f32; 3]> {
	Some([
		scalar_signal(frame, &format!("{prefix}.x"))?,
		scalar_signal(frame, &format!("{prefix}.y"))?,
		scalar_signal(frame, &format!("{prefix}.z"))?,
	])
}

fn hands_are_close_overhead(frame: &UNMotionFrame) -> bool {
	let Some(left) = arm_point(frame, "left", "wrist") else {
		return false;
	};
	let Some(right) = arm_point(frame, "right", "wrist") else {
		return false;
	};
	if left[1] < 0.75 || right[1] < 0.75 {
		return false;
	}
	for prefix in ["left", "right"] {
		let Some(upper) = arm_segment_direction(frame, prefix, "shoulder", "elbow") else {
			return false;
		};
		let Some(lower) = arm_segment_direction(frame, prefix, "elbow", "wrist") else {
			return false;
		};
		if upper[1] < 0.70 || lower[1] < 0.70 {
			return false;
		}
	}
	let delta = sub_vec3(left, right);
	((delta[0] * delta[0]) + (delta[1] * delta[1]) + (delta[2] * delta[2])).sqrt() < 0.22
}

fn hand_finger_fold(frame: &UNMotionFrame, prefix: &str) -> f32 {
	let values = ["middle", "ring", "little"]
		.iter()
		.filter_map(|finger| scalar_signal(frame, &format!("hand.{prefix}.{finger}.curl")))
		.collect::<Vec<_>>();
	if values.is_empty() {
		0.0
	} else {
		values.iter().sum::<f32>() / values.len() as f32
	}
}

fn arm_rest_axis(side: BodySide) -> [f32; 3] {
	match side {
		BodySide::Left => [-1.0, 0.0, 0.0],
		BodySide::Right => [1.0, 0.0, 0.0],
	}
}

fn typed_finger_rotation(frame: &UNMotionFrame, bone: &str) -> Option<[f32; 4]> {
	let (side, finger, joint_index) = typed_finger_bone_route(bone)?;
	let hand = match side {
		BodySide::Left => frame.left_hand.as_ref()?,
		BodySide::Right => frame.right_hand.as_ref()?,
	};
	let pose = hand.fingers.iter().find(|pose| pose.finger == finger)?;
	let joint = pose.joints.get(joint_index)?;
	joint.rotation.map(quat_to_array)
}

fn typed_finger_bone_route(bone: &str) -> Option<(BodySide, Finger, usize)> {
	let (side, rest) = if let Some(rest) = bone.strip_prefix("Left") {
		(BodySide::Left, rest)
	} else {
		(BodySide::Right, bone.strip_prefix("Right")?)
	};
	let (finger, segment) = if let Some(segment) = rest.strip_prefix("Thumb") {
		(Finger::Thumb, segment)
	} else if let Some(segment) = rest.strip_prefix("Index") {
		(Finger::Index, segment)
	} else if let Some(segment) = rest.strip_prefix("Middle") {
		(Finger::Middle, segment)
	} else if let Some(segment) = rest.strip_prefix("Ring") {
		(Finger::Ring, segment)
	} else if let Some(segment) = rest.strip_prefix("Little") {
		(Finger::Little, segment)
	} else {
		return None;
	};
	let joint_index = match segment {
		"Proximal" => 0,
		"Intermediate" => 1,
		"Distal" => 2,
		_ => return None,
	};
	Some((side, finger, joint_index))
}

fn finger_rotation(frame: &UNMotionFrame, bone: &str) -> Option<[f32; 4]> {
	let (side, rest) = if let Some(rest) = bone.strip_prefix("Left") {
		(BodySide::Left, rest)
	} else {
		(BodySide::Right, bone.strip_prefix("Right")?)
	};
	let (finger, joint, factor) = finger_signal_and_factor(rest)?;
	let prefix = side.signal_prefix();
	let finger_curl = scalar_signal(frame, &format!("hand.{prefix}.{finger}.curl")).unwrap_or(0.0);
	let sibling_fold = hand_finger_fold(frame, prefix);
	let curl = scalar_signal(frame, &format!("hand.{prefix}.{finger}.{joint}.curl"))
		.or_else(|| scalar_signal(frame, &format!("hand.{prefix}.{finger}.curl")))
		.unwrap_or(0.0);
	let spread = scalar_signal(frame, &format!("hand.{prefix}.{finger}.spread")).unwrap_or(0.0);
	let factor = adjusted_finger_factor(finger, rest, side, finger_curl, sibling_fold, curl, factor);
	Some(finger_curl_to_quat(curl, spread, factor, rest, side, finger_curl))
}

fn finger_signal_and_factor(rest: &str) -> Option<(&'static str, &'static str, f32)> {
	let (finger, segment) = if let Some(segment) = rest.strip_prefix("Thumb") {
		("thumb", segment)
	} else if let Some(segment) = rest.strip_prefix("Index") {
		("index", segment)
	} else if let Some(segment) = rest.strip_prefix("Middle") {
		("middle", segment)
	} else if let Some(segment) = rest.strip_prefix("Ring") {
		("ring", segment)
	} else if let Some(segment) = rest.strip_prefix("Little") {
		("little", segment)
	} else {
		return None;
	};
	let (joint, factor) = match segment {
		"Proximal" if finger == "thumb" => ("mcp", 0.35),
		"Intermediate" if finger == "thumb" => ("pip", 0.25),
		"Distal" if finger == "thumb" => ("dip", 0.25),
		"Proximal" if finger == "index" => ("mcp", 0.75),
		"Intermediate" if finger == "index" => ("pip", 1.05),
		"Distal" if finger == "index" => ("dip", 0.35),
		"Proximal" => ("mcp", 1.35),
		"Intermediate" => ("pip", 1.25),
		"Distal" => ("dip", 0.9),
		_ => return None,
	};
	Some((finger, joint, factor))
}

fn adjusted_finger_factor(
	finger: &str,
	rest: &str,
	side: BodySide,
	finger_curl: f32,
	sibling_fold: f32,
	joint_curl: f32,
	fallback: f32,
) -> f32 {
	if finger != "index" {
		return fallback;
	}

	let thumb_grip = sibling_fold > 0.30 && sibling_fold < 0.55 && (finger_curl > 0.25 || joint_curl > 0.80);
	if thumb_grip {
		match rest {
			"IndexProximal" => 1.25,
			"IndexIntermediate" => 1.85,
			"IndexDistal" if side == BodySide::Right => 1.6,
			"IndexDistal" => 4.6,
			_ => fallback,
		}
	} else if finger_curl > 0.60 && sibling_fold > 0.55 {
		match rest {
			"IndexProximal" => 1.4,
			"IndexIntermediate" => 1.8,
			"IndexDistal" if side == BodySide::Right => 1.0,
			"IndexDistal" => 1.45,
			_ => fallback,
		}
	} else {
		fallback
	}
}

fn side_sign(side: BodySide) -> f32 {
	match side {
		BodySide::Left => 1.0,
		BodySide::Right => -1.0,
	}
}

const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

// Rest offsets captured from Warudo VMC output. VMC receivers such as VSeeFace
// expect local humanoid bone poses, so positions are stable avatar-local offsets
// while motion is carried by quaternions.
const HUMANOID_BONE_REST_POSES: &[BoneRestPose] = &[
	BoneRestPose {
		name: "Hips",
		position: [0.0, 0.9022, 0.0400],
	},
	BoneRestPose {
		name: "LeftUpperLeg",
		position: [-0.0627, -0.0996, -0.0204],
	},
	BoneRestPose {
		name: "RightUpperLeg",
		position: [0.0627, -0.0996, -0.0204],
	},
	BoneRestPose {
		name: "LeftLowerLeg",
		position: [0.0, -0.3356, -0.0113],
	},
	BoneRestPose {
		name: "RightLowerLeg",
		position: [0.0, -0.3356, -0.0113],
	},
	BoneRestPose {
		name: "LeftFoot",
		position: [-0.0017, -0.3443, -0.0225],
	},
	BoneRestPose {
		name: "RightFoot",
		position: [0.0017, -0.3443, -0.0225],
	},
	BoneRestPose {
		name: "Spine",
		position: [0.0, 0.0350, 0.0],
	},
	BoneRestPose {
		name: "Chest",
		position: [0.0, 0.0785, -0.0001],
	},
	BoneRestPose {
		name: "Neck",
		position: [0.0, 0.1789, -0.0365],
	},
	BoneRestPose {
		name: "Head",
		position: [0.0, 0.0505, 0.0041],
	},
	BoneRestPose {
		name: "LeftShoulder",
		position: [-0.0389, 0.1566, -0.0346],
	},
	BoneRestPose {
		name: "RightShoulder",
		position: [0.0389, 0.1566, -0.0346],
	},
	BoneRestPose {
		name: "LeftUpperArm",
		position: [-0.0442, 0.0011, -0.0003],
	},
	BoneRestPose {
		name: "RightUpperArm",
		position: [0.0442, 0.0011, -0.0003],
	},
	BoneRestPose {
		name: "LeftLowerArm",
		position: [-0.2234, 0.0008, -0.0062],
	},
	BoneRestPose {
		name: "RightLowerArm",
		position: [0.2234, 0.0008, -0.0062],
	},
	BoneRestPose {
		name: "LeftHand",
		position: [-0.1832, -0.0001, 0.0048],
	},
	BoneRestPose {
		name: "RightHand",
		position: [0.1832, -0.0001, 0.0048],
	},
	BoneRestPose {
		name: "LeftToes",
		position: [-0.0011, -0.0717, 0.0971],
	},
	BoneRestPose {
		name: "RightToes",
		position: [0.0011, -0.0717, 0.0971],
	},
	BoneRestPose {
		name: "LeftEye",
		position: [-0.0135, 0.0569, 0.0213],
	},
	BoneRestPose {
		name: "RightEye",
		position: [0.0135, 0.0569, 0.0213],
	},
	BoneRestPose {
		name: "Jaw",
		position: [0.0, 0.0, 0.0],
	},
	BoneRestPose {
		name: "LeftThumbProximal",
		position: [-0.0211, -0.0043, 0.0142],
	},
	BoneRestPose {
		name: "LeftThumbIntermediate",
		position: [-0.0201, -0.0070, 0.0202],
	},
	BoneRestPose {
		name: "LeftThumbDistal",
		position: [-0.0217, -0.0035, 0.0086],
	},
	BoneRestPose {
		name: "LeftIndexProximal",
		position: [-0.0676, -0.0015, 0.0203],
	},
	BoneRestPose {
		name: "LeftIndexIntermediate",
		position: [-0.0233, -0.0001, -0.0012],
	},
	BoneRestPose {
		name: "LeftIndexDistal",
		position: [-0.0178, -0.0001, -0.0010],
	},
	BoneRestPose {
		name: "LeftMiddleProximal",
		position: [-0.0700, 0.0013, 0.0061],
	},
	BoneRestPose {
		name: "LeftMiddleIntermediate",
		position: [-0.0257, -0.0001, -0.0001],
	},
	BoneRestPose {
		name: "LeftMiddleDistal",
		position: [-0.0206, -0.0001, -0.0001],
	},
	BoneRestPose {
		name: "LeftRingProximal",
		position: [-0.0675, -0.0016, -0.0080],
	},
	BoneRestPose {
		name: "LeftRingIntermediate",
		position: [-0.0231, -0.0001, 0.0012],
	},
	BoneRestPose {
		name: "LeftRingDistal",
		position: [-0.0186, -0.0001, 0.0011],
	},
	BoneRestPose {
		name: "LeftLittleProximal",
		position: [-0.0642, -0.0053, -0.0182],
	},
	BoneRestPose {
		name: "LeftLittleIntermediate",
		position: [-0.0154, 0.0001, -0.0001],
	},
	BoneRestPose {
		name: "LeftLittleDistal",
		position: [-0.0124, 0.0, 0.0014],
	},
	BoneRestPose {
		name: "RightThumbProximal",
		position: [0.0211, -0.0044, 0.0152],
	},
	BoneRestPose {
		name: "RightThumbIntermediate",
		position: [0.0201, -0.0072, 0.0209],
	},
	BoneRestPose {
		name: "RightThumbDistal",
		position: [0.0218, -0.0036, 0.0089],
	},
	BoneRestPose {
		name: "RightIndexProximal",
		position: [0.0676, -0.0015, 0.0203],
	},
	BoneRestPose {
		name: "RightIndexIntermediate",
		position: [0.0233, -0.0002, 0.0],
	},
	BoneRestPose {
		name: "RightIndexDistal",
		position: [0.0178, -0.0001, -0.0010],
	},
	BoneRestPose {
		name: "RightMiddleProximal",
		position: [0.0700, 0.0013, 0.0070],
	},
	BoneRestPose {
		name: "RightMiddleIntermediate",
		position: [0.0257, -0.0001, -0.0001],
	},
	BoneRestPose {
		name: "RightMiddleDistal",
		position: [0.0206, -0.0001, 0.0],
	},
	BoneRestPose {
		name: "RightRingProximal",
		position: [0.0675, -0.0016, -0.0080],
	},
	BoneRestPose {
		name: "RightRingIntermediate",
		position: [0.0231, -0.0001, 0.0017],
	},
	BoneRestPose {
		name: "RightRingDistal",
		position: [0.0186, -0.0001, 0.0011],
	},
	BoneRestPose {
		name: "RightLittleProximal",
		position: [0.0642, -0.0053, -0.0182],
	},
	BoneRestPose {
		name: "RightLittleIntermediate",
		position: [0.0154, -0.0001, 0.0],
	},
	BoneRestPose {
		name: "RightLittleDistal",
		position: [0.0124, 0.0, 0.0015],
	},
	BoneRestPose {
		name: "UpperChest",
		position: [0.0, 0.0, 0.0],
	},
];

fn scalar_signal(frame: &UNMotionFrame, name: &str) -> Option<f32> {
	frame.signals.iter().find_map(|signal| {
		if signal.name == name {
			if let MotionSignalValue::Scalar(value) = signal.value {
				return Some(value.clamp(-1.0, 1.0));
			}
		}
		None
	})
}

fn frame_has_vmc_bone_signals(frame: &UNMotionFrame) -> bool {
	frame.signals.iter().any(|signal| {
		signal.name.starts_with("head.")
			|| signal.name.starts_with("arm.")
			|| signal.name.starts_with("hand.")
			|| signal.name.starts_with("torso.")
			|| signal.name.starts_with("leg.")
			|| signal.name.starts_with("foot.")
	})
}

fn frame_has_hand_motion(frame: &UNMotionFrame) -> bool {
	frame
		.left_hand
		.as_ref()
		.is_some_and(|hand| hand.wrist.is_some() || !hand.fingers.is_empty())
		|| frame
			.right_hand
			.as_ref()
			.is_some_and(|hand| hand.wrist.is_some() || !hand.fingers.is_empty())
}

fn frame_has_direct_humanoid_pose(frame: &UNMotionFrame) -> bool {
	frame
		.body
		.as_ref()
		.and_then(|body| body.humanoid.as_ref())
		.map(|humanoid| humanoid.root.is_some() || !humanoid.bones.is_empty())
		.unwrap_or(false)
}

fn euler_to_quat(pitch_norm: f32, yaw_norm: f32, roll_norm: f32) -> [f32; 4] {
	euler_radians_to_quat(pitch_norm * 0.65, yaw_norm * 0.85, roll_norm * 0.55)
}

fn euler_radians_to_quat(pitch: f32, yaw: f32, roll: f32) -> [f32; 4] {
	let (sx, cx) = ((pitch * 0.5).sin(), (pitch * 0.5).cos());
	let (sy, cy) = ((yaw * 0.5).sin(), (yaw * 0.5).cos());
	let (sz, cz) = ((roll * 0.5).sin(), (roll * 0.5).cos());

	let qw = cx * cy * cz + sx * sy * sz;
	let qx = sx * cy * cz - cx * sy * sz;
	let qy = cx * sy * cz + sx * cy * sz;
	let qz = cx * cy * sz - sx * sy * cz;
	[qx, qy, qz, qw]
}

fn quat_from_to(from: [f32; 3], to: [f32; 3]) -> [f32; 4] {
	let Some(from) = normalize3(from) else {
		return IDENTITY_QUAT;
	};
	let Some(to) = normalize3(to) else {
		return IDENTITY_QUAT;
	};
	let dot = dot3(from, to).clamp(-1.0, 1.0);
	if dot > 0.9995 {
		return IDENTITY_QUAT;
	}
	if dot < -0.9995 {
		let axis = if from[0].abs() < 0.9 {
			normalize3(cross3(from, [1.0, 0.0, 0.0])).unwrap_or([0.0, 1.0, 0.0])
		} else {
			normalize3(cross3(from, [0.0, 1.0, 0.0])).unwrap_or([0.0, 0.0, 1.0])
		};
		return [axis[0], axis[1], axis[2], 0.0];
	}
	let axis = cross3(from, to);
	normalize_quat([axis[0], axis[1], axis[2], 1.0 + dot])
}

fn quat_from_basis(from_primary: [f32; 3], from_secondary: [f32; 3], to_primary: [f32; 3], to_secondary: [f32; 3]) -> Option<[f32; 4]> {
	let from = orthonormal_basis(from_primary, from_secondary)?;
	let to = orthonormal_basis(to_primary, to_secondary)?;
	let matrix = [
		[
			(to[0][0] * from[0][0]) + (to[1][0] * from[1][0]) + (to[2][0] * from[2][0]),
			(to[0][0] * from[0][1]) + (to[1][0] * from[1][1]) + (to[2][0] * from[2][1]),
			(to[0][0] * from[0][2]) + (to[1][0] * from[1][2]) + (to[2][0] * from[2][2]),
		],
		[
			(to[0][1] * from[0][0]) + (to[1][1] * from[1][0]) + (to[2][1] * from[2][0]),
			(to[0][1] * from[0][1]) + (to[1][1] * from[1][1]) + (to[2][1] * from[2][1]),
			(to[0][1] * from[0][2]) + (to[1][1] * from[1][2]) + (to[2][1] * from[2][2]),
		],
		[
			(to[0][2] * from[0][0]) + (to[1][2] * from[1][0]) + (to[2][2] * from[2][0]),
			(to[0][2] * from[0][1]) + (to[1][2] * from[1][1]) + (to[2][2] * from[2][1]),
			(to[0][2] * from[0][2]) + (to[1][2] * from[1][2]) + (to[2][2] * from[2][2]),
		],
	];
	Some(quat_from_rotation_matrix(matrix))
}

fn orthonormal_basis(primary: [f32; 3], secondary: [f32; 3]) -> Option<[[f32; 3]; 3]> {
	let x = normalize3(primary)?;
	let projected = dot3(secondary, x);
	let y = normalize3([
		secondary[0] - (x[0] * projected),
		secondary[1] - (x[1] * projected),
		secondary[2] - (x[2] * projected),
	])?;
	let z = normalize3(cross3(x, y))?;
	Some([x, y, z])
}

fn quat_from_rotation_matrix(matrix: [[f32; 3]; 3]) -> [f32; 4] {
	let trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
	if trace > 0.0 {
		let scale = (trace + 1.0).sqrt() * 2.0;
		return normalize_quat([
			(matrix[2][1] - matrix[1][2]) / scale,
			(matrix[0][2] - matrix[2][0]) / scale,
			(matrix[1][0] - matrix[0][1]) / scale,
			0.25 * scale,
		]);
	}
	if matrix[0][0] > matrix[1][1] && matrix[0][0] > matrix[2][2] {
		let scale = (1.0 + matrix[0][0] - matrix[1][1] - matrix[2][2]).sqrt() * 2.0;
		return normalize_quat([
			0.25 * scale,
			(matrix[0][1] + matrix[1][0]) / scale,
			(matrix[0][2] + matrix[2][0]) / scale,
			(matrix[2][1] - matrix[1][2]) / scale,
		]);
	}
	if matrix[1][1] > matrix[2][2] {
		let scale = (1.0 + matrix[1][1] - matrix[0][0] - matrix[2][2]).sqrt() * 2.0;
		return normalize_quat([
			(matrix[0][1] + matrix[1][0]) / scale,
			0.25 * scale,
			(matrix[1][2] + matrix[2][1]) / scale,
			(matrix[0][2] - matrix[2][0]) / scale,
		]);
	}
	let scale = (1.0 + matrix[2][2] - matrix[0][0] - matrix[1][1]).sqrt() * 2.0;
	normalize_quat([
		(matrix[0][2] + matrix[2][0]) / scale,
		(matrix[1][2] + matrix[2][1]) / scale,
		0.25 * scale,
		(matrix[1][0] - matrix[0][1]) / scale,
	])
}

fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
	let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
	if len <= 1e-6 {
		IDENTITY_QUAT
	} else {
		[q[0] / len, q[1] / len, q[2] / len, q[3] / len]
	}
}

fn dampen_rotation(rotation: [f32; 4], strength: f32) -> [f32; 4] {
	let t = 1.0 - strength.clamp(0.0, 1.0);
	normalize_quat([rotation[0] * t, rotation[1] * t, rotation[2] * t, 1.0 + ((rotation[3] - 1.0) * t)])
}

fn quat_inverse(q: [f32; 4]) -> [f32; 4] {
	let normalized = normalize_quat(q);
	[-normalized[0], -normalized[1], -normalized[2], normalized[3]]
}

fn mirror_horizontal_quat(q: [f32; 4]) -> [f32; 4] {
	let q = normalize_quat(q);
	normalize_quat([q[0], -q[1], -q[2], q[3]])
}

fn quat_mul(left: [f32; 4], right: [f32; 4]) -> [f32; 4] {
	normalize_quat([
		(left[3] * right[0]) + (left[0] * right[3]) + (left[1] * right[2]) - (left[2] * right[1]),
		(left[3] * right[1]) - (left[0] * right[2]) + (left[1] * right[3]) + (left[2] * right[0]),
		(left[3] * right[2]) + (left[0] * right[1]) - (left[1] * right[0]) + (left[2] * right[3]),
		(left[3] * right[3]) - (left[0] * right[0]) - (left[1] * right[1]) - (left[2] * right[2]),
	])
}

fn normalize3(v: [f32; 3]) -> Option<[f32; 3]> {
	let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
	if len <= 1e-6 {
		None
	} else {
		Some([v[0] / len, v[1] / len, v[2] / len])
	}
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
	(a[0] * b[0]) + (a[1] * b[1]) + (a[2] * b[2])
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
	[
		(a[1] * b[2]) - (a[2] * b[1]),
		(a[2] * b[0]) - (a[0] * b[2]),
		(a[0] * b[1]) - (a[1] * b[0]),
	]
}

fn scale3(vector: [f32; 3], scale: f32) -> [f32; 3] {
	[vector[0] * scale, vector[1] * scale, vector[2] * scale]
}

fn sub_vec3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
	[left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn midpoint3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
	[(left[0] + right[0]) * 0.5, (left[1] + right[1]) * 0.5, (left[2] + right[2]) * 0.5]
}

fn mix3(left: [f32; 3], right: [f32; 3], amount: f32) -> Option<[f32; 3]> {
	normalize3([
		(left[0] * (1.0 - amount)) + (right[0] * amount),
		(left[1] * (1.0 - amount)) + (right[1] * amount),
		(left[2] * (1.0 - amount)) + (right[2] * amount),
	])
}

fn finger_curl_to_quat(curl: f32, spread: f32, factor: f32, rest: &str, side: BodySide, finger_curl: f32) -> [f32; 4] {
	if rest == "ThumbProximal" && is_thumb_opposition_case(curl, spread, side, finger_curl) {
		return thumb_proximal_to_quat(curl, spread, side);
	}
	let curl_angle = curl.clamp(0.0, 1.0) * factor * side_sign(side);
	let spread_angle = if rest.ends_with("Proximal") {
		finger_spread_angle(spread, rest, side)
	} else {
		0.0
	};
	euler_radians_to_quat(0.0, spread_angle, curl_angle)
}

fn is_thumb_opposition_case(curl: f32, spread: f32, side: BodySide, finger_curl: f32) -> bool {
	curl > 0.25
		&& match side {
			BodySide::Left => spread > 0.35,
			BodySide::Right => spread < -0.35 && finger_curl < 0.60,
		}
}

fn thumb_proximal_to_quat(curl: f32, spread: f32, side: BodySide) -> [f32; 4] {
	let opposition_angle = curl.clamp(0.0, 1.0) * 0.65;
	let abduction_angle = -spread.clamp(-1.0, 1.0) * 0.60;
	let curl_angle = curl.clamp(0.0, 1.0) * 0.12 * side_sign(side);
	euler_radians_to_quat(opposition_angle, abduction_angle, curl_angle)
}

fn finger_spread_angle(spread: f32, rest: &str, side: BodySide) -> f32 {
	let spread = canonical_finger_spread(spread, side);
	if rest.starts_with("Thumb") {
		return spread * 0.10;
	}
	let factor = if rest.starts_with("Index") {
		0.85
	} else if rest.starts_with("Little") {
		1.15
	} else if rest.starts_with("Ring") {
		0.52
	} else {
		0.0
	};
	spread * factor
}

fn canonical_finger_spread(spread: f32, side: BodySide) -> f32 {
	let spread = spread.clamp(-1.0, 1.0);
	match side {
		BodySide::Right => -spread,
		BodySide::Left => spread,
	}
}

fn default_blendshape_map() -> HashMap<String, BlendshapeRoute> {
	let mut map = HashMap::new();
	map.insert("head.yaw".to_string(), BlendshapeRoute::simple("HeadYaw"));
	map.insert(
		"head.pitch".to_string(),
		BlendshapeRoute {
			blendshape_name: "HeadPitch".to_string(),
			scale: -1.0,
			offset: 0.0,
			clamp_min: Some(-1.0),
			clamp_max: Some(1.0),
		},
	);
	map.insert("head.roll".to_string(), BlendshapeRoute::simple("HeadRoll"));
	map.insert("eye.left.yaw".to_string(), BlendshapeRoute::simple("EyeLeftYaw"));
	map.insert("eye.right.yaw".to_string(), BlendshapeRoute::simple("EyeRightYaw"));
	map.insert("eye.left.pitch".to_string(), BlendshapeRoute::simple("EyeLeftPitch"));
	map.insert("eye.right.pitch".to_string(), BlendshapeRoute::simple("EyeRightPitch"));
	for name in PERFECT_SYNC_BLENDSHAPES {
		map.insert(format!("face.{name}"), BlendshapeRoute::simple(*name));
	}
	map
}

const PERFECT_SYNC_BLENDSHAPES: &[&str] = &[
	"browDownLeft",
	"browDownRight",
	"browInnerUp",
	"browOuterUpLeft",
	"browOuterUpRight",
	"cheekPuff",
	"cheekSquintLeft",
	"cheekSquintRight",
	"eyeBlinkLeft",
	"eyeBlinkRight",
	"eyeLookDownLeft",
	"eyeLookDownRight",
	"eyeLookInLeft",
	"eyeLookInRight",
	"eyeLookOutLeft",
	"eyeLookOutRight",
	"eyeLookUpLeft",
	"eyeLookUpRight",
	"eyeSquintLeft",
	"eyeSquintRight",
	"eyeWideLeft",
	"eyeWideRight",
	"jawForward",
	"jawLeft",
	"jawOpen",
	"jawRight",
	"mouthClose",
	"mouthDimpleLeft",
	"mouthDimpleRight",
	"mouthFrownLeft",
	"mouthFrownRight",
	"mouthFunnel",
	"mouthLeft",
	"mouthLowerDownLeft",
	"mouthLowerDownRight",
	"mouthPressLeft",
	"mouthPressRight",
	"mouthPucker",
	"mouthRight",
	"mouthRollLower",
	"mouthRollUpper",
	"mouthShrugLower",
	"mouthShrugUpper",
	"mouthSmileLeft",
	"mouthSmileRight",
	"mouthStretchLeft",
	"mouthStretchRight",
	"mouthUpperUpLeft",
	"mouthUpperUpRight",
	"noseSneerLeft",
	"noseSneerRight",
];

#[cfg(test)]
mod tests {
	use super::*;
	use rosc::decoder;
	use un_motion_frame::{
		BodyMotion, BoneSample, ExpressionSample, FaceMotion, Finger, FingerPose, HandMotion, HumanoidBone, HumanoidPose, MotionSignal,
		MotionSignalValue, Quatf, SampleState, TrackingState, TransformSample,
	};

	fn recv_messages(receiver: &UdpSocket, buf: &mut [u8]) -> Vec<OscMessage> {
		let (len, _) = receiver.recv_from(buf).expect("packet");
		let (_, packet) = decoder::decode_udp(&buf[..len]).expect("decode");
		flatten_messages(packet)
	}

	fn flatten_messages(packet: OscPacket) -> Vec<OscMessage> {
		match packet {
			OscPacket::Message(msg) => vec![msg],
			OscPacket::Bundle(bundle) => bundle.content.into_iter().flat_map(flatten_messages).collect(),
		}
	}

	fn message_names(messages: &[OscMessage]) -> Vec<String> {
		messages
			.iter()
			.filter(|message| message.addr == "/VMC/Ext/Bone/Pos")
			.filter_map(|message| match message.args.first() {
				Some(OscType::String(name)) => Some(name.clone()),
				_ => None,
			})
			.collect()
	}

	fn find_message<'a>(messages: &'a [OscMessage], addr: &str, name: &str) -> &'a OscMessage {
		messages
			.iter()
			.find(|message| message.addr == addr && message.args.first() == Some(&OscType::String(name.to_string())))
			.unwrap_or_else(|| panic!("missing {addr} {name}"))
	}

	fn scalar(name: &str, value: f32) -> MotionSignal {
		MotionSignal {
			name: name.to_string(),
			value: MotionSignalValue::Scalar(value),
			confidence: 1.0,
			source_index: Some(0),
			state: SampleState::Valid,
		}
	}

	fn transform(rotation: [f32; 4]) -> TransformSample {
		TransformSample {
			translation: None,
			rotation: Some(Quatf {
				x: rotation[0],
				y: rotation[1],
				z: rotation[2],
				w: rotation[3],
			}),
			scale: None,
			linear_velocity: None,
			angular_velocity: None,
		}
	}

	fn quat_angle_rad(left: [f32; 4], right: [f32; 4]) -> f32 {
		let left = normalize_quat(left);
		let right = normalize_quat(right);
		let dot = ((left[0] * right[0]) + (left[1] * right[1]) + (left[2] * right[2]) + (left[3] * right[3]))
			.abs()
			.clamp(-1.0, 1.0);
		2.0 * dot.acos()
	}

	fn quat_rotate_vec3(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
		let q = normalize_quat(q);
		let u = [q[0], q[1], q[2]];
		let uv = cross3(u, v);
		let uuv = cross3(u, uv);
		[
			v[0] + (2.0 * ((q[3] * uv[0]) + uuv[0])),
			v[1] + (2.0 * ((q[3] * uv[1]) + uuv[1])),
			v[2] + (2.0 * ((q[3] * uv[2]) + uuv[2])),
		]
	}

	fn assert_vec3_near(actual: [f32; 3], expected: [f32; 3], epsilon: f32) {
		for (actual, expected) in actual.into_iter().zip(expected) {
			assert!((actual - expected).abs() <= epsilon, "actual={actual:?} expected={expected:?}");
		}
	}

	#[test]
	fn sink_passes_direct_humanoid_pose_and_face_expressions() {
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![BoneSample {
					bone: HumanoidBone::Head,
					transform: transform([0.0, 0.25, 0.0, 0.9682458]),
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				}],
			}),
		});
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "Joy".to_string(),
				value: 0.75,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});

		let messages = flatten_messages(OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content: vmc_packets_for_frame_without_ok(&frame),
		}));

		assert!(message_names(&messages).contains(&"Head".to_string()));
		let blend = find_message(&messages, "/VMC/Ext/Blend/Val", "Joy");
		assert_eq!(blend.args.get(1), Some(&OscType::Float(0.75)));
		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/Blend/Apply"));
	}

	#[test]
	fn sink_emits_direct_humanoid_root_as_root_pos_packet() {
		let mut frame = UNMotionFrame::new(11);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: Some(TransformSample {
					translation: Some(Vec3f { x: 0.25, y: 1.0, z: -0.5 }),
					rotation: Some(Quatf {
						x: 0.1,
						y: 0.2,
						z: 0.3,
						w: 0.9,
					}),
					scale: None,
					linear_velocity: None,
					angular_velocity: None,
				}),
				bones: vec![BoneSample {
					bone: HumanoidBone::Head,
					transform: transform([0.0, 0.25, 0.0, 0.9682458]),
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				}],
			}),
		});

		let messages = flatten_messages(OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content: vmc_packets_for_frame_without_ok(&frame),
		}));

		let root = find_message(&messages, "/VMC/Ext/Root/Pos", "root");
		assert_eq!(root.args[1], OscType::Float(0.25));
		assert_eq!(root.args[2], OscType::Float(1.0));
		assert_eq!(root.args[3], OscType::Float(-0.5));
		assert!(
			!messages
				.iter()
				.any(|message| message.addr == "/VMC/Ext/Bone/Pos" && message.args.first() == Some(&OscType::String("root".to_string())))
		);
	}

	#[test]
	fn sink_sends_ok_and_mapped_blendshape_packets() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create");
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: Some(TransformSample {
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
				}),
				bones: vec![BoneSample {
					bone: HumanoidBone::Head,
					transform: transform([0.0, 0.25, 0.0, 0.9682458]),
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				}],
			}),
		});
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![
				ExpressionSample {
					name: "HeadYaw".to_string(),
					value: 0.25,
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				},
				ExpressionSample {
					name: "HeadPitch".to_string(),
					value: -0.1,
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				},
			],
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/OK"));

		let root = messages.iter().find(|message| message.addr == "/VMC/Ext/Root/Pos").expect("root");
		assert_eq!(root.addr, "/VMC/Ext/Root/Pos");
		assert_eq!(root.args[0], OscType::String("root".to_string()));

		let bone = find_message(&messages, "/VMC/Ext/Bone/Pos", "Head");
		assert_eq!(bone.addr, "/VMC/Ext/Bone/Pos");
		assert_eq!(bone.args[0], OscType::String("Head".to_string()));
		assert_eq!(message_names(&messages), vec!["Head".to_string()]);

		let blend1 = find_message(&messages, "/VMC/Ext/Blend/Val", "HeadYaw");
		assert_eq!(blend1.addr, "/VMC/Ext/Blend/Val");
		assert_eq!(blend1.args[0], OscType::String("HeadYaw".to_string()));

		let blend2 = find_message(&messages, "/VMC/Ext/Blend/Val", "HeadPitch");
		assert_eq!(blend2.addr, "/VMC/Ext/Blend/Val");
		assert_eq!(blend2.args[0], OscType::String("HeadPitch".to_string()));

		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/Blend/Apply"));
	}

	#[test]
	fn sink_uses_custom_blendshape_mapping() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut map = HashMap::new();
		map.insert("head.pitch".to_string(), BlendshapeRoute::simple("HeadPitch"));
		let mut sink = VmcOutputSink::new(target)
			.expect("sink create")
			.with_ok_packet(false)
			.with_blendshape_map(map);

		let mut frame = UNMotionFrame::new(2);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "head.pitch".to_string(),
				value: -0.25,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		let msg = find_message(&messages, "/VMC/Ext/Blend/Val", "HeadPitch");
		assert_eq!(msg.addr, "/VMC/Ext/Blend/Val");
		assert_eq!(msg.args[0], OscType::String("HeadPitch".to_string()));
		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/Blend/Apply"));
	}

	#[test]
	fn sink_does_not_emit_zero_head_pose_without_head_signals() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(100)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create").with_ok_packet(false);
		let frame = UNMotionFrame::new(4);

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		assert!(receiver.recv_from(&mut buf).is_err());
	}

	#[test]
	fn sink_does_not_project_signal_only_body_into_vmc_bones() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(100)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create").with_ok_packet(false);
		let mut frame = UNMotionFrame::new(12);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.2),
			scalar("arm.left.shoulder.y", 0.4),
			scalar("arm.left.elbow.x", -0.4),
			scalar("arm.left.elbow.y", 0.5),
			scalar("arm.left.wrist.x", -0.6),
			scalar("arm.left.wrist.y", 0.5),
		]);

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		assert!(
			receiver.recv_from(&mut buf).is_err(),
			"VMC UDP output must consume typed UNMotionFrame fields, not signal-only body geometry"
		);
	}

	#[test]
	fn sink_maps_eye_expressions_to_default_blendshapes() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create").with_ok_packet(false);
		let mut frame = UNMotionFrame::new(5);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "EyeLeftYaw".to_string(),
				value: 0.35,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		let msg = find_message(&messages, "/VMC/Ext/Blend/Val", "EyeLeftYaw");
		assert_eq!(msg.addr, "/VMC/Ext/Blend/Val");
		assert_eq!(msg.args[0], OscType::String("EyeLeftYaw".to_string()));
		assert_eq!(msg.args[1], OscType::Float(0.35));
		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/Blend/Apply"));
	}

	#[test]
	fn sink_maps_perfect_sync_face_expressions_to_default_blendshapes() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create").with_ok_packet(false);
		let mut frame = UNMotionFrame::new(8);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![
				ExpressionSample {
					name: "eyeBlinkLeft".to_string(),
					value: 0.75,
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				},
				ExpressionSample {
					name: "jawOpen".to_string(),
					value: 0.5,
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				},
			],
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		let blink = find_message(&messages, "/VMC/Ext/Blend/Val", "eyeBlinkLeft");
		assert_eq!(blink.addr, "/VMC/Ext/Blend/Val");
		assert_eq!(blink.args[0], OscType::String("eyeBlinkLeft".to_string()));
		assert_eq!(blink.args[1], OscType::Float(0.75));
		let jaw = find_message(&messages, "/VMC/Ext/Blend/Val", "jawOpen");
		assert_eq!(jaw.addr, "/VMC/Ext/Blend/Val");
		assert_eq!(jaw.args[0], OscType::String("jawOpen".to_string()));
		assert_eq!(jaw.args[1], OscType::Float(0.5));
		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/Blend/Apply"));
	}

	#[test]
	fn sink_applies_scale_offset_and_clamp() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut map = HashMap::new();
		map.insert(
			"head.yaw".to_string(),
			BlendshapeRoute {
				blendshape_name: "HeadYaw".to_string(),
				scale: 2.0,
				offset: 0.1,
				clamp_min: Some(-0.2),
				clamp_max: Some(0.2),
			},
		);
		let mut sink = VmcOutputSink::new(target)
			.expect("sink create")
			.with_ok_packet(false)
			.with_blendshape_map(map);

		let mut frame = UNMotionFrame::new(3);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "head.yaw".to_string(),
				value: 0.2,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		let msg = find_message(&messages, "/VMC/Ext/Blend/Val", "HeadYaw");
		assert_eq!(msg.addr, "/VMC/Ext/Blend/Val");
		assert_eq!(msg.args[0], OscType::String("HeadYaw".to_string()));
		assert_eq!(msg.args[1], OscType::Float(0.2));
	}

	#[test]
	fn sink_maps_direct_humanoid_arm_and_hand_to_vmc_bones() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create").with_ok_packet(false);
		let mut frame = UNMotionFrame::new(6);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: [
					HumanoidBone::LeftShoulder,
					HumanoidBone::LeftUpperArm,
					HumanoidBone::LeftLowerArm,
					HumanoidBone::LeftHand,
				]
				.into_iter()
				.map(|bone| BoneSample {
					bone,
					transform: transform([0.0, 0.0, 0.25, 0.9682458]),
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				})
				.collect(),
			}),
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/Root/Pos"));
		let names = message_names(&messages);

		assert!(names.contains(&"LeftShoulder".to_string()));
		assert!(names.contains(&"LeftUpperArm".to_string()));
		assert!(names.contains(&"LeftLowerArm".to_string()));
		assert!(names.contains(&"LeftHand".to_string()));
	}

	#[test]
	fn torso_signals_rotate_chest_bones() {
		let mut frame = UNMotionFrame::new(16);
		frame.signals.extend([
			scalar("torso.left.hip.x", -0.25),
			scalar("torso.left.hip.y", 0.0),
			scalar("torso.left.hip.z", 0.0),
			scalar("torso.right.hip.x", 0.25),
			scalar("torso.right.hip.y", 0.0),
			scalar("torso.right.hip.z", 0.0),
			scalar("torso.left.shoulder.x", -0.10),
			scalar("torso.left.shoulder.y", 0.7),
			scalar("torso.left.shoulder.z", 0.20),
			scalar("torso.right.shoulder.x", 0.40),
			scalar("torso.right.shoulder.y", 0.7),
			scalar("torso.right.shoulder.z", 0.20),
		]);

		assert!(quat_angle_rad(chest_rotation(&frame), IDENTITY_QUAT) > 0.1);
		assert!(quat_angle_rad(hips_rotation(&frame), IDENTITY_QUAT) < 1e-4);
	}

	#[test]
	fn chest_stabilization_dampens_only_chest_bones() {
		let rotation = normalize_quat([0.0, 0.35, 0.0, 0.94]);
		let options = ChestStabilizationOptions::new(true, 0.75);
		let damped = stabilize_chest_bone_rotation("Chest", rotation, options);
		let upper_damped = stabilize_chest_bone_rotation("UpperChest", rotation, options);
		let head = stabilize_chest_bone_rotation("Head", rotation, options);

		assert!(quat_angle_rad(damped, IDENTITY_QUAT) < quat_angle_rad(rotation, IDENTITY_QUAT));
		assert!(quat_angle_rad(upper_damped, IDENTITY_QUAT) < quat_angle_rad(rotation, IDENTITY_QUAT));
		assert_eq!(head, rotation);
	}

	#[test]
	fn sink_sends_stabilized_chest_rotation() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target)
			.expect("sink create")
			.with_ok_packet(false)
			.with_chest_stabilization(true, 1.0);
		let mut frame = UNMotionFrame::new(19);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![BoneSample {
					bone: HumanoidBone::Chest,
					transform: transform(normalize_quat([0.0, 0.35, 0.0, 0.94])),
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				}],
			}),
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		let chest = find_message(&messages, "/VMC/Ext/Bone/Pos", "Chest");
		assert!(matches!(chest.args[4], OscType::Float(value) if value.abs() < 1e-4));
		assert!(matches!(chest.args[5], OscType::Float(value) if value.abs() < 1e-4));
		assert!(matches!(chest.args[6], OscType::Float(value) if value.abs() < 1e-4));
		assert!(matches!(chest.args[7], OscType::Float(value) if (value - 1.0).abs() < 1e-4));
	}

	#[test]
	fn leg_and_foot_signals_rotate_lower_body_bones() {
		let mut frame = UNMotionFrame::new(17);
		frame.signals.extend([
			scalar("leg.left.hip.x", -0.2),
			scalar("leg.left.hip.y", 0.0),
			scalar("leg.left.hip.z", 0.0),
			scalar("leg.left.knee.x", -0.35),
			scalar("leg.left.knee.y", -0.45),
			scalar("leg.left.knee.z", 0.10),
			scalar("leg.left.ankle.x", -0.25),
			scalar("leg.left.ankle.y", -0.90),
			scalar("leg.left.ankle.z", 0.20),
			scalar("leg.right.hip.x", 0.2),
			scalar("leg.right.hip.y", 0.0),
			scalar("leg.right.hip.z", 0.0),
			scalar("leg.right.knee.x", 0.35),
			scalar("leg.right.knee.y", -0.45),
			scalar("leg.right.knee.z", 0.10),
			scalar("leg.right.ankle.x", 0.25),
			scalar("leg.right.ankle.y", -0.90),
			scalar("leg.right.ankle.z", 0.20),
			scalar("foot.left.ankle.x", -0.25),
			scalar("foot.left.ankle.y", -0.90),
			scalar("foot.left.ankle.z", 0.20),
			scalar("foot.left.heel.x", -0.25),
			scalar("foot.left.heel.y", -0.95),
			scalar("foot.left.heel.z", 0.05),
			scalar("foot.left.index.x", -0.20),
			scalar("foot.left.index.y", -0.95),
			scalar("foot.left.index.z", 0.45),
		]);

		assert!(quat_angle_rad(upper_leg_rotation(&frame, BodySide::Left), IDENTITY_QUAT) > 0.1);
		assert!(quat_angle_rad(lower_leg_rotation(&frame, BodySide::Left), IDENTITY_QUAT) > 0.1);
		assert!(quat_angle_rad(foot_rotation(&frame, BodySide::Left), IDENTITY_QUAT) > 0.1);
	}

	#[test]
	fn direct_torso_leg_and_foot_pose_triggers_vmc_bone_packets() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create").with_ok_packet(false);
		let mut frame = UNMotionFrame::new(18);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: [
					HumanoidBone::Chest,
					HumanoidBone::LeftUpperLeg,
					HumanoidBone::LeftFoot,
					HumanoidBone::LeftToes,
				]
				.into_iter()
				.map(|bone| BoneSample {
					bone,
					transform: transform([0.0, 0.0, 0.25, 0.9682458]),
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				})
				.collect(),
			}),
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		let names = message_names(&messages);
		assert!(names.contains(&"Chest".to_string()));
		assert!(names.contains(&"LeftUpperLeg".to_string()));
		assert!(names.contains(&"LeftFoot".to_string()));
		assert!(names.contains(&"LeftToes".to_string()));
	}

	#[test]
	fn lower_arm_rotation_is_local_to_upper_arm() {
		let mut frame = UNMotionFrame::new(9);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.3),
			scalar("arm.left.shoulder.y", 0.0),
			scalar("arm.left.elbow.x", -0.3),
			scalar("arm.left.elbow.y", 1.0),
			scalar("arm.left.wrist.x", -0.3),
			scalar("arm.left.wrist.y", 2.0),
		]);

		let upper = upper_arm_rotation(&frame, BodySide::Left);
		let lower = lower_arm_rotation(&frame, BodySide::Left);

		assert!(quat_angle_rad(upper, IDENTITY_QUAT) > 0.5);
		assert!(quat_angle_rad(lower, IDENTITY_QUAT) < 1e-4);
	}

	#[test]
	fn hand_rotation_is_local_to_lower_arm_when_arm_signals_exist() {
		let mut frame = UNMotionFrame::new(10);
		frame.signals.extend([
			scalar("arm.right.elbow.x", 0.3),
			scalar("arm.right.elbow.y", 0.0),
			scalar("arm.right.wrist.x", 0.3),
			scalar("arm.right.wrist.y", 1.0),
		]);
		let lower_global = quat_from_to(arm_rest_axis(BodySide::Right), [0.0, 1.0, 0.0]);

		let hand = hand_local_rotation(&frame, BodySide::Right, lower_global);

		assert!(quat_angle_rad(hand, IDENTITY_QUAT) < 1e-4);
	}

	#[test]
	fn hand_rotation_without_palm_basis_stays_local_to_forearm() {
		let mut frame = UNMotionFrame::new(15);
		frame.signals.extend([
			scalar("arm.left.elbow.x", -0.3),
			scalar("arm.left.elbow.y", 0.0),
			scalar("arm.left.wrist.x", -0.3),
			scalar("arm.left.wrist.y", 1.0),
		]);

		let hand = hand_rotation(&frame, BodySide::Left);

		assert!(quat_angle_rad(hand, IDENTITY_QUAT) < 1e-4);
	}

	#[test]
	fn left_foldback_lower_arm_without_hand_basis_keeps_lower_arm_neutral() {
		let mut frame = UNMotionFrame::new(20);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.18),
			scalar("arm.left.shoulder.y", 0.48),
			scalar("arm.left.shoulder.z", 0.03),
			scalar("arm.left.elbow.x", -0.12),
			scalar("arm.left.elbow.y", 0.30),
			scalar("arm.left.elbow.z", 0.14),
			scalar("arm.left.wrist.x", -0.03),
			scalar("arm.left.wrist.y", 0.44),
			scalar("arm.left.wrist.z", 0.23),
		]);

		let lower = lower_arm_rotation(&frame, BodySide::Left);

		assert!(quat_angle_rad(lower, IDENTITY_QUAT) < 1e-4);
	}

	#[test]
	fn left_foldback_lower_arm_keeps_hand_basis_when_available() {
		let mut frame = UNMotionFrame::new(21);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.18),
			scalar("arm.left.shoulder.y", 0.48),
			scalar("arm.left.shoulder.z", 0.03),
			scalar("arm.left.elbow.x", -0.12),
			scalar("arm.left.elbow.y", 0.30),
			scalar("arm.left.elbow.z", 0.14),
			scalar("arm.left.wrist.x", -0.03),
			scalar("arm.left.wrist.y", 0.44),
			scalar("arm.left.wrist.z", 0.23),
			scalar("hand.left.palm.forward.x", 0.0),
			scalar("hand.left.palm.forward.y", 1.0),
			scalar("hand.left.palm.forward.z", 0.0),
			scalar("hand.left.palm.across.x", 1.0),
			scalar("hand.left.palm.across.y", 0.0),
			scalar("hand.left.palm.across.z", 0.0),
		]);

		let lower = lower_arm_rotation(&frame, BodySide::Left);

		assert!(quat_angle_rad(lower, IDENTITY_QUAT) > 0.2);
	}

	#[test]
	fn lower_arm_global_rotation_tracks_palm_twist_without_changing_direction() {
		let mut frame = UNMotionFrame::new(14);
		frame.signals.extend([
			scalar("arm.right.elbow.x", 0.3),
			scalar("arm.right.elbow.y", 0.0),
			scalar("arm.right.wrist.x", 0.3),
			scalar("arm.right.wrist.y", 1.0),
			scalar("hand.right.palm.forward.x", 1.0),
			scalar("hand.right.palm.forward.y", 0.0),
			scalar("hand.right.palm.forward.z", 0.0),
			scalar("hand.right.palm.across.x", 0.0),
			scalar("hand.right.palm.across.y", 0.0),
			scalar("hand.right.palm.across.z", 1.0),
			scalar("hand.right.palm.normal.x", 0.0),
			scalar("hand.right.palm.normal.y", 0.0),
			scalar("hand.right.palm.normal.z", 1.0),
		]);
		let raw = quat_from_to(arm_rest_axis(BodySide::Right), [0.0, 1.0, 0.0]);
		let lower = lower_arm_global_rotation(&frame, BodySide::Right).expect("lower arm");
		let lower_axis = quat_rotate_vec3(lower, arm_rest_axis(BodySide::Right));
		let lower_palm = quat_rotate_vec3(lower, [0.0, -1.0, 0.0]);

		assert!(quat_angle_rad(lower, raw) > 0.2);
		assert_vec3_near(lower_axis, [0.0, 1.0, 0.0], 0.001);
		assert!(
			lower_palm[2] > 0.99,
			"forearm wrist twist should follow palm normal: {lower_palm:?}"
		);
	}

	#[test]
	fn hidden_back_hands_use_stable_arm_fallback() {
		let mut frame = UNMotionFrame::new(29);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.185),
			scalar("arm.left.shoulder.y", 0.480),
			scalar("arm.left.shoulder.z", 0.085),
			scalar("arm.left.elbow.x", -0.268),
			scalar("arm.left.elbow.y", 0.284),
			scalar("arm.left.elbow.z", 0.003),
			scalar("arm.left.wrist.x", -0.160),
			scalar("arm.left.wrist.y", 0.149),
			scalar("arm.left.wrist.z", 0.034),
			scalar("arm.right.shoulder.x", 0.145),
			scalar("arm.right.shoulder.y", 0.474),
			scalar("arm.right.shoulder.z", 0.078),
			scalar("arm.right.elbow.x", 0.217),
			scalar("arm.right.elbow.y", 0.265),
			scalar("arm.right.elbow.z", 0.032),
			scalar("arm.right.wrist.x", 0.120),
			scalar("arm.right.wrist.y", 0.155),
			scalar("arm.right.wrist.z", 0.099),
		]);

		assert!(hidden_back_hands_pose(&frame));
		assert!(
			quat_angle_rad(
				upper_arm_rotation(&frame, BodySide::Left),
				hidden_back_upper_arm_rotation(BodySide::Left)
			) < 0.001
		);
		assert!(
			quat_angle_rad(
				lower_arm_rotation(&frame, BodySide::Right),
				hidden_back_lower_arm_rotation(BodySide::Right)
			) < 0.001
		);
	}

	#[test]
	fn detected_hand_blocks_hidden_back_fallback() {
		let mut frame = UNMotionFrame::new(30);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.185),
			scalar("arm.left.shoulder.y", 0.480),
			scalar("arm.left.shoulder.z", 0.085),
			scalar("arm.left.elbow.x", -0.268),
			scalar("arm.left.elbow.y", 0.284),
			scalar("arm.left.elbow.z", 0.003),
			scalar("arm.left.wrist.x", -0.160),
			scalar("arm.left.wrist.y", 0.149),
			scalar("arm.left.wrist.z", 0.034),
			scalar("arm.right.shoulder.x", 0.145),
			scalar("arm.right.shoulder.y", 0.474),
			scalar("arm.right.shoulder.z", 0.078),
			scalar("arm.right.elbow.x", 0.217),
			scalar("arm.right.elbow.y", 0.265),
			scalar("arm.right.elbow.z", 0.032),
			scalar("arm.right.wrist.x", 0.120),
			scalar("arm.right.wrist.y", 0.155),
			scalar("arm.right.wrist.z", 0.099),
			scalar("hand.right.palm.forward.x", 1.0),
			scalar("hand.right.palm.forward.y", 0.0),
			scalar("hand.right.palm.forward.z", 0.0),
			scalar("hand.right.palm.across.x", 0.0),
			scalar("hand.right.palm.across.y", 0.0),
			scalar("hand.right.palm.across.z", 1.0),
		]);

		assert!(!hidden_back_hands_pose(&frame));
	}

	#[test]
	fn lateral_stop_palms_disable_lower_arm_hand_basis_blend() {
		let mut frame = UNMotionFrame::new(31);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.173),
			scalar("arm.left.shoulder.y", 0.511),
			scalar("arm.left.shoulder.z", 0.031),
			scalar("arm.left.elbow.x", -0.325),
			scalar("arm.left.elbow.y", 0.419),
			scalar("arm.left.elbow.z", 0.059),
			scalar("arm.left.wrist.x", -0.477),
			scalar("arm.left.wrist.y", 0.447),
			scalar("arm.left.wrist.z", 0.191),
			scalar("hand.left.palm.forward.x", -0.078),
			scalar("hand.left.palm.forward.y", 0.957),
			scalar("hand.left.palm.forward.z", 0.279),
			scalar("hand.left.palm.across.x", 0.463),
			scalar("hand.left.palm.across.y", 0.210),
			scalar("hand.left.palm.across.z", -0.861),
			scalar("arm.right.shoulder.x", 0.142),
			scalar("arm.right.shoulder.y", 0.487),
			scalar("arm.right.shoulder.z", 0.028),
			scalar("arm.right.elbow.x", 0.326),
			scalar("arm.right.elbow.y", 0.373),
			scalar("arm.right.elbow.z", 0.062),
			scalar("arm.right.wrist.x", 0.489),
			scalar("arm.right.wrist.y", 0.416),
			scalar("arm.right.wrist.z", 0.198),
			scalar("hand.right.palm.forward.x", 0.055),
			scalar("hand.right.palm.forward.y", 0.999),
			scalar("hand.right.palm.forward.z", 0.002),
			scalar("hand.right.palm.across.x", -0.747),
			scalar("hand.right.palm.across.y", 0.278),
			scalar("hand.right.palm.across.z", -0.604),
		]);

		assert!(outward_stop_palms_pose(&frame));
		assert_eq!(lower_arm_hand_basis_blend(&frame), 0.0);
	}

	#[test]
	fn side_raised_stop_palms_disable_lower_arm_hand_basis_blend() {
		let mut frame = UNMotionFrame::new(33);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.182),
			scalar("arm.left.shoulder.y", 0.456),
			scalar("arm.left.shoulder.z", 0.100),
			scalar("arm.left.elbow.x", -0.350),
			scalar("arm.left.elbow.y", 0.385),
			scalar("arm.left.elbow.z", 0.174),
			scalar("arm.left.wrist.x", -0.366),
			scalar("arm.left.wrist.y", 0.540),
			scalar("arm.left.wrist.z", 0.339),
			scalar("hand.left.palm.forward.x", 0.060),
			scalar("hand.left.palm.forward.y", 0.992),
			scalar("hand.left.palm.forward.z", -0.108),
			scalar("hand.left.palm.across.x", 0.842),
			scalar("hand.left.palm.across.y", 0.294),
			scalar("hand.left.palm.across.z", -0.451),
			scalar("arm.right.shoulder.x", 0.149),
			scalar("arm.right.shoulder.y", 0.462),
			scalar("arm.right.shoulder.z", 0.093),
			scalar("arm.right.elbow.x", 0.323),
			scalar("arm.right.elbow.y", 0.384),
			scalar("arm.right.elbow.z", 0.194),
			scalar("arm.right.wrist.x", 0.373),
			scalar("arm.right.wrist.y", 0.549),
			scalar("arm.right.wrist.z", 0.351),
			scalar("hand.right.palm.forward.x", -0.152),
			scalar("hand.right.palm.forward.y", 0.982),
			scalar("hand.right.palm.forward.z", -0.110),
			scalar("hand.right.palm.across.x", -0.865),
			scalar("hand.right.palm.across.y", 0.166),
			scalar("hand.right.palm.across.z", -0.473),
		]);

		assert!(outward_stop_palms_pose(&frame));
		assert_eq!(lower_arm_hand_basis_blend(&frame), 0.0);
	}

	#[test]
	fn close_front_prayer_disables_lower_arm_hand_basis_blend() {
		let mut frame = UNMotionFrame::new(35);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.186),
			scalar("arm.left.shoulder.y", 0.478),
			scalar("arm.left.shoulder.z", 0.030),
			scalar("arm.left.elbow.x", -0.118),
			scalar("arm.left.elbow.y", 0.297),
			scalar("arm.left.elbow.z", 0.138),
			scalar("arm.left.wrist.x", -0.032),
			scalar("arm.left.wrist.y", 0.437),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.104),
			scalar("arm.right.shoulder.y", 0.479),
			scalar("arm.right.shoulder.z", 0.059),
			scalar("arm.right.elbow.x", 0.023),
			scalar("arm.right.elbow.y", 0.267),
			scalar("arm.right.elbow.z", 0.199),
			scalar("arm.right.wrist.x", -0.077),
			scalar("arm.right.wrist.y", 0.411),
			scalar("arm.right.wrist.z", 0.287),
			scalar("hand.right.palm.forward.x", 0.142),
			scalar("hand.right.palm.forward.y", 0.989),
			scalar("hand.right.palm.forward.z", -0.038),
			scalar("hand.right.palm.across.x", 0.066),
			scalar("hand.right.palm.across.y", 0.288),
			scalar("hand.right.palm.across.z", -0.955),
		]);

		assert!(close_front_prayer_pose(&frame));
		assert_eq!(lower_arm_hand_basis_blend(&frame), 0.0);
	}

	#[test]
	fn forward_stop_palms_keep_lower_arm_hand_twist_local() {
		let mut frame = UNMotionFrame::new(32);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.170),
			scalar("arm.left.shoulder.y", 0.470),
			scalar("arm.left.shoulder.z", 0.103),
			scalar("arm.left.elbow.x", -0.246),
			scalar("arm.left.elbow.y", 0.290),
			scalar("arm.left.elbow.z", 0.233),
			scalar("arm.left.wrist.x", -0.244),
			scalar("arm.left.wrist.y", 0.363),
			scalar("arm.left.wrist.z", 0.440),
			scalar("hand.left.palm.forward.x", 0.146),
			scalar("hand.left.palm.forward.y", 0.983),
			scalar("hand.left.palm.forward.z", 0.108),
			scalar("hand.left.palm.across.x", 0.950),
			scalar("hand.left.palm.across.y", 0.272),
			scalar("hand.left.palm.across.z", -0.152),
			scalar("arm.right.shoulder.x", 0.161),
			scalar("arm.right.shoulder.y", 0.476),
			scalar("arm.right.shoulder.z", 0.081),
			scalar("arm.right.elbow.x", 0.219),
			scalar("arm.right.elbow.y", 0.287),
			scalar("arm.right.elbow.z", 0.222),
			scalar("arm.right.wrist.x", 0.218),
			scalar("arm.right.wrist.y", 0.358),
			scalar("arm.right.wrist.z", 0.445),
			scalar("hand.right.palm.forward.x", -0.239),
			scalar("hand.right.palm.forward.y", 0.960),
			scalar("hand.right.palm.forward.z", 0.144),
			scalar("hand.right.palm.across.x", -0.943),
			scalar("hand.right.palm.across.y", 0.215),
			scalar("hand.right.palm.across.z", -0.254),
		]);

		assert!(!outward_stop_palms_pose(&frame));
		assert!(forward_stop_palms_pose(&frame));
		assert!(forward_stop_palm_pose(&frame, BodySide::Left));
		assert!(forward_stop_palm_pose(&frame, BodySide::Right));
		assert!(
			quat_angle_rad(
				upper_arm_rotation(&frame, BodySide::Left),
				forward_stop_palm_upper_arm_rotation(BodySide::Left)
			) < 0.001
		);
		assert!(
			quat_angle_rad(
				lower_arm_rotation(&frame, BodySide::Right),
				forward_stop_palm_lower_arm_rotation(BodySide::Right)
			) < 0.001
		);
		assert!(
			quat_angle_rad(
				hand_rotation(&frame, BodySide::Left),
				forward_stop_palm_hand_rotation(BodySide::Left)
			) < 0.001
		);
		assert_eq!(lower_arm_hand_basis_blend(&frame), 0.0);
	}

	#[test]
	fn one_sided_forward_stop_palm_uses_stable_arm_fallback() {
		let mut frame = UNMotionFrame::new(36);
		frame.signals.extend([
			scalar("arm.right.shoulder.x", 0.132),
			scalar("arm.right.shoulder.y", 0.498),
			scalar("arm.right.shoulder.z", 0.085),
			scalar("arm.right.elbow.x", 0.148),
			scalar("arm.right.elbow.y", 0.379),
			scalar("arm.right.elbow.z", 0.178),
			scalar("arm.right.wrist.x", 0.060),
			scalar("arm.right.wrist.y", 0.417),
			scalar("arm.right.wrist.z", 0.330),
			scalar("hand.right.palm.forward.x", -0.308),
			scalar("hand.right.palm.forward.y", 0.940),
			scalar("hand.right.palm.forward.z", 0.145),
			scalar("hand.right.palm.across.x", -0.922),
			scalar("hand.right.palm.across.y", 0.168),
			scalar("hand.right.palm.across.z", -0.349),
		]);

		assert!(!forward_stop_palms_pose(&frame));
		assert!(forward_stop_palm_pose(&frame, BodySide::Right));
		assert!(
			quat_angle_rad(
				upper_arm_rotation(&frame, BodySide::Right),
				forward_stop_palm_upper_arm_rotation(BodySide::Right)
			) < 0.001
		);
		assert!(
			quat_angle_rad(
				lower_arm_rotation(&frame, BodySide::Right),
				forward_stop_palm_lower_arm_rotation(BodySide::Right)
			) < 0.001
		);
		assert!(
			quat_angle_rad(
				hand_rotation(&frame, BodySide::Right),
				forward_stop_palm_hand_rotation(BodySide::Right)
			) < 0.001
		);
	}

	#[test]
	fn wrist_roll_front_palms_do_not_use_forward_stop_fallback() {
		let mut frame = UNMotionFrame::new(34);
		frame.signals.extend([
			scalar("arm.left.elbow.x", -0.10),
			scalar("arm.left.elbow.y", 0.10),
			scalar("arm.left.elbow.z", 0.10),
			scalar("arm.left.wrist.x", 0.03),
			scalar("arm.left.wrist.y", 0.66),
			scalar("arm.left.wrist.z", 0.92),
			scalar("hand.left.palm.forward.x", 0.0),
			scalar("hand.left.palm.forward.y", 1.0),
			scalar("hand.left.palm.forward.z", 0.0),
			scalar("hand.left.palm.across.x", 0.9),
			scalar("hand.left.palm.across.y", 0.0),
			scalar("hand.left.palm.across.z", -0.17),
			scalar("arm.right.elbow.x", 0.10),
			scalar("arm.right.elbow.y", 0.10),
			scalar("arm.right.elbow.z", 0.10),
			scalar("arm.right.wrist.x", -0.02),
			scalar("arm.right.wrist.y", 0.66),
			scalar("arm.right.wrist.z", 0.92),
			scalar("hand.right.palm.forward.x", 0.0),
			scalar("hand.right.palm.forward.y", 0.98),
			scalar("hand.right.palm.forward.z", 0.0),
			scalar("hand.right.palm.across.x", -0.9),
			scalar("hand.right.palm.across.y", 0.0),
			scalar("hand.right.palm.across.z", -0.15),
		]);

		assert!(!forward_stop_palms_pose(&frame));
	}

	#[test]
	fn left_open_palm_biases_arm_roll_toward_camera_back() {
		let mut frame = UNMotionFrame::new(12);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", 0.0),
			scalar("arm.left.shoulder.y", 0.0),
			scalar("arm.left.shoulder.z", 0.0),
			scalar("arm.left.elbow.x", -0.65),
			scalar("arm.left.elbow.y", -0.64),
			scalar("arm.left.elbow.z", 0.41),
			scalar("arm.left.wrist.x", -0.89),
			scalar("arm.left.wrist.y", 0.07),
			scalar("arm.left.wrist.z", 1.07),
			scalar("hand.left.palm.normal.x", 0.0),
			scalar("hand.left.palm.normal.y", 0.0),
			scalar("hand.left.palm.normal.z", -1.0),
		]);

		let upper = arm_segment_direction(&frame, "left", "shoulder", "elbow").expect("upper");
		let lower = arm_segment_direction(&frame, "left", "elbow", "wrist").expect("lower");
		let raw = normalize3(cross3(upper, lower)).expect("raw plane");
		let adjusted = arm_plane_secondary(&frame, BodySide::Left, ArmSegmentRole::Upper).expect("adjusted plane");

		assert!(adjusted[2] < raw[2]);
	}

	#[test]
	fn left_back_palm_does_not_override_arm_roll_plane() {
		let mut frame = UNMotionFrame::new(13);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.3),
			scalar("arm.left.shoulder.y", 0.17),
			scalar("arm.left.shoulder.z", 0.02),
			scalar("arm.left.elbow.x", -0.56),
			scalar("arm.left.elbow.y", -0.17),
			scalar("arm.left.elbow.z", 0.24),
			scalar("arm.left.wrist.x", -0.69),
			scalar("arm.left.wrist.y", 0.12),
			scalar("arm.left.wrist.z", 0.58),
			scalar("hand.left.palm.normal.x", 0.2),
			scalar("hand.left.palm.normal.y", -0.25),
			scalar("hand.left.palm.normal.z", 0.95),
		]);

		let upper = arm_segment_direction(&frame, "left", "shoulder", "elbow").expect("upper");
		let lower = arm_segment_direction(&frame, "left", "elbow", "wrist").expect("lower");
		let raw = normalize3(cross3(upper, lower)).expect("raw plane");
		let adjusted = arm_plane_secondary(&frame, BodySide::Left, ArmSegmentRole::Upper).expect("adjusted plane");

		assert!(dot3(adjusted, raw) > 0.999);
	}

	#[test]
	fn left_extended_stop_palm_inverts_unstable_across_axis() {
		let mut frame = UNMotionFrame::new(15);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.170),
			scalar("arm.left.shoulder.y", 0.470),
			scalar("arm.left.shoulder.z", 0.103),
			scalar("arm.left.elbow.x", -0.246),
			scalar("arm.left.elbow.y", 0.290),
			scalar("arm.left.elbow.z", 0.233),
			scalar("arm.left.wrist.x", -0.244),
			scalar("arm.left.wrist.y", 0.363),
			scalar("arm.left.wrist.z", 0.440),
			scalar("hand.left.palm.forward.x", 0.146),
			scalar("hand.left.palm.forward.y", 0.983),
			scalar("hand.left.palm.forward.z", 0.108),
			scalar("hand.left.palm.normal.x", 0.194),
			scalar("hand.left.palm.normal.y", -0.136),
			scalar("hand.left.palm.normal.z", 0.972),
			scalar("hand.left.middle.curl", 0.025),
			scalar("hand.left.ring.curl", 0.020),
			scalar("hand.left.little.curl", 0.045),
		]);

		assert!(left_extended_stop_palm_needs_across_inversion(&frame, [0.146, 0.983, 0.108]));
	}

	#[test]
	fn left_wrist_roll_front_palm_keeps_across_axis() {
		let mut frame = UNMotionFrame::new(16);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.170),
			scalar("arm.left.shoulder.y", 0.470),
			scalar("arm.left.shoulder.z", 0.103),
			scalar("arm.left.elbow.x", -0.246),
			scalar("arm.left.elbow.y", 0.290),
			scalar("arm.left.elbow.z", 0.233),
			scalar("arm.left.wrist.x", -0.234),
			scalar("arm.left.wrist.y", 0.398),
			scalar("arm.left.wrist.z", 0.377),
			scalar("hand.left.palm.forward.x", 0.049),
			scalar("hand.left.palm.forward.y", 0.998),
			scalar("hand.left.palm.forward.z", 0.042),
			scalar("hand.left.palm.normal.x", 0.199),
			scalar("hand.left.palm.normal.y", -0.051),
			scalar("hand.left.palm.normal.z", 0.979),
			scalar("hand.left.middle.curl", 0.045),
			scalar("hand.left.ring.curl", 0.045),
			scalar("hand.left.little.curl", 0.045),
		]);

		assert!(!left_extended_stop_palm_needs_across_inversion(&frame, [0.049, 0.998, 0.042]));
	}

	#[test]
	fn close_overhead_hands_can_share_opposite_palm_basis() {
		let mut frame = UNMotionFrame::new(22);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.151),
			scalar("arm.left.shoulder.y", 0.527),
			scalar("arm.left.shoulder.z", 0.113),
			scalar("arm.left.elbow.x", -0.175),
			scalar("arm.left.elbow.y", 0.730),
			scalar("arm.left.elbow.z", 0.186),
			scalar("arm.left.wrist.x", -0.059),
			scalar("arm.left.wrist.y", 0.914),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.137),
			scalar("arm.right.shoulder.y", 0.512),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.178),
			scalar("arm.right.elbow.y", 0.738),
			scalar("arm.right.elbow.z", 0.221),
			scalar("arm.right.wrist.x", 0.093),
			scalar("arm.right.wrist.y", 0.956),
			scalar("arm.right.wrist.z", 0.230),
		]);

		assert!(hands_are_close_overhead(&frame));
	}

	#[test]
	fn close_overhead_hands_do_not_pull_forearm_toward_hand_basis() {
		let mut frame = UNMotionFrame::new(24);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.151),
			scalar("arm.left.shoulder.y", 0.527),
			scalar("arm.left.shoulder.z", 0.113),
			scalar("arm.left.elbow.x", -0.175),
			scalar("arm.left.elbow.y", 0.730),
			scalar("arm.left.elbow.z", 0.186),
			scalar("arm.left.wrist.x", -0.059),
			scalar("arm.left.wrist.y", 0.914),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.137),
			scalar("arm.right.shoulder.y", 0.512),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.178),
			scalar("arm.right.elbow.y", 0.738),
			scalar("arm.right.elbow.z", 0.221),
			scalar("arm.right.wrist.x", 0.093),
			scalar("arm.right.wrist.y", 0.956),
			scalar("arm.right.wrist.z", 0.230),
			scalar("hand.right.palm.forward.x", -0.1),
			scalar("hand.right.palm.forward.y", 0.98),
			scalar("hand.right.palm.forward.z", 0.1),
			scalar("hand.right.palm.across.x", 0.0),
			scalar("hand.right.palm.across.y", 0.1),
			scalar("hand.right.palm.across.z", 0.99),
		]);

		assert_eq!(lower_arm_hand_basis_blend(&frame), 0.0);
	}

	#[test]
	fn close_overhead_right_upper_arm_uses_palm_across_as_roll_axis() {
		let mut frame = UNMotionFrame::new(25);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.151),
			scalar("arm.left.shoulder.y", 0.527),
			scalar("arm.left.shoulder.z", 0.113),
			scalar("arm.left.elbow.x", -0.175),
			scalar("arm.left.elbow.y", 0.730),
			scalar("arm.left.elbow.z", 0.186),
			scalar("arm.left.wrist.x", -0.059),
			scalar("arm.left.wrist.y", 0.914),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.137),
			scalar("arm.right.shoulder.y", 0.512),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.178),
			scalar("arm.right.elbow.y", 0.738),
			scalar("arm.right.elbow.z", 0.221),
			scalar("arm.right.wrist.x", 0.093),
			scalar("arm.right.wrist.y", 0.956),
			scalar("arm.right.wrist.z", 0.230),
			scalar("hand.right.palm.forward.x", -0.073),
			scalar("hand.right.palm.forward.y", 0.995),
			scalar("hand.right.palm.forward.z", -0.068),
			scalar("hand.right.palm.normal.x", 0.983),
			scalar("hand.right.palm.normal.y", 0.061),
			scalar("hand.right.palm.normal.z", -0.172),
			scalar("hand.right.palm.across.x", -0.182),
			scalar("hand.right.palm.across.y", 0.259),
			scalar("hand.right.palm.across.z", -0.949),
		]);

		let axis = arm_plane_secondary(&frame, BodySide::Right, ArmSegmentRole::Upper).expect("right upper arm plane");

		assert!(dot3(axis, [-0.182, 0.259, -0.949]) > 0.99);
	}

	#[test]
	fn close_overhead_right_lower_arm_keeps_geometric_roll_axis() {
		let mut frame = UNMotionFrame::new(26);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.151),
			scalar("arm.left.shoulder.y", 0.527),
			scalar("arm.left.shoulder.z", 0.113),
			scalar("arm.left.elbow.x", -0.175),
			scalar("arm.left.elbow.y", 0.730),
			scalar("arm.left.elbow.z", 0.186),
			scalar("arm.left.wrist.x", -0.059),
			scalar("arm.left.wrist.y", 0.914),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.137),
			scalar("arm.right.shoulder.y", 0.512),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.178),
			scalar("arm.right.elbow.y", 0.738),
			scalar("arm.right.elbow.z", 0.221),
			scalar("arm.right.wrist.x", 0.093),
			scalar("arm.right.wrist.y", 0.956),
			scalar("arm.right.wrist.z", 0.230),
			scalar("hand.right.palm.forward.x", -0.073),
			scalar("hand.right.palm.forward.y", 0.995),
			scalar("hand.right.palm.forward.z", -0.068),
			scalar("hand.right.palm.normal.x", 0.983),
			scalar("hand.right.palm.normal.y", 0.061),
			scalar("hand.right.palm.normal.z", -0.172),
			scalar("hand.right.palm.across.x", -0.182),
			scalar("hand.right.palm.across.y", 0.259),
			scalar("hand.right.palm.across.z", -0.949),
		]);

		let upper = arm_segment_direction(&frame, "right", "shoulder", "elbow").expect("upper");
		let lower = arm_segment_direction(&frame, "right", "elbow", "wrist").expect("lower");
		let raw = scale3(normalize3(cross3(upper, lower)).expect("raw plane"), side_sign(BodySide::Right));
		let axis = arm_plane_secondary(&frame, BodySide::Right, ArmSegmentRole::Lower).expect("right lower arm plane");

		assert!(dot3(axis, raw) > 0.999);
	}

	#[test]
	fn close_overhead_missing_lower_arm_mirrors_detected_opposite_side() {
		let mut frame = UNMotionFrame::new(29);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.151),
			scalar("arm.left.shoulder.y", 0.527),
			scalar("arm.left.shoulder.z", 0.113),
			scalar("arm.left.elbow.x", -0.175),
			scalar("arm.left.elbow.y", 0.730),
			scalar("arm.left.elbow.z", 0.186),
			scalar("arm.left.wrist.x", -0.059),
			scalar("arm.left.wrist.y", 0.914),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.137),
			scalar("arm.right.shoulder.y", 0.512),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.178),
			scalar("arm.right.elbow.y", 0.738),
			scalar("arm.right.elbow.z", 0.221),
			scalar("arm.right.wrist.x", 0.093),
			scalar("arm.right.wrist.y", 0.956),
			scalar("arm.right.wrist.z", 0.230),
			scalar("hand.right.palm.forward.x", -0.073),
			scalar("hand.right.palm.forward.y", 0.995),
			scalar("hand.right.palm.forward.z", -0.068),
			scalar("hand.right.palm.normal.x", 0.983),
			scalar("hand.right.palm.normal.y", 0.061),
			scalar("hand.right.palm.normal.z", -0.172),
			scalar("hand.right.palm.across.x", -0.182),
			scalar("hand.right.palm.across.y", 0.259),
			scalar("hand.right.palm.across.z", -0.949),
		]);
		let mirrored = mirrored_opposite_lower_arm_local_rotation(&frame, BodySide::Left).expect("mirrored lower arm");
		let lower = lower_arm_rotation(&frame, BodySide::Left);

		assert!(quat_angle_rad(lower, mirrored) < 0.001);
	}

	#[test]
	fn close_overhead_detected_hand_adds_forearm_depth() {
		let mut frame = UNMotionFrame::new(27);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.151),
			scalar("arm.left.shoulder.y", 0.527),
			scalar("arm.left.shoulder.z", 0.113),
			scalar("arm.left.elbow.x", -0.175),
			scalar("arm.left.elbow.y", 0.730),
			scalar("arm.left.elbow.z", 0.186),
			scalar("arm.left.wrist.x", -0.059),
			scalar("arm.left.wrist.y", 0.914),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.137),
			scalar("arm.right.shoulder.y", 0.512),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.178),
			scalar("arm.right.elbow.y", 0.738),
			scalar("arm.right.elbow.z", 0.221),
			scalar("arm.right.wrist.x", 0.093),
			scalar("arm.right.wrist.y", 0.956),
			scalar("arm.right.wrist.z", 0.230),
			scalar("hand.right.palm.across.x", -0.182),
			scalar("hand.right.palm.across.y", 0.259),
			scalar("hand.right.palm.across.z", -0.949),
		]);
		let lower = arm_segment_direction(&frame, "right", "elbow", "wrist").expect("lower");
		let adjusted = overhead_lower_arm_direction(&frame, BodySide::Right, lower);

		assert!(adjusted[2] > 0.85);
		assert!(adjusted[1] < lower[1]);
	}

	#[test]
	fn close_overhead_missing_hand_keeps_forearm_direction() {
		let mut frame = UNMotionFrame::new(28);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.151),
			scalar("arm.left.shoulder.y", 0.527),
			scalar("arm.left.shoulder.z", 0.113),
			scalar("arm.left.elbow.x", -0.175),
			scalar("arm.left.elbow.y", 0.730),
			scalar("arm.left.elbow.z", 0.186),
			scalar("arm.left.wrist.x", -0.059),
			scalar("arm.left.wrist.y", 0.914),
			scalar("arm.left.wrist.z", 0.231),
			scalar("arm.right.shoulder.x", 0.137),
			scalar("arm.right.shoulder.y", 0.512),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.178),
			scalar("arm.right.elbow.y", 0.738),
			scalar("arm.right.elbow.z", 0.221),
			scalar("arm.right.wrist.x", 0.093),
			scalar("arm.right.wrist.y", 0.956),
			scalar("arm.right.wrist.z", 0.230),
		]);
		let lower = arm_segment_direction(&frame, "left", "elbow", "wrist").expect("lower");
		let adjusted = overhead_lower_arm_direction(&frame, BodySide::Left, lower);

		assert!(dot3(adjusted, lower) > 0.999);
	}

	#[test]
	fn close_low_hands_do_not_share_opposite_palm_basis() {
		let mut frame = UNMotionFrame::new(23);
		frame.signals.extend([
			scalar("arm.left.shoulder.x", -0.200),
			scalar("arm.left.shoulder.y", 0.520),
			scalar("arm.left.shoulder.z", 0.110),
			scalar("arm.left.elbow.x", -0.180),
			scalar("arm.left.elbow.y", 0.460),
			scalar("arm.left.elbow.z", 0.180),
			scalar("arm.left.wrist.x", -0.050),
			scalar("arm.left.wrist.y", 0.437),
			scalar("arm.left.wrist.z", 0.230),
			scalar("arm.right.shoulder.x", 0.140),
			scalar("arm.right.shoulder.y", 0.510),
			scalar("arm.right.shoulder.z", 0.130),
			scalar("arm.right.elbow.x", 0.160),
			scalar("arm.right.elbow.y", 0.480),
			scalar("arm.right.elbow.z", 0.210),
			scalar("arm.right.wrist.x", 0.080),
			scalar("arm.right.wrist.y", 0.455),
			scalar("arm.right.wrist.z", 0.230),
		]);

		assert!(!hands_are_close_overhead(&frame));
	}

	#[test]
	fn hand_rotation_prefers_validated_palm_basis_over_wrist_euler() {
		let mut frame = UNMotionFrame::new(11);
		frame.signals.extend([
			scalar("hand.right.palm.forward.x", 1.0),
			scalar("hand.right.palm.forward.y", 0.0),
			scalar("hand.right.palm.forward.z", 0.0),
			scalar("hand.right.palm.across.x", 0.0),
			scalar("hand.right.palm.across.y", 0.0),
			scalar("hand.right.palm.across.z", 1.0),
			scalar("hand.right.palm.normal.x", 0.0),
			scalar("hand.right.palm.normal.y", -1.0),
			scalar("hand.right.palm.normal.z", 0.0),
			scalar("hand.right.wrist.pitch", 1.0),
			scalar("hand.right.wrist.yaw", 1.0),
			scalar("hand.right.wrist.roll", 1.0),
		]);

		let hand = hand_rotation(&frame, BodySide::Right);

		assert!(quat_angle_rad(hand, IDENTITY_QUAT) < 1e-4);
	}

	#[test]
	fn sink_maps_typed_finger_motion_to_vmc_humanoid_bones() {
		let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
		receiver
			.set_read_timeout(Some(std::time::Duration::from_millis(500)))
			.expect("set timeout");
		let target = receiver.local_addr().expect("local addr");

		let mut sink = VmcOutputSink::new(target).expect("sink create").with_ok_packet(false);
		let mut frame = UNMotionFrame::new(7);
		frame.right_hand = Some(HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			wrist: None,
			fingers: vec![FingerPose {
				finger: Finger::Index,
				joints: vec![
					transform([0.0, 0.05, 0.10, 0.993]),
					transform([0.0, 0.0, -0.35, 0.937]),
					transform([0.0, 0.0, -0.20, 0.980]),
				],
				confidence: 1.0,
			}],
		});

		sink.send(&frame).expect("send should succeed");

		let mut buf = [0_u8; 65535];
		let messages = recv_messages(&receiver, &mut buf);
		assert!(messages.iter().any(|message| message.addr == "/VMC/Ext/Root/Pos"));
		let names = message_names(&messages);

		assert!(names.contains(&"RightIndexProximal".to_string()));
		assert!(names.contains(&"RightIndexIntermediate".to_string()));
		assert!(names.contains(&"RightIndexDistal".to_string()));
		assert_eq!(names.len(), HUMANOID_BONE_REST_POSES.len());

		let proximal = find_message(&messages, "/VMC/Ext/Bone/Pos", "RightIndexProximal");
		assert!(matches!(proximal.args[5], OscType::Float(value) if value.abs() > 0.01));
		let intermediate = find_message(&messages, "/VMC/Ext/Bone/Pos", "RightIndexIntermediate");
		let distal = find_message(&messages, "/VMC/Ext/Bone/Pos", "RightIndexDistal");
		assert!(matches!(intermediate.args[6], OscType::Float(value) if value.abs() > 0.01));
		assert!(matches!(distal.args[6], OscType::Float(value) if value.abs() > 0.01));
	}

	#[test]
	fn typed_finger_spread_is_converted_to_vmc_coordinate_space() {
		let mut frame = UNMotionFrame::new(38);
		frame.right_hand = Some(HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			wrist: None,
			fingers: vec![FingerPose {
				finger: Finger::Index,
				joints: vec![transform([0.0, 0.25, 0.0, 0.9682458])],
				confidence: 1.0,
			}],
		});

		let packets = vmc_packets_for_frame_without_ok(&frame);
		let messages = flatten_messages(OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content: packets,
		}));
		let proximal = find_message(&messages, "/VMC/Ext/Bone/Pos", "RightIndexProximal");
		let OscType::Float(y) = proximal.args[5] else {
			panic!("RightIndexProximal rotation y should be float");
		};
		assert!(
			y < -0.20,
			"UNMotion +Y spread must be encoded as VMC -Y before VMC receive conversion: {y}"
		);
	}

	#[test]
	fn sink_maps_typed_hand_motion_to_vmc_finger_bones() {
		let mut frame = UNMotionFrame::new(37);
		frame.right_hand = Some(HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			wrist: None,
			fingers: vec![FingerPose {
				finger: Finger::Index,
				joints: vec![
					TransformSample {
						translation: None,
						rotation: Some(Quatf {
							x: 0.0,
							y: 0.0,
							z: 0.1,
							w: 0.995,
						}),
						scale: None,
						linear_velocity: None,
						angular_velocity: None,
					},
					TransformSample {
						translation: None,
						rotation: Some(Quatf {
							x: 0.0,
							y: 0.0,
							z: 0.2,
							w: 0.98,
						}),
						scale: None,
						linear_velocity: None,
						angular_velocity: None,
					},
					TransformSample {
						translation: None,
						rotation: Some(Quatf {
							x: 0.0,
							y: 0.0,
							z: 0.3,
							w: 0.954,
						}),
						scale: None,
						linear_velocity: None,
						angular_velocity: None,
					},
				],
				confidence: 1.0,
			}],
		});

		let packets = vmc_packets_for_frame_without_ok(&frame);
		let messages = flatten_messages(OscPacket::Bundle(OscBundle {
			timetag: OscTime { seconds: 0, fractional: 1 },
			content: packets,
		}));
		let intermediate = find_message(&messages, "/VMC/Ext/Bone/Pos", "RightIndexIntermediate");
		let OscType::Float(z) = intermediate.args[6] else {
			panic!("RightIndexIntermediate rotation z should be float");
		};
		assert!((z - 0.2).abs() < 1e-4);
	}

	#[test]
	fn curled_index_uses_stronger_distal_factor() {
		assert_eq!(
			adjusted_finger_factor("index", "IndexDistal", BodySide::Right, 0.20, 0.45, 0.50, 0.35),
			0.35
		);
		assert_eq!(
			adjusted_finger_factor("index", "IndexProximal", BodySide::Right, 0.50, 0.45, 0.70, 0.75),
			1.25
		);
		assert_eq!(
			adjusted_finger_factor("index", "IndexIntermediate", BodySide::Left, 0.30, 0.45, 1.00, 1.05),
			1.85
		);
		assert_eq!(
			adjusted_finger_factor("index", "IndexDistal", BodySide::Right, 0.30, 0.45, 0.90, 0.35),
			1.6
		);
		assert_eq!(
			adjusted_finger_factor("index", "IndexDistal", BodySide::Left, 0.30, 0.45, 0.90, 0.35),
			4.6
		);
		assert_eq!(
			adjusted_finger_factor("index", "IndexDistal", BodySide::Left, 0.55, 0.65, 0.90, 0.35),
			0.35
		);
		assert_eq!(
			adjusted_finger_factor("index", "IndexDistal", BodySide::Left, 0.70, 0.65, 0.90, 0.35),
			1.45
		);
		assert_eq!(
			adjusted_finger_factor("index", "IndexDistal", BodySide::Right, 0.70, 0.65, 0.90, 0.35),
			1.0
		);
		assert_eq!(
			adjusted_finger_factor("middle", "MiddleDistal", BodySide::Right, 0.50, 0.45, 0.90, 0.9),
			0.9
		);
	}

	#[test]
	fn finger_spread_uses_export_hand_axes() {
		assert!(finger_spread_angle(0.5, "ThumbProximal", BodySide::Left) > 0.0);
		assert!(finger_spread_angle(0.5, "IndexProximal", BodySide::Left) > 0.0);
		assert!(finger_spread_angle(-0.5, "LittleProximal", BodySide::Left) < 0.0);
		assert!(
			(finger_spread_angle(-0.5, "LittleProximal", BodySide::Left) - finger_spread_angle(0.5, "LittleProximal", BodySide::Right))
				.abs() < 1e-5
		);
	}

	#[test]
	fn thumb_opposition_case_is_narrowly_gated() {
		assert!(is_thumb_opposition_case(0.48, 0.52, BodySide::Left, 0.41));
		assert!(is_thumb_opposition_case(0.42, -0.51, BodySide::Right, 0.41));
		assert!(!is_thumb_opposition_case(0.40, -1.0, BodySide::Left, 0.61));
		assert!(!is_thumb_opposition_case(0.30, -1.0, BodySide::Right, 0.63));
		assert!(!is_thumb_opposition_case(0.18, -0.50, BodySide::Right, 0.42));
	}
}
