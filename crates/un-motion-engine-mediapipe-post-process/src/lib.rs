use un_motion_engine_mediapipe_types::MediaPipeRawOutput;
use un_motion_frame::{
	BodyMotion, BoneSample, CoordinateSpace, ExpressionSample, FaceMotion, Finger, FingerPose, HandMotion, HumanoidBone, HumanoidPose,
	LengthUnit, MotionSignal, MotionSignalValue, MotionSourceInfo, MotionSourceKind, Quatf, SampleState, TimestampBasis, TrackingState,
	TransformSample, UNMotionFrame,
};
use un_motion_interfaces::{FrameProcessor, ImageFrame};
use un_motion_mediapipe_native::{
	FACE_LANDMARK_COUNT, HAND_LANDMARK_COUNT, MAX_HANDS, NativeFace, NativeHand, NativeHands, NativeMediaPipeOutput, NativePose,
};

#[derive(Clone, Debug, PartialEq)]
pub struct MediaPipePostProcessConfig {
	pub head_enabled: bool,
	pub face_enabled: bool,
	pub hands_enabled: bool,
	pub arms_ik_enabled: bool,
	pub torso_enabled: bool,
	pub legs_enabled: bool,
	pub feet_enabled: bool,
	pub include_fingers: bool,
	pub min_landmark_confidence: f32,
	pub input_width: u32,
	pub input_height: u32,
	pub camera_diagonal_view_angle_deg: f32,
	pub eye_open_bias: f32,
	pub mirror_mode: String,
	pub source_id: String,
	pub display_name: String,
	pub rules: MediaPipePostProcessRules,
	pub face_pose_model: Option<FacePoseModelConfig>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FacePoseModelConfig {
	pub enabled: bool,
	pub neutral_nose_drop_eye_mouth: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaPipePostProcessRules {
	pub hold_lost_landmarks: bool,
	pub ease_recovery: bool,
	pub limit_rotation_jumps: bool,
	pub head_source_switch_blend: bool,
	pub lost_signal_behavior: String,
	pub lost_signal_rest_pose_blend: f32,
	pub lost_signal_hold_seconds: f32,
	pub lost_signal_head_behavior: String,
	pub lost_signal_head_rest_pose_blend: f32,
	pub lost_signal_head_hold_seconds: f32,
	pub lost_signal_hands_behavior: String,
	pub lost_signal_hands_rest_pose_blend: f32,
	pub lost_signal_hands_hold_seconds: f32,
	pub lost_signal_arms_behavior: String,
	pub lost_signal_arms_rest_pose_blend: f32,
	pub lost_signal_arms_hold_seconds: f32,
	pub lost_signal_recovery_seconds: f32,
	pub head_from_pose: bool,
	pub head_from_face_matrix: bool,
	pub head_reconcile: bool,
	pub neutral_eye_fallback: bool,
	pub hand_camera_target: bool,
	pub hand_orientation: bool,
	pub finger_derived: bool,
	pub arm_from_pose: bool,
	pub arm_ik_from_hands: bool,
	pub crossed_hand_heuristic: bool,
	pub coordinate_correction: bool,
	pub final_clamp: bool,
}

impl Default for MediaPipePostProcessRules {
	fn default() -> Self {
		Self {
			hold_lost_landmarks: true,
			ease_recovery: true,
			limit_rotation_jumps: true,
			head_source_switch_blend: true,
			lost_signal_behavior: "rest-pose".to_string(),
			lost_signal_rest_pose_blend: 0.3,
			lost_signal_hold_seconds: 8.2,
			lost_signal_head_behavior: "hold".to_string(),
			lost_signal_head_rest_pose_blend: 0.3,
			lost_signal_head_hold_seconds: 8.2,
			lost_signal_hands_behavior: "rest-pose".to_string(),
			lost_signal_hands_rest_pose_blend: 0.3,
			lost_signal_hands_hold_seconds: 8.2,
			lost_signal_arms_behavior: "rest-pose".to_string(),
			lost_signal_arms_rest_pose_blend: 0.3,
			lost_signal_arms_hold_seconds: 8.2,
			lost_signal_recovery_seconds: 0.25,
			head_from_pose: true,
			head_from_face_matrix: true,
			head_reconcile: true,
			neutral_eye_fallback: true,
			hand_camera_target: true,
			hand_orientation: true,
			finger_derived: true,
			arm_from_pose: true,
			arm_ik_from_hands: true,
			crossed_hand_heuristic: true,
			coordinate_correction: true,
			final_clamp: true,
		}
	}
}

impl Default for MediaPipePostProcessConfig {
	fn default() -> Self {
		Self {
			head_enabled: true,
			face_enabled: true,
			hands_enabled: true,
			arms_ik_enabled: true,
			torso_enabled: false,
			legs_enabled: false,
			feet_enabled: false,
			include_fingers: true,
			min_landmark_confidence: 0.55,
			input_width: 1280,
			input_height: 720,
			camera_diagonal_view_angle_deg: 70.0,
			eye_open_bias: 0.5,
			mirror_mode: "normal".to_string(),
			source_id: "webcam:mediapipe-native".to_string(),
			display_name: "MediaPipe Native".to_string(),
			rules: MediaPipePostProcessRules::default(),
			face_pose_model: None,
		}
	}
}

#[derive(Clone, Debug, Default)]
pub struct MediaPipePostProcessor {
	pub config: MediaPipePostProcessConfig,
	stabilizer: MotionStabilizer,
}

impl MediaPipePostProcessor {
	pub fn new(config: MediaPipePostProcessConfig) -> Self {
		Self {
			config,
			stabilizer: MotionStabilizer::default(),
		}
	}

	pub fn process_native_output(&mut self, input: &ImageFrame, native: &NativeMediaPipeOutput) -> UNMotionFrame {
		self.process_native_output_with_sequence(input.metadata.sequence, input.metadata.capture_timestamp_ns, native)
	}

	pub fn process_native_output_with_sequence(
		&mut self,
		output_sequence: u64,
		capture_timestamp_ns: u64,
		native: &NativeMediaPipeOutput,
	) -> UNMotionFrame {
		let observation_quality = mediapipe_observation_qualities(native, &self.config);
		let mut signals = native_mediapipe_output_signals(native, &self.config);
		signals = apply_tracking_transforms(signals, &self.config);
		let mut frame = UNMotionFrame::new(output_sequence);
		let now = now_unix_ns();
		frame.header.timestamp_basis = TimestampBasis::SourceLocal;
		frame.header.capture_timestamp_ns = capture_timestamp_ns;
		frame.header.frame_timestamp_ns = capture_timestamp_ns;
		frame.header.processed_timestamp_ns = now;
		frame.header.coordinate_space = CoordinateSpace::UNMotion;
		frame.header.length_unit = LengthUnit::Normalized;
		frame.header.stream_id = Some(self.config.source_id.clone());
		let confidence = average_confidence(&signals);
		frame.sources.push(MotionSourceInfo {
			source_id: self.config.source_id.clone(),
			source_kind: MotionSourceKind::WebcamPose,
			display_name: Some(self.config.display_name.clone()),
			confidence,
			latency_ns: Some(now.saturating_sub(capture_timestamp_ns)),
			state: if signals.is_empty() {
				TrackingState::Lost
			} else {
				TrackingState::Valid
			},
		});
		if self.config.rules.final_clamp {
			for signal in &mut signals {
				if let MotionSignalValue::Scalar(value) = signal.value {
					signal.value = MotionSignalValue::Scalar(value.clamp(-1.0, 1.0));
				}
				signal.confidence = signal.confidence.clamp(0.0, 1.0);
			}
		}
		frame.signals = signals;
		frame.body = signal_body_motion_from_signals(&frame.signals);
		frame.face = signal_face_motion_from_signals(&frame.signals);
		let (left_hand, right_hand) = native_hand_motions(native, &self.config);
		frame.left_hand = left_hand;
		frame.right_hand = right_hand;
		self.stabilizer.apply(&mut frame, &observation_quality, &self.config.rules);
		frame.metadata.notes.push(format!(
			"mediapipe.post_process native_return_code={} pose={} hands={} face={} holistic_pose={} holistic_left_hand={} holistic_right_hand={} holistic_face={}",
			native.return_code,
			native.pose.landmark_count,
			native.hands.hand_count,
			native.face.landmark_count,
			native.holistic.pose.landmark_count,
			native.holistic.left_hand.landmark_count,
			native.holistic.right_hand.landmark_count,
			native.holistic.face.landmark_count
		));
		frame.metadata.notes.push(format!(
			"mediapipe.post_process rules={}",
			post_process_rules_summary(&self.config.rules)
		));
		frame.metadata.notes.push(observation_quality.summary_note());
		let (_, _, primary_face) = primary_native_parts(native);
		if let Some(metrics) = face_landmark_metrics(primary_face) {
			frame.metadata.notes.push(format!(
				"mediapipe.face_metrics noseDropEyeMouth={:.6} yaw={:.6} roll={:.6} confidence={:.6}",
				metrics.nose_drop_eye_mouth, metrics.yaw, metrics.roll, metrics.confidence
			));
		}
		frame
	}

	pub fn native_raw_passthrough_frame(
		&self,
		output_sequence: u64,
		capture_timestamp_ns: u64,
		native: &NativeMediaPipeOutput,
	) -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(output_sequence);
		frame.header.timestamp_basis = TimestampBasis::SourceLocal;
		frame.header.capture_timestamp_ns = capture_timestamp_ns;
		frame.header.frame_timestamp_ns = capture_timestamp_ns;
		frame.header.processed_timestamp_ns = now_unix_ns();
		frame.header.coordinate_space = CoordinateSpace::UNMotion;
		frame.header.length_unit = LengthUnit::Normalized;
		frame.header.stream_id = Some(format!("{}:raw", self.config.source_id));
		frame.sources.push(MotionSourceInfo {
			source_id: format!("{}:raw", self.config.source_id),
			source_kind: MotionSourceKind::WebcamPose,
			display_name: Some(format!("{} Raw", self.config.display_name)),
			confidence: 0.0,
			latency_ns: None,
			state: TrackingState::Lost,
		});
		frame.metadata.notes.push(format!(
			"post_process=none native_return_code={} pose={} hands={} face={}",
			native.return_code, native.pose.landmark_count, native.hands.hand_count, native.face.landmark_count
		));
		frame
	}
}

#[cfg(test)]
fn fixed_name_bytes(value: &str) -> [u8; un_motion_mediapipe_native::BLENDSHAPE_NAME_BYTES] {
	let mut bytes = [0_u8; un_motion_mediapipe_native::BLENDSHAPE_NAME_BYTES];
	let src = value.as_bytes();
	let len = src.len().min(un_motion_mediapipe_native::BLENDSHAPE_NAME_BYTES.saturating_sub(1));
	bytes[..len].copy_from_slice(&src[..len]);
	bytes
}

impl FrameProcessor<(&ImageFrame, &NativeMediaPipeOutput), UNMotionFrame> for MediaPipePostProcessor {
	fn process(&mut self, input: (&ImageFrame, &NativeMediaPipeOutput)) -> anyhow::Result<UNMotionFrame> {
		Ok(self.process_native_output(input.0, input.1))
	}
}

impl FrameProcessor<(&ImageFrame, &MediaPipeRawOutput), UNMotionFrame> for MediaPipePostProcessor {
	fn process(&mut self, input: (&ImageFrame, &MediaPipeRawOutput)) -> anyhow::Result<UNMotionFrame> {
		match input.1 {
			MediaPipeRawOutput::Native(native) => Ok(self.process_native_output(input.0, native)),
			MediaPipeRawOutput::Empty => Ok(self.native_raw_passthrough_frame(
				input.0.metadata.sequence,
				input.0.metadata.capture_timestamp_ns,
				&NativeMediaPipeOutput::default(),
			)),
		}
	}
}

pub fn native_mediapipe_output_signals(native: &NativeMediaPipeOutput, config: &MediaPipePostProcessConfig) -> Vec<MotionSignal> {
	if native.holistic.pose.landmark_count > 0
		|| native.holistic.left_hand.landmark_count > 0
		|| native.holistic.right_hand.landmark_count > 0
		|| native.holistic.face.landmark_count > 0
		|| native.holistic.face.blendshape_count > 0
	{
		let mut hands = NativeHands::default();
		for hand in [native.holistic.left_hand, native.holistic.right_hand] {
			if hand.landmark_count > 0 && (hands.hand_count as usize) < hands.hands.len() {
				hands.hands[hands.hand_count as usize] = hand;
				hands.hand_count += 1;
			}
		}
		return native_mediapipe_signals(&native.holistic.pose, &hands, &native.holistic.face, config);
	}
	native_mediapipe_signals(&native.pose, &native.hands, &native.face, config)
}

fn primary_native_parts(native: &NativeMediaPipeOutput) -> (&NativePose, NativeHands, &NativeFace) {
	if native.holistic.pose.landmark_count > 0
		|| native.holistic.left_hand.landmark_count > 0
		|| native.holistic.right_hand.landmark_count > 0
		|| native.holistic.face.landmark_count > 0
		|| native.holistic.face.blendshape_count > 0
	{
		let mut hands = NativeHands::default();
		for hand in [native.holistic.left_hand, native.holistic.right_hand] {
			if hand.landmark_count > 0 && (hands.hand_count as usize) < hands.hands.len() {
				hands.hands[hands.hand_count as usize] = hand;
				hands.hand_count += 1;
			}
		}
		return (&native.holistic.pose, hands, &native.holistic.face);
	}
	(&native.pose, native.hands, &native.face)
}

fn mediapipe_observation_qualities(native: &NativeMediaPipeOutput, config: &MediaPipePostProcessConfig) -> ObservationSet {
	let (pose, hands, face) = primary_native_parts(native);
	let left_hand = hand_observation_quality("left", &hands, config.min_landmark_confidence);
	let right_hand = hand_observation_quality("right", &hands, config.min_landmark_confidence);
	ObservationSet {
		head: head_observation_quality(pose, face, config.min_landmark_confidence),
		arms: arms_observation_quality(
			pose,
			config.min_landmark_confidence,
			config.hands_enabled,
			config.arms_ik_enabled && config.rules.arm_ik_from_hands,
			&left_hand,
			&right_hand,
		),
		left_hand,
		right_hand,
	}
}

#[derive(Clone, Debug, PartialEq)]
struct ObservationSet {
	head: ObservationQuality,
	arms: ObservationQuality,
	left_hand: ObservationQuality,
	right_hand: ObservationQuality,
}

impl ObservationSet {
	fn summary_note(&self) -> String {
		format!(
			"mediapipe.quality head={:.3}({}) arms={:.3}({}) left_hand={:.3}({}) right_hand={:.3}({})",
			self.head.score,
			self.head.reason,
			self.arms.score,
			self.arms.reason,
			self.left_hand.score,
			self.left_hand.reason,
			self.right_hand.score,
			self.right_hand.reason
		)
	}
}

#[derive(Clone, Debug, PartialEq)]
struct ObservationQuality {
	score: f32,
	reason: &'static str,
}

impl ObservationQuality {
	fn new(score: f32, reason: &'static str) -> Self {
		Self {
			score: score.clamp(0.0, 1.0),
			reason,
		}
	}

	fn is_tracked(&self) -> bool {
		self.score >= 0.55
	}
}

#[derive(Clone, Debug, Default)]
struct MotionStabilizer {
	head: Option<TimedHeadSnapshot>,
	head_source_switch: Option<HeadSourceSwitch>,
	left_arm: Option<TimedArmSnapshot>,
	right_arm: Option<TimedArmSnapshot>,
	left_hand: Option<TimedHandSnapshot>,
	right_hand: Option<TimedHandSnapshot>,
	last_head_hold_ns: Option<u64>,
	last_left_arm_hold_ns: Option<u64>,
	last_right_arm_hold_ns: Option<u64>,
	last_left_hand_hold_ns: Option<u64>,
	last_right_hand_hold_ns: Option<u64>,
}

#[derive(Clone, Debug)]
struct TimedHeadSnapshot {
	timestamp_ns: u64,
	quality_reason: &'static str,
	bone: Option<BoneSample>,
	face_head: Option<TransformSample>,
}

#[derive(Clone, Debug)]
struct HeadSourceSwitch {
	from: TimedHeadSnapshot,
	to_reason: &'static str,
}

#[derive(Clone, Debug)]
struct TimedHandSnapshot {
	timestamp_ns: u64,
	motion: HandMotion,
}

#[derive(Clone, Debug)]
struct TimedArmSnapshot {
	timestamp_ns: u64,
	upper: Option<BoneSample>,
	lower: Option<BoneSample>,
}

impl MotionStabilizer {
	const SHORT_OCCLUSION_NS: u64 = 250_000_000;
	const HEAD_SOURCE_SWITCH_BLEND_NS: u64 = 120_000_000;
	const HEAD_MAX_ANGULAR_SPEED_RAD_PER_SEC: f32 = 12.0;
	const ARM_MAX_ANGULAR_SPEED_RAD_PER_SEC: f32 = 18.0;
	const ROTATION_JUMP_SLACK_RAD: f32 = 0.10;

	fn apply(&mut self, frame: &mut UNMotionFrame, quality: &ObservationSet, rules: &MediaPipePostProcessRules) {
		let timestamp_ns = frame.header.capture_timestamp_ns;
		self.apply_head(frame, &quality.head, timestamp_ns, rules);
		self.apply_arm(frame, &quality.arms, timestamp_ns, HandSide::Left, rules);
		self.apply_arm(frame, &quality.arms, timestamp_ns, HandSide::Right, rules);
		self.apply_hand(&mut frame.left_hand, &quality.left_hand, timestamp_ns, HandSide::Left, rules);
		self.apply_hand(&mut frame.right_hand, &quality.right_hand, timestamp_ns, HandSide::Right, rules);
	}

	fn apply_head(
		&mut self,
		frame: &mut UNMotionFrame,
		quality: &ObservationQuality,
		timestamp_ns: u64,
		rules: &MediaPipePostProcessRules,
	) {
		if quality.is_tracked() {
			if rules.ease_recovery
				&& let Some(alpha) = recovery_alpha(self.last_head_hold_ns, timestamp_ns, lost_signal_recovery_ns(rules))
			{
				if let Some(snapshot) = self.head.as_ref() {
					blend_head_snapshot_into_frame(snapshot, frame, alpha);
					frame.metadata.notes.push("mediapipe.stability head=recovering".to_string());
				}
			} else {
				self.last_head_hold_ns = None;
			}
			if rules.limit_rotation_jumps
				&& let Some(snapshot) = self.head.as_ref()
				&& limit_body_bone_rotation_delta(
					snapshot.bone.as_ref(),
					body_bone(frame, HumanoidBone::Head),
					timestamp_ns.saturating_sub(snapshot.timestamp_ns),
					Self::HEAD_MAX_ANGULAR_SPEED_RAD_PER_SEC,
					Self::ROTATION_JUMP_SLACK_RAD,
				) {
				if let Some(limited) = blend_body_bone(
					snapshot.bone.as_ref(),
					body_bone(frame, HumanoidBone::Head),
					rotation_delta_limit_alpha(
						snapshot.bone.as_ref(),
						body_bone(frame, HumanoidBone::Head),
						timestamp_ns.saturating_sub(snapshot.timestamp_ns),
						Self::HEAD_MAX_ANGULAR_SPEED_RAD_PER_SEC,
						Self::ROTATION_JUMP_SLACK_RAD,
					)
					.unwrap_or(1.0),
				) {
					upsert_body_bone(frame, limited);
					frame.metadata.notes.push("mediapipe.stability head=rotation_limited".to_string());
				}
			}
			if rules.head_source_switch_blend {
				self.apply_head_source_switch_blend(frame, quality, timestamp_ns);
			} else {
				self.head_source_switch = None;
			}
			self.head = Some(TimedHeadSnapshot {
				timestamp_ns,
				quality_reason: quality.reason,
				bone: body_bone(frame, HumanoidBone::Head).cloned(),
				face_head: frame.face.as_ref().and_then(|face| face.head.clone()),
			});
			return;
		}
		self.head_source_switch = None;
		match lost_signal_behavior(lost_signal_part_behavior(rules, LostSignalPart::Head)) {
			LostSignalBehavior::RestPose => {
				let previous = self.head.as_ref();
				let head = Self::rest_pose_arm_bone(
					previous.and_then(|snapshot| snapshot.bone.as_ref()),
					HumanoidBone::Head,
					IDENTITY_QUAT_ARRAY,
					quality.score,
				);
				upsert_body_bone(frame, head.clone());
				self.head = Some(TimedHeadSnapshot {
					timestamp_ns,
					quality_reason: quality.reason,
					bone: Some(head),
					face_head: None,
				});
				self.last_head_hold_ns = None;
				frame.metadata.notes.push("mediapipe.stability head=rest_pose_lost".to_string());
			}
			LostSignalBehavior::Hold => {
				if !rules.hold_lost_landmarks {
					return;
				}
				let Some(snapshot) = self.head.as_ref().filter(|snapshot| {
					timestamp_ns.saturating_sub(snapshot.timestamp_ns) <= lost_signal_part_hold_ns(rules, LostSignalPart::Head)
				}) else {
					return;
				};
				if let Some(mut bone) = snapshot.bone.clone() {
					bone.state = SampleState::Held;
					bone.confidence = bone.confidence.min(quality.score.max(0.25));
					upsert_body_bone(frame, bone);
				}
				if let Some(head) = snapshot.face_head.clone() {
					let face = frame.face.get_or_insert_with(|| FaceMotion {
						tracking_state: TrackingState::Recovering,
						confidence: quality.score.max(0.25),
						head: None,
						expressions: Vec::new(),
					});
					face.tracking_state = TrackingState::Recovering;
					face.confidence = face.confidence.max(quality.score.max(0.25));
					face.head = Some(head);
				}
				self.last_head_hold_ns = Some(timestamp_ns);
				frame.metadata.notes.push("mediapipe.stability head=held".to_string());
			}
			LostSignalBehavior::Drop => {
				self.head = None;
				self.last_head_hold_ns = None;
				frame.metadata.notes.push("mediapipe.stability head=drop_lost".to_string());
			}
		}
	}

	fn apply_head_source_switch_blend(&mut self, frame: &mut UNMotionFrame, quality: &ObservationQuality, timestamp_ns: u64) {
		let Some(previous) = self.head.as_ref() else {
			self.head_source_switch = None;
			return;
		};
		if previous.quality_reason != quality.reason
			&& timestamp_ns.saturating_sub(previous.timestamp_ns) <= Self::SHORT_OCCLUSION_NS
			&& (body_bone(frame, HumanoidBone::Head).is_some() || frame.face.as_ref().and_then(|face| face.head.as_ref()).is_some())
		{
			self.head_source_switch = Some(HeadSourceSwitch {
				from: previous.clone(),
				to_reason: quality.reason,
			});
		}
		let Some(source_switch) = self.head_source_switch.clone() else {
			return;
		};
		if source_switch.to_reason != quality.reason {
			self.head_source_switch = None;
			return;
		}
		let elapsed = timestamp_ns.saturating_sub(source_switch.from.timestamp_ns);
		if elapsed > Self::HEAD_SOURCE_SWITCH_BLEND_NS {
			self.head_source_switch = None;
			return;
		}
		let alpha = smoothstep((elapsed as f32 / Self::HEAD_SOURCE_SWITCH_BLEND_NS as f32).clamp(0.0, 1.0));
		blend_head_snapshot_into_frame(&source_switch.from, frame, alpha);
		frame.metadata.notes.push(format!(
			"mediapipe.stability head=source_switch from={} to={}",
			source_switch.from.quality_reason, quality.reason
		));
	}

	fn apply_arm(
		&mut self,
		frame: &mut UNMotionFrame,
		quality: &ObservationQuality,
		timestamp_ns: u64,
		side: HandSide,
		rules: &MediaPipePostProcessRules,
	) {
		if quality.is_tracked() {
			let held_at = match side {
				HandSide::Left => self.last_left_arm_hold_ns,
				HandSide::Right => self.last_right_arm_hold_ns,
			};
			if rules.ease_recovery
				&& let Some(alpha) = recovery_alpha(held_at, timestamp_ns, lost_signal_recovery_ns(rules))
			{
				let snapshot = match side {
					HandSide::Left => self.left_arm.as_ref(),
					HandSide::Right => self.right_arm.as_ref(),
				};
				if let Some(snapshot) = snapshot {
					if let Some(blended) = blend_body_bone(snapshot.upper.as_ref(), body_bone(frame, side.upper_arm_bone()), alpha) {
						upsert_body_bone(frame, blended);
					}
					if let Some(blended) = blend_body_bone(snapshot.lower.as_ref(), body_bone(frame, side.lower_arm_bone()), alpha) {
						upsert_body_bone(frame, blended);
					}
					frame
						.metadata
						.notes
						.push(format!("mediapipe.stability {}_arm=recovering", side.prefix()));
				}
			} else {
				match side {
					HandSide::Left => self.last_left_arm_hold_ns = None,
					HandSide::Right => self.last_right_arm_hold_ns = None,
				}
			}
			let snapshot = match side {
				HandSide::Left => self.left_arm.as_ref(),
				HandSide::Right => self.right_arm.as_ref(),
			};
			if rules.limit_rotation_jumps
				&& let Some(snapshot) = snapshot
			{
				let elapsed_ns = timestamp_ns.saturating_sub(snapshot.timestamp_ns);
				let mut limited = false;
				if let Some(bone) = limit_body_bone_rotation_delta_to_sample(
					snapshot.upper.as_ref(),
					body_bone(frame, side.upper_arm_bone()),
					elapsed_ns,
					Self::ARM_MAX_ANGULAR_SPEED_RAD_PER_SEC,
					Self::ROTATION_JUMP_SLACK_RAD,
				) {
					upsert_body_bone(frame, bone);
					limited = true;
				}
				if let Some(bone) = limit_body_bone_rotation_delta_to_sample(
					snapshot.lower.as_ref(),
					body_bone(frame, side.lower_arm_bone()),
					elapsed_ns,
					Self::ARM_MAX_ANGULAR_SPEED_RAD_PER_SEC,
					Self::ROTATION_JUMP_SLACK_RAD,
				) {
					upsert_body_bone(frame, bone);
					limited = true;
				}
				if limited {
					frame
						.metadata
						.notes
						.push(format!("mediapipe.stability {}_arm=rotation_limited", side.prefix()));
				}
			}
			let snapshot = TimedArmSnapshot {
				timestamp_ns,
				upper: body_bone(frame, side.upper_arm_bone()).cloned(),
				lower: body_bone(frame, side.lower_arm_bone()).cloned(),
			};
			match side {
				HandSide::Left => self.left_arm = Some(snapshot),
				HandSide::Right => self.right_arm = Some(snapshot),
			}
			return;
		}
		if quality.reason == "pose_chain_hands_missing" {
			match lost_signal_behavior(lost_signal_part_behavior(rules, LostSignalPart::Arms)) {
				LostSignalBehavior::RestPose => {
					let previous = match side {
						HandSide::Left => self.left_arm.as_ref(),
						HandSide::Right => self.right_arm.as_ref(),
					};
					let upper = Self::rest_pose_arm_bone(
						previous.and_then(|snapshot| snapshot.upper.as_ref()),
						side.upper_arm_bone(),
						rest_pose_upper_arm_rotation(side, lost_signal_part_rest_pose_blend(rules, LostSignalPart::Arms)),
						quality.score,
					);
					let lower = Self::rest_pose_arm_bone(
						previous.and_then(|snapshot| snapshot.lower.as_ref()),
						side.lower_arm_bone(),
						IDENTITY_QUAT_ARRAY,
						quality.score,
					);
					let hand = Self::rest_pose_arm_bone(None, side.hand_bone(), IDENTITY_QUAT_ARRAY, quality.score);
					upsert_body_bone(frame, upper.clone());
					upsert_body_bone(frame, lower.clone());
					upsert_body_bone(frame, hand);
					let rest_pose_snapshot = TimedArmSnapshot {
						timestamp_ns,
						upper: Some(upper),
						lower: Some(lower),
					};
					match side {
						HandSide::Left => {
							self.left_arm = Some(rest_pose_snapshot);
							self.last_left_arm_hold_ns = None;
						}
						HandSide::Right => {
							self.right_arm = Some(rest_pose_snapshot);
							self.last_right_arm_hold_ns = None;
						}
					}
					frame
						.metadata
						.notes
						.push(format!("mediapipe.stability {}_arm=rest_pose_hands_missing", side.prefix()));
				}
				LostSignalBehavior::Hold => {
					let snapshot = match side {
						HandSide::Left => self.left_arm.as_ref(),
						HandSide::Right => self.right_arm.as_ref(),
					}
					.filter(|snapshot| {
						timestamp_ns.saturating_sub(snapshot.timestamp_ns) <= lost_signal_part_hold_ns(rules, LostSignalPart::Arms)
					});
					let Some(snapshot) = snapshot else {
						return;
					};
					for bone in [snapshot.upper.clone(), snapshot.lower.clone()].into_iter().flatten() {
						let mut bone = bone;
						bone.state = SampleState::Held;
						bone.confidence = bone.confidence.min(quality.score.max(0.25));
						upsert_body_bone(frame, bone);
					}
					match side {
						HandSide::Left => self.last_left_arm_hold_ns = Some(timestamp_ns),
						HandSide::Right => self.last_right_arm_hold_ns = Some(timestamp_ns),
					}
					frame
						.metadata
						.notes
						.push(format!("mediapipe.stability {}_arm=hold_hands_missing", side.prefix()));
				}
				LostSignalBehavior::Drop => {
					match side {
						HandSide::Left => {
							self.left_arm = None;
							self.last_left_arm_hold_ns = None;
						}
						HandSide::Right => {
							self.right_arm = None;
							self.last_right_arm_hold_ns = None;
						}
					}
					frame
						.metadata
						.notes
						.push(format!("mediapipe.stability {}_arm=drop_hands_missing", side.prefix()));
				}
			}
			return;
		}
		if !rules.hold_lost_landmarks {
			return;
		}
		let snapshot = match side {
			HandSide::Left => self.left_arm.as_ref(),
			HandSide::Right => self.right_arm.as_ref(),
		}
		.filter(|snapshot| timestamp_ns.saturating_sub(snapshot.timestamp_ns) <= Self::SHORT_OCCLUSION_NS);
		let Some(snapshot) = snapshot else {
			return;
		};
		for bone in [snapshot.upper.clone(), snapshot.lower.clone()].into_iter().flatten() {
			let mut bone = bone;
			bone.state = SampleState::Held;
			bone.confidence = bone.confidence.min(quality.score.max(0.25));
			upsert_body_bone(frame, bone);
		}
		match side {
			HandSide::Left => self.last_left_arm_hold_ns = Some(timestamp_ns),
			HandSide::Right => self.last_right_arm_hold_ns = Some(timestamp_ns),
		}
		frame.metadata.notes.push(format!("mediapipe.stability {}_arm=held", side.prefix()));
	}

	fn rest_pose_arm_bone(previous: Option<&BoneSample>, bone: HumanoidBone, target_rotation: [f32; 4], confidence: f32) -> BoneSample {
		let mut rest_pose = body_bone_sample_with_confidence(bone, target_rotation, confidence.max(0.2));
		rest_pose.state = SampleState::Valid;
		let Some(previous) = previous else {
			return rest_pose;
		};
		let mut blended = blend_body_bone(Some(previous), Some(&rest_pose), 0.18).unwrap_or(rest_pose);
		blended.confidence = confidence.max(0.2);
		blended.state = SampleState::Valid;
		blended
	}

	fn apply_hand(
		&mut self,
		hand: &mut Option<HandMotion>,
		quality: &ObservationQuality,
		timestamp_ns: u64,
		side: HandSide,
		rules: &MediaPipePostProcessRules,
	) {
		if quality.is_tracked() {
			let held_at = match side {
				HandSide::Left => self.last_left_hand_hold_ns,
				HandSide::Right => self.last_right_hand_hold_ns,
			};
			if rules.ease_recovery
				&& let Some(alpha) = recovery_alpha(held_at, timestamp_ns, lost_signal_recovery_ns(rules))
			{
				let snapshot = match side {
					HandSide::Left => self.left_hand.as_ref(),
					HandSide::Right => self.right_hand.as_ref(),
				};
				if let (Some(snapshot), Some(current)) = (snapshot, hand.as_ref()) {
					*hand = Some(blend_hand_motion(&snapshot.motion, current, alpha));
				}
			} else {
				match side {
					HandSide::Left => self.last_left_hand_hold_ns = None,
					HandSide::Right => self.last_right_hand_hold_ns = None,
				}
			}
			if let Some(motion) = hand.clone() {
				match side {
					HandSide::Left => self.left_hand = Some(TimedHandSnapshot { timestamp_ns, motion }),
					HandSide::Right => self.right_hand = Some(TimedHandSnapshot { timestamp_ns, motion }),
				}
			}
			return;
		}
		match lost_signal_behavior(lost_signal_part_behavior(rules, LostSignalPart::Hands)) {
			LostSignalBehavior::Drop => match side {
				HandSide::Left => {
					self.left_hand = None;
					self.last_left_hand_hold_ns = None;
				}
				HandSide::Right => {
					self.right_hand = None;
					self.last_right_hand_hold_ns = None;
				}
			},
			LostSignalBehavior::RestPose | LostSignalBehavior::Hold => {
				if !rules.hold_lost_landmarks {
					return;
				}
				let snapshot = match side {
					HandSide::Left => self.left_hand.as_ref(),
					HandSide::Right => self.right_hand.as_ref(),
				}
				.filter(|snapshot| {
					timestamp_ns.saturating_sub(snapshot.timestamp_ns) <= lost_signal_part_hold_ns(rules, LostSignalPart::Hands)
				});
				let Some(snapshot) = snapshot else {
					return;
				};
				let mut motion = snapshot.motion.clone();
				motion.tracking_state = TrackingState::Recovering;
				motion.confidence = motion.confidence.min(quality.score.max(0.25));
				for finger in &mut motion.fingers {
					finger.confidence = finger.confidence.min(motion.confidence);
				}
				match side {
					HandSide::Left => self.last_left_hand_hold_ns = Some(timestamp_ns),
					HandSide::Right => self.last_right_hand_hold_ns = Some(timestamp_ns),
				}
				*hand = Some(motion);
			}
		}
	}
}

fn recovery_alpha(held_at_ns: Option<u64>, timestamp_ns: u64, recovery_window_ns: u64) -> Option<f32> {
	let held_at_ns = held_at_ns?;
	let elapsed = timestamp_ns.saturating_sub(held_at_ns);
	if elapsed > recovery_window_ns {
		return None;
	}
	Some((elapsed as f32 / recovery_window_ns as f32).clamp(0.0, 1.0))
}

fn blend_body_bone(previous: Option<&BoneSample>, current: Option<&BoneSample>, alpha: f32) -> Option<BoneSample> {
	let mut blended = current?.clone();
	let previous_rotation = previous?.transform.rotation.as_ref()?;
	let current_rotation = blended.transform.rotation.as_ref()?;
	blended.transform.rotation = Some(quatf_nlerp(previous_rotation, current_rotation, alpha));
	blended.confidence = blended.confidence.max(previous?.confidence);
	Some(blended)
}

fn limit_body_bone_rotation_delta(
	previous: Option<&BoneSample>,
	current: Option<&BoneSample>,
	elapsed_ns: u64,
	max_rad_per_sec: f32,
	slack_rad: f32,
) -> bool {
	rotation_delta_limit_alpha(previous, current, elapsed_ns, max_rad_per_sec, slack_rad).is_some()
}

fn limit_body_bone_rotation_delta_to_sample(
	previous: Option<&BoneSample>,
	current: Option<&BoneSample>,
	elapsed_ns: u64,
	max_rad_per_sec: f32,
	slack_rad: f32,
) -> Option<BoneSample> {
	let alpha = rotation_delta_limit_alpha(previous, current, elapsed_ns, max_rad_per_sec, slack_rad)?;
	blend_body_bone(previous, current, alpha)
}

fn rotation_delta_limit_alpha(
	previous: Option<&BoneSample>,
	current: Option<&BoneSample>,
	elapsed_ns: u64,
	max_rad_per_sec: f32,
	slack_rad: f32,
) -> Option<f32> {
	let previous_rotation = previous?.transform.rotation.as_ref()?;
	let current_rotation = current?.transform.rotation.as_ref()?;
	let angle = quatf_angle_rad(previous_rotation, current_rotation);
	if !angle.is_finite() || angle <= 1e-5 {
		return None;
	}
	let elapsed_sec = (elapsed_ns as f32 / 1_000_000_000.0).clamp(0.001, 0.25);
	let max_angle = (max_rad_per_sec * elapsed_sec) + slack_rad.max(0.0);
	if angle <= max_angle {
		return None;
	}
	Some((max_angle / angle).clamp(0.0, 1.0))
}

fn blend_head_snapshot_into_frame(snapshot: &TimedHeadSnapshot, frame: &mut UNMotionFrame, alpha: f32) {
	if let Some(blended) = blend_body_bone(snapshot.bone.as_ref(), body_bone(frame, HumanoidBone::Head), alpha) {
		upsert_body_bone(frame, blended);
	}
	if let Some(blended) = blend_transform_sample(
		snapshot.face_head.as_ref(),
		frame.face.as_ref().and_then(|face| face.head.as_ref()),
		alpha,
	) && let Some(face) = frame.face.as_mut()
	{
		face.head = Some(blended);
	}
}

fn smoothstep(t: f32) -> f32 {
	let t = t.clamp(0.0, 1.0);
	t * t * (3.0 - 2.0 * t)
}

fn blend_transform_sample(previous: Option<&TransformSample>, current: Option<&TransformSample>, alpha: f32) -> Option<TransformSample> {
	let mut blended = current?.clone();
	if let (Some(previous_rotation), Some(current_rotation)) = (previous?.rotation.as_ref(), blended.rotation.as_ref()) {
		blended.rotation = Some(quatf_nlerp(previous_rotation, current_rotation, alpha));
	}
	Some(blended)
}

fn blend_hand_motion(previous: &HandMotion, current: &HandMotion, alpha: f32) -> HandMotion {
	let mut blended = current.clone();
	if let Some(wrist) = blend_transform_sample(previous.wrist.as_ref(), current.wrist.as_ref(), alpha) {
		blended.wrist = Some(wrist);
	}
	for finger in &mut blended.fingers {
		if let Some(previous_finger) = previous.fingers.iter().find(|candidate| candidate.finger == finger.finger) {
			for (joint, previous_joint) in finger.joints.iter_mut().zip(previous_finger.joints.iter()) {
				if let Some(blended_joint) = blend_transform_sample(Some(previous_joint), Some(joint), alpha) {
					*joint = blended_joint;
				}
			}
			finger.confidence = finger.confidence.max(previous_finger.confidence);
		}
	}
	blended
}

fn head_observation_quality(pose: &NativePose, face: &NativeFace, min_confidence: f32) -> ObservationQuality {
	if face.matrix_rows >= 3 && face.matrix_cols >= 3 {
		return ObservationQuality::new(face.confidence.max(0.75), "face_matrix");
	}
	if face.landmark_count >= FACE_LANDMARK_COUNT as u32 {
		return ObservationQuality::new(face.confidence.max(0.75), "face_landmarks");
	}
	if pose.world_landmark_count >= 13 {
		let confidence = landmark_confidence([
			pose.landmarks[0],
			pose.landmarks[2],
			pose.landmarks[5],
			pose.landmarks[7],
			pose.landmarks[8],
			pose.landmarks[11],
			pose.landmarks[12],
		]);
		if confidence >= min_confidence {
			return ObservationQuality::new(confidence * 0.70, "pose_world");
		}
		return ObservationQuality::new(confidence * 0.35, "pose_world_low_confidence");
	}
	if pose.landmark_count >= 9 {
		let confidence = landmark_confidence([
			pose.landmarks[0],
			pose.landmarks[2],
			pose.landmarks[5],
			pose.landmarks[7],
			pose.landmarks[8],
		]);
		if confidence >= min_confidence {
			return ObservationQuality::new(confidence * 0.45, "pose_2d");
		}
		return ObservationQuality::new(confidence * 0.25, "pose_2d_low_confidence");
	}
	ObservationQuality::new(0.0, "missing")
}

fn arms_observation_quality(
	pose: &NativePose,
	min_confidence: f32,
	hands_enabled: bool,
	hand_ik_enabled: bool,
	left_hand: &ObservationQuality,
	right_hand: &ObservationQuality,
) -> ObservationQuality {
	if pose.world_landmark_count < 17 && pose.landmark_count < 17 {
		return hand_ik_arm_observation_quality(hand_ik_enabled, left_hand, right_hand);
	}
	let confidence = landmark_confidence([
		pose.landmarks[11],
		pose.landmarks[12],
		pose.landmarks[13],
		pose.landmarks[14],
		pose.landmarks[15],
		pose.landmarks[16],
	]);
	let points = if pose.world_landmark_count >= 17 {
		[
			head_world_point(pose, 11),
			head_world_point(pose, 12),
			head_world_point(pose, 13),
			head_world_point(pose, 14),
			head_world_point(pose, 15),
			head_world_point(pose, 16),
		]
	} else {
		[
			pose_point(pose, 11),
			pose_point(pose, 12),
			pose_point(pose, 13),
			pose_point(pose, 14),
			pose_point(pose, 15),
			pose_point(pose, 16),
		]
	};
	let lengths = [
		distance3d(points[0], points[2]),
		distance3d(points[2], points[4]),
		distance3d(points[1], points[3]),
		distance3d(points[3], points[5]),
	];
	if lengths.iter().any(|length| !length.is_finite() || *length <= 1e-5) {
		let hand_ik = hand_ik_arm_observation_quality(hand_ik_enabled, left_hand, right_hand);
		if hand_ik.is_tracked() {
			return hand_ik;
		}
		return ObservationQuality::new(confidence * 0.2, "degenerate");
	}
	if confidence < min_confidence {
		let hand_ik = hand_ik_arm_observation_quality(hand_ik_enabled, left_hand, right_hand);
		if hand_ik.is_tracked() {
			return hand_ik;
		}
		return ObservationQuality::new(confidence * 0.45, "low_confidence");
	}
	if hands_enabled && left_hand.score <= 0.0 && right_hand.score <= 0.0 {
		return ObservationQuality::new(confidence * 0.35, "pose_chain_hands_missing");
	}
	let left_ratio = lengths[0].min(lengths[1]) / lengths[0].max(lengths[1]);
	let right_ratio = lengths[2].min(lengths[3]) / lengths[2].max(lengths[3]);
	let symmetry = left_ratio.min(right_ratio).clamp(0.0, 1.0);
	ObservationQuality::new(confidence * (0.55 + 0.45 * symmetry), "pose_chain")
}

fn hand_ik_arm_observation_quality(
	hand_ik_enabled: bool,
	left_hand: &ObservationQuality,
	right_hand: &ObservationQuality,
) -> ObservationQuality {
	if !hand_ik_enabled {
		return ObservationQuality::new(0.0, "missing");
	}
	let hand_score = left_hand.score.max(right_hand.score);
	if hand_score <= 0.0 {
		return ObservationQuality::new(0.0, "missing");
	}
	if hand_score >= 0.55 {
		return ObservationQuality::new(hand_score * 0.70, "hand_ik");
	}
	ObservationQuality::new(hand_score * 0.45, "hand_ik_low_confidence")
}

fn hand_observation_quality(side: &str, hands: &NativeHands, min_confidence: f32) -> ObservationQuality {
	let Some(hand) = hands
		.hands
		.iter()
		.take(hands.hand_count as usize)
		.find(|hand| (side == "right" && hand.handedness_is_right == 1) || (side == "left" && hand.handedness_is_right == 0))
	else {
		return ObservationQuality::new(0.0, "missing");
	};
	if hand.landmark_count < HAND_LANDMARK_COUNT as u32 {
		return ObservationQuality::new(hand.confidence.max(hand.handedness_score) * 0.2, "partial_landmarks");
	}
	let confidence = hand.confidence.max(hand.handedness_score).clamp(0.0, 1.0);
	let landmarks = if hand.world_landmark_count >= HAND_LANDMARK_COUNT as u32 {
		hand_world_points(hand)
	} else {
		hand_points(hand)
	};
	let palm = hand_palm_scale(&landmarks);
	if !palm.is_finite() || palm <= 1e-5 {
		return ObservationQuality::new(confidence * 0.2, "degenerate_palm");
	}
	if confidence < min_confidence {
		return ObservationQuality::new(confidence * 0.45, "low_confidence");
	}
	let geometry = hand_geometry_quality(&landmarks, palm);
	ObservationQuality::new(confidence * geometry, "landmark_geometry")
}

fn hand_geometry_quality(landmarks: &[Point3; HAND_LANDMARK_COUNT], palm: f32) -> f32 {
	let fingers = [[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12], [13, 14, 15, 16], [17, 18, 19, 20]];
	let mut valid = 0.0;
	for [mcp, pip, dip, tip] in fingers {
		let lengths = [
			distance3d(landmarks[mcp], landmarks[pip]),
			distance3d(landmarks[pip], landmarks[dip]),
			distance3d(landmarks[dip], landmarks[tip]),
		];
		if lengths
			.iter()
			.all(|length| length.is_finite() && *length > palm * 0.03 && *length < palm * 1.8)
		{
			valid += 1.0;
		}
	}
	(0.35 + 0.65 * (valid / 5.0_f32)).clamp(0.0, 1.0)
}

pub fn native_mediapipe_signals(
	pose: &NativePose,
	hands: &NativeHands,
	face: &NativeFace,
	config: &MediaPipePostProcessConfig,
) -> Vec<MotionSignal> {
	let mut signals = Vec::new();
	let pose_head_signals = if config.head_enabled && config.rules.head_from_pose && pose.landmark_count >= 9 {
		head_signals_from_pose(pose, config.min_landmark_confidence)
	} else {
		Vec::new()
	};
	if (config.head_enabled || config.face_enabled)
		&& (face.matrix_rows >= 3 || face.matrix_cols >= 3 || face.landmark_count > 0 || face.blendshape_count > 0)
	{
		push_face_signals(&mut signals, face, config);
		if config.head_enabled && config.rules.head_reconcile {
			reconcile_head_signals_with_pose(&mut signals, &pose_head_signals);
		}
	}
	if config.head_enabled && !signals.iter().any(|signal| signal.name.starts_with("head.")) {
		signals.extend(pose_head_signals);
	}
	if config.face_enabled
		&& config.rules.neutral_eye_fallback
		&& !signals.iter().any(|signal| signal.name.starts_with("eye."))
		&& !signals.iter().any(|signal| signal.name.starts_with("face."))
		&& pose.landmark_count >= 9
	{
		push_neutral_eye_signals_from_pose(&mut signals, pose, config.min_landmark_confidence);
	}
	if config.hands_enabled || config.arms_ik_enabled {
		for hand in hands
			.hands
			.iter()
			.take(hands.hand_count as usize)
			.filter(|hand| hand.landmark_count >= HAND_LANDMARK_COUNT as u32)
		{
			let side = match hand.handedness_is_right {
				0 => "left",
				1 => "right",
				_ => continue,
			};
			push_hand_signals(
				&mut signals,
				side,
				hand,
				config,
				config.hands_enabled && config.include_fingers && config.rules.finger_derived,
			);
		}
	}
	if config.arms_ik_enabled {
		if config.rules.arm_from_pose {
			push_arm_pose_signals(&mut signals, pose, config);
		}
		if config.rules.arm_ik_from_hands {
			push_arm_ik_from_hand_signals(&mut signals, config);
		}
	}
	if config.torso_enabled {
		push_torso_signals(&mut signals, pose, config);
	}
	if config.legs_enabled {
		push_leg_signals(&mut signals, pose, config);
	}
	if config.feet_enabled {
		push_feet_signals(&mut signals, pose, config);
	}
	signals
}

fn head_signals_from_pose(pose: &NativePose, min_confidence: f32) -> Vec<MotionSignal> {
	if pose.world_landmark_count >= 13 {
		let world = head_signals_from_pose_world(pose, min_confidence);
		if !world.is_empty() {
			return world;
		}
	}
	let mut out = Vec::new();
	let nose = pose.landmarks[0];
	let left_eye = pose.landmarks[2];
	let right_eye = pose.landmarks[5];
	let left_ear = pose.landmarks[7];
	let right_ear = pose.landmarks[8];

	let ear_dx = (left_ear.x - right_ear.x).abs();
	let yaw_confidence = landmark_confidence([nose, left_ear, right_ear]);
	if ear_dx > 1e-5 && yaw_confidence >= min_confidence {
		let ear_mid_x = (left_ear.x + right_ear.x) * 0.5;
		let yaw = ((nose.x - ear_mid_x) / (ear_dx * 0.5)).clamp(-1.0, 1.0);
		out.push(signal_scalar("head.yaw", yaw, yaw_confidence));
	}

	let eye_mid_y = (left_eye.y + right_eye.y) * 0.5;
	let pitch_confidence = landmark_confidence([nose, left_eye, right_eye]);
	if pitch_confidence >= min_confidence {
		let pitch = ((eye_mid_y - nose.y) / 0.25).clamp(-1.0, 1.0);
		out.push(signal_scalar("head.pitch", pitch, pitch_confidence));
	}

	let eye_dx = (left_eye.x - right_eye.x).abs();
	let roll_confidence = landmark_confidence([left_eye, right_eye]);
	if eye_dx > 1e-5 && roll_confidence >= min_confidence {
		let roll = ((left_eye.y - right_eye.y) / eye_dx).clamp(-1.0, 1.0);
		out.push(signal_scalar("head.roll", roll, roll_confidence));
	}
	out
}

fn head_signals_from_pose_world(pose: &NativePose, min_confidence: f32) -> Vec<MotionSignal> {
	let mut out = Vec::new();
	let nose = head_world_point(pose, 0);
	let left_eye = head_world_point(pose, 2);
	let right_eye = head_world_point(pose, 5);
	let left_ear = head_world_point(pose, 7);
	let right_ear = head_world_point(pose, 8);
	let left_shoulder = head_world_point(pose, 11);
	let right_shoulder = head_world_point(pose, 12);
	let ear_mid = average_points([left_ear, right_ear]);
	let eye_mid = average_points([left_eye, right_eye]);
	let shoulder_mid = average_points([left_shoulder, right_shoulder]);
	let ear_width = distance3d(left_ear, right_ear).max(1e-5);

	let yaw_confidence = landmark_confidence([pose.landmarks[0], pose.landmarks[7], pose.landmarks[8]]);
	if yaw_confidence >= min_confidence {
		let yaw = ((nose.x - ear_mid.x) / (ear_width * 0.65)).clamp(-1.0, 1.0);
		out.push(signal_scalar("head.yaw", yaw, yaw_confidence));
	}

	let pitch_confidence = landmark_confidence([
		pose.landmarks[0],
		pose.landmarks[2],
		pose.landmarks[5],
		pose.landmarks[11],
		pose.landmarks[12],
	]);
	if pitch_confidence >= min_confidence {
		let face_forward = sub3(nose, ear_mid);
		let forward_depth = face_forward.x.hypot(face_forward.z).max(1e-5);
		let pose_pitch = (face_forward.y.atan2(forward_depth) / 0.65).clamp(-1.0, 1.0);
		let head_lift = ((eye_mid.y - shoulder_mid.y) / distance3d(eye_mid, shoulder_mid).max(0.12)).clamp(0.0, 1.0);
		let pitch = (pose_pitch * 0.75 + (head_lift - 0.82) * 0.35).clamp(-1.0, 1.0);
		out.push(signal_scalar("head.pitch", pitch, pitch_confidence));
	}

	let roll_confidence = landmark_confidence([pose.landmarks[7], pose.landmarks[8]]);
	if roll_confidence >= min_confidence {
		let roll = ((left_ear.y - right_ear.y) / ear_width).clamp(-1.0, 1.0);
		out.push(signal_scalar("head.roll", roll, roll_confidence));
	}
	out
}

fn head_world_point(pose: &NativePose, index: usize) -> Point3 {
	let landmark = pose.world_landmarks[index];
	Point3 {
		x: landmark.x,
		y: -landmark.y,
		z: -landmark.z,
	}
}

fn pose_point(pose: &NativePose, index: usize) -> Point3 {
	let landmark = pose.landmarks[index];
	Point3 {
		x: landmark.x,
		y: landmark.y,
		z: landmark.z,
	}
}

fn reconcile_head_signals_with_pose(signals: &mut Vec<MotionSignal>, pose_head: &[MotionSignal]) {
	if pose_head.is_empty() || !signals.iter().any(|signal| signal.name.starts_with("head.")) {
		return;
	}
	if head_signals_are_saturated(signals) {
		signals.retain(|signal| !signal.name.starts_with("head."));
		signals.extend(pose_head.iter().cloned());
		return;
	}
	for pose_signal in pose_head {
		if !pose_signal.name.starts_with("head.") {
			continue;
		}
		if pose_signal.name == "head.pitch" {
			continue;
		}
		let MotionSignalValue::Scalar(pose_value) = pose_signal.value else {
			continue;
		};
		let Some(face_signal) = signals.iter_mut().find(|signal| signal.name == pose_signal.name) else {
			signals.push(pose_signal.clone());
			continue;
		};
		let MotionSignalValue::Scalar(face_value) = face_signal.value else {
			continue;
		};
		if face_value.abs() < 0.08 || pose_value.abs() < 0.08 || face_value.signum() == pose_value.signum() {
			continue;
		}
		face_signal.value = MotionSignalValue::Scalar(face_value.abs().copysign(pose_value).clamp(-1.0, 1.0));
		face_signal.confidence = face_signal.confidence.min(pose_signal.confidence);
	}
}

fn push_face_signals(signals: &mut Vec<MotionSignal>, face: &NativeFace, config: &MediaPipePostProcessConfig) {
	let c = face.matrix_cols as usize;
	let confidence = if face.confidence > 0.0 { face.confidence } else { 0.75 }.clamp(0.0, 1.0);

	if config.head_enabled && config.rules.head_from_face_matrix && c >= 3 && face.matrix.len() >= 11 {
		let [r00, r01, r02, r10, r11, r12, r20, r21, r22] = normalized_face_rotation(face);
		let _ = (r00, r01, r20, r21);
		let pitch = (-r12).clamp(-1.0, 1.0).asin();
		let yaw = r02.atan2(r22);
		let roll = r10.atan2(r11);
		push_scalar(signals, "head.yaw", (yaw / 0.85).clamp(-1.0, 1.0), confidence);
		let pitch_signal = if let Some(model) = config.face_pose_model.as_ref().filter(|model| model.enabled) {
			face_landmark_head_estimate(face, config.min_landmark_confidence, Some(model))
				.map(|estimate| estimate.pitch)
				.unwrap_or_else(|| (pitch / 0.65).clamp(-1.0, 1.0))
		} else {
			(pitch / 0.65).clamp(-1.0, 1.0)
		};
		push_scalar(signals, "head.pitch", pitch_signal, confidence);
		push_scalar(signals, "head.roll", (roll / 0.55).clamp(-1.0, 1.0), confidence);
	} else if config.head_enabled && face.landmark_count >= FACE_LANDMARK_COUNT as u32 {
		signals.extend(head_signals_from_face_landmarks(
			face,
			config.min_landmark_confidence,
			config.face_pose_model.as_ref(),
		));
	}

	if !config.face_enabled {
		return;
	}

	let score = |name: &str| -> f32 {
		face.blendshapes
			.iter()
			.take(face.blendshape_count as usize)
			.find_map(|blendshape| {
				let actual = blendshape_name(blendshape.name);
				(actual == name).then_some(blendshape.score)
			})
			.unwrap_or(0.0)
	};
	let left_yaw = score("eyeLookOutLeft") - score("eyeLookInLeft");
	let right_yaw = score("eyeLookInRight") - score("eyeLookOutRight");
	let left_pitch = score("eyeLookUpLeft") - score("eyeLookDownLeft");
	let right_pitch = score("eyeLookUpRight") - score("eyeLookDownRight");
	let mut emitted_eye_signal = false;
	for (name, value, confidence) in [
		(
			"eye.left.yaw",
			left_yaw.clamp(-1.0, 1.0),
			score("eyeLookOutLeft").max(score("eyeLookInLeft")),
		),
		(
			"eye.right.yaw",
			right_yaw.clamp(-1.0, 1.0),
			score("eyeLookOutRight").max(score("eyeLookInRight")),
		),
		(
			"eye.left.pitch",
			left_pitch.clamp(-1.0, 1.0),
			score("eyeLookUpLeft").max(score("eyeLookDownLeft")),
		),
		(
			"eye.right.pitch",
			right_pitch.clamp(-1.0, 1.0),
			score("eyeLookUpRight").max(score("eyeLookDownRight")),
		),
	] {
		if confidence > 0.0 {
			push_scalar(signals, name, value, confidence);
			emitted_eye_signal = true;
		}
	}
	if !emitted_eye_signal && face.landmark_count > 0 {
		let eye_confidence = face.confidence.clamp(0.0, 1.0);
		if eye_confidence >= config.min_landmark_confidence {
			for name in ["eye.left.yaw", "eye.right.yaw", "eye.left.pitch", "eye.right.pitch"] {
				push_scalar(signals, name, 0.0, eye_confidence);
			}
		}
	}

	for blendshape in face.blendshapes.iter().take(face.blendshape_count as usize) {
		let name = blendshape_name(blendshape.name);
		if name.is_empty() || name == "_neutral" {
			continue;
		}
		let value = remap_eye_openness_blendshape(&name, blendshape.score, config.eye_open_bias);
		push_scalar(signals, &format!("face.{name}"), value, 1.0);
	}
}

fn remap_eye_openness_blendshape(name: &str, value: f32, eye_open_bias: f32) -> f32 {
	let value = value.clamp(0.0, 1.0);
	let bias = eye_open_bias.clamp(0.0, 1.0);
	match name {
		"eyeBlinkLeft" | "eyeBlinkRight" => remap_eye_blink(value, bias),
		"eyeWideLeft" | "eyeWideRight" => remap_eye_wide(value, bias),
		_ => value,
	}
}

fn remap_eye_blink(value: f32, eye_open_bias: f32) -> f32 {
	let open = (eye_open_bias - 0.5) * 2.0;
	if open >= 0.0 {
		let deadzone = lerp(0.10, 0.55, open);
		((value - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0)
	} else {
		let heaviness = -open;
		(value + heaviness * 0.22 * (1.0 - value)).clamp(0.0, 1.0)
	}
}

fn remap_eye_wide(value: f32, eye_open_bias: f32) -> f32 {
	let gain = ((eye_open_bias - 0.5) * 1.2).max(0.0);
	(value + gain * (1.0 - value)).clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
	a + (b - a) * t.clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FaceLandmarkHeadEstimate {
	yaw: f32,
	pitch: f32,
	roll: f32,
	confidence: f32,
	nose_drop_eye_mouth: f32,
}

fn face_landmark_metrics(face: &NativeFace) -> Option<FaceLandmarkHeadEstimate> {
	face_landmark_head_estimate(face, 0.0, None)
}

fn face_landmark_head_estimate(
	face: &NativeFace,
	min_confidence: f32,
	model: Option<&FacePoseModelConfig>,
) -> Option<FaceLandmarkHeadEstimate> {
	let confidence = face.confidence.max(0.75).clamp(0.0, 1.0);
	if confidence < min_confidence {
		return None;
	}
	if face.landmark_count < FACE_LANDMARK_COUNT as u32 {
		return None;
	}
	let nose = face.landmarks[1];
	let chin = face.landmarks[152];
	let left_eye_outer = face.landmarks[33];
	let right_eye_outer = face.landmarks[263];
	let left_face = face.landmarks[234];
	let right_face = face.landmarks[454];
	let left_mouth = face.landmarks[61];
	let right_mouth = face.landmarks[291];
	let eye_width = (right_eye_outer.x - left_eye_outer.x).abs().max(1e-5);
	let face_width = (right_face.x - left_face.x).abs().max(eye_width).max(1e-5);
	let face_mid_x = (left_face.x + right_face.x) * 0.5;
	let eye_mid_y = (left_eye_outer.y + right_eye_outer.y) * 0.5;
	let mouth_mid_y = (left_mouth.y + right_mouth.y) * 0.5;
	let face_height = (chin.y - eye_mid_y).abs().max(1e-5);
	let eye_mouth_height = (mouth_mid_y - eye_mid_y).abs().max(face_height * 0.35).max(1e-5);

	let yaw = ((nose.x - face_mid_x) / (face_width * 0.65)).clamp(-0.95, 0.95);
	let nose_mouth_drop = (nose.y - eye_mid_y) / eye_mouth_height;
	let yaw_pitch_weight = (1.0 - 0.70 * (yaw.abs() / 0.95).clamp(0.0, 1.0).powi(2)).clamp(0.30, 1.0);
	let neutral_nose_mouth_drop = model
		.filter(|model| model.enabled)
		.map(|model| model.neutral_nose_drop_eye_mouth.clamp(0.35, 0.90))
		.unwrap_or(0.64);
	let pitch = (((neutral_nose_mouth_drop - nose_mouth_drop) / 0.60) * yaw_pitch_weight).clamp(-0.95, 0.95);
	let roll = ((left_eye_outer.y - right_eye_outer.y) / eye_width).clamp(-0.95, 0.95);

	Some(FaceLandmarkHeadEstimate {
		yaw,
		pitch,
		roll,
		confidence,
		nose_drop_eye_mouth: nose_mouth_drop,
	})
}

fn head_signals_from_face_landmarks(face: &NativeFace, min_confidence: f32, model: Option<&FacePoseModelConfig>) -> Vec<MotionSignal> {
	let Some(estimate) = face_landmark_head_estimate(face, min_confidence, model) else {
		return Vec::new();
	};
	vec![
		signal_scalar("head.yaw", estimate.yaw, estimate.confidence),
		signal_scalar("head.pitch", estimate.pitch, estimate.confidence),
		signal_scalar("head.roll", estimate.roll, estimate.confidence),
	]
}

fn head_signals_are_saturated(signals: &[MotionSignal]) -> bool {
	let mut head_count = 0;
	for signal in signals.iter().filter(|signal| signal.name.starts_with("head.")) {
		let MotionSignalValue::Scalar(value) = signal.value else {
			continue;
		};
		head_count += 1;
		if value.abs() >= 0.98 {
			return true;
		}
	}
	head_count > 0 && head_count < 3
}

fn push_neutral_eye_signals_from_pose(out: &mut Vec<MotionSignal>, pose: &NativePose, min_confidence: f32) {
	let confidence = landmark_confidence([pose.landmarks[0], pose.landmarks[2], pose.landmarks[5]]);
	if confidence < min_confidence {
		return;
	}
	for name in ["eye.left.yaw", "eye.right.yaw", "eye.left.pitch", "eye.right.pitch"] {
		out.push(signal_scalar(name, 0.0, confidence));
	}
}

fn normalized_face_rotation(face: &NativeFace) -> [f32; 9] {
	let c = face.matrix_cols.max(4) as usize;
	let mut row0 = [face.matrix[0], face.matrix[1], face.matrix[2]];
	let mut row1 = [face.matrix[c], face.matrix[c + 1], face.matrix[c + 2]];
	let mut row2 = [face.matrix[2 * c], face.matrix[(2 * c) + 1], face.matrix[(2 * c) + 2]];
	normalize_vec3(&mut row0);
	normalize_vec3(&mut row1);
	normalize_vec3(&mut row2);
	[row0[0], row0[1], row0[2], row1[0], row1[1], row1[2], row2[0], row2[1], row2[2]]
}

fn normalize_vec3(row: &mut [f32; 3]) {
	let length = (row[0].mul_add(row[0], row[1].mul_add(row[1], row[2] * row[2]))).sqrt();
	if !length.is_finite() || length <= 1e-6 {
		return;
	}
	for value in row {
		*value /= length;
	}
}

fn push_hand_signals(
	signals: &mut Vec<MotionSignal>,
	side: &str,
	hand: &NativeHand,
	config: &MediaPipePostProcessConfig,
	include_fingers: bool,
) {
	let confidence = hand.confidence.max(hand.handedness_score).clamp(0.0, 1.0);
	if confidence < config.min_landmark_confidence {
		return;
	}
	let normalized = hand_points(hand);
	let landmarks = if hand.world_landmark_count >= HAND_LANDMARK_COUNT as u32 {
		hand_world_points(hand)
	} else {
		normalized
	};
	let wrist = if config.rules.hand_camera_target {
		hand_camera_target(side, &normalized, config)
	} else {
		raw_hand_wrist_target(&normalized)
	};
	push_scalar(signals, &format!("hand.{side}.present"), 1.0, confidence);
	push_scalar(signals, &format!("hand.{side}.wrist.x"), wrist.x, confidence);
	push_scalar(signals, &format!("hand.{side}.wrist.y"), wrist.y, confidence);
	push_scalar(signals, &format!("hand.{side}.wrist.z"), wrist.z, confidence);
	push_scalar(signals, &format!("hand.{side}.open"), hand_open(&landmarks), confidence);
	push_scalar(signals, &format!("hand.{side}.pinch"), finger_pinch(&landmarks), confidence);
	if config.rules.hand_orientation {
		push_scalar(signals, &format!("hand.{side}.palm.roll"), palm_roll(&landmarks), confidence);

		for (name, value) in wrist_rotation_signals(side, &landmarks) {
			push_scalar(signals, &format!("hand.{side}.{name}"), value, confidence);
		}
	}

	if include_fingers {
		push_finger_curl_signals(signals, side, &landmarks, confidence);
		push_finger_spread_signals(signals, side, &landmarks, confidence);
	}
}

fn native_hand_motions(native: &NativeMediaPipeOutput, config: &MediaPipePostProcessConfig) -> (Option<HandMotion>, Option<HandMotion>) {
	let mut left = None;
	let mut right = None;
	let mut hands = NativeHands::default();
	if native.holistic.left_hand.landmark_count > 0 || native.holistic.right_hand.landmark_count > 0 {
		let mut count = 0usize;
		for hand in [native.holistic.left_hand, native.holistic.right_hand] {
			if hand.landmark_count >= HAND_LANDMARK_COUNT as u32 && count < MAX_HANDS {
				hands.hands[count] = hand;
				count += 1;
			}
		}
		hands.hand_count = count as u32;
	} else {
		hands = native.hands;
	}
	for hand in hands
		.hands
		.iter()
		.take(hands.hand_count.min(MAX_HANDS as u32) as usize)
		.filter(|hand| hand.landmark_count >= HAND_LANDMARK_COUNT as u32)
	{
		let Some(motion) = hand_motion_from_native(hand, config) else {
			continue;
		};
		match hand.handedness_is_right {
			0 => left = Some(motion),
			1 => right = Some(motion),
			_ => {}
		}
	}
	(left, right)
}

fn signal_body_motion_from_signals(signals: &[MotionSignal]) -> Option<BodyMotion> {
	let mut bones = Vec::new();
	if let Some(rotation) = head_rotation_from_signals(signals) {
		bones.push(body_bone_sample_with_confidence(HumanoidBone::Head, rotation.0, rotation.1));
	}
	if let Some((rotation, confidence)) = hips_rotation_from_signals(signals) {
		bones.push(body_bone_sample_with_confidence(HumanoidBone::Hips, rotation, confidence));
	}
	if let Some(rotation) = chest_rotation_from_signals(signals) {
		bones.push(body_bone_sample_with_confidence(HumanoidBone::Chest, rotation.0, rotation.1));
	}
	for side in [HandSide::Left, HandSide::Right] {
		if let Some(rotation) = upper_arm_local_rotation_from_signals(signals, side) {
			bones.push(body_bone_sample(side.upper_arm_bone(), rotation));
		}
		if let Some(rotation) = lower_arm_local_rotation_from_signals(signals, side) {
			bones.push(body_bone_sample(side.lower_arm_bone(), rotation));
		}
		if let Some(rotation) = hand_local_rotation_from_signals(signals, side) {
			bones.push(body_bone_sample(side.hand_bone(), rotation));
		}
		if let Some(rotation) = upper_leg_local_rotation_from_signals(signals, side) {
			bones.push(body_bone_sample(side.upper_leg_bone(), rotation));
		}
		if let Some(rotation) = lower_leg_local_rotation_from_signals(signals, side) {
			bones.push(body_bone_sample(side.lower_leg_bone(), rotation));
		}
		if let Some(rotation) = foot_local_rotation_from_signals(signals, side) {
			bones.push(body_bone_sample(side.foot_bone(), rotation));
		}
	}
	if bones.is_empty() {
		return None;
	}
	Some(BodyMotion {
		tracking_state: TrackingState::Valid,
		confidence: 1.0,
		humanoid: Some(HumanoidPose { root: None, bones }),
	})
}

fn signal_face_motion_from_signals(signals: &[MotionSignal]) -> Option<FaceMotion> {
	let expressions: Vec<_> = signals
		.iter()
		.filter_map(|signal| {
			let name = signal.name.strip_prefix("face.")?;
			let MotionSignalValue::Scalar(value) = signal.value else {
				return None;
			};
			Some(ExpressionSample {
				name: name.to_string(),
				value: value.clamp(0.0, 1.0),
				confidence: signal.confidence.clamp(0.0, 1.0),
				source_index: Some(0),
				state: SampleState::Valid,
			})
		})
		.collect();
	let head = head_transform_from_signals(signals);
	if expressions.is_empty() && head.is_none() {
		return None;
	}
	Some(FaceMotion {
		tracking_state: TrackingState::Valid,
		confidence: expressions
			.iter()
			.map(|sample| sample.confidence)
			.chain(head.as_ref().map(|_| head_confidence_from_signals(signals)))
			.reduce(f32::max)
			.unwrap_or(1.0),
		head,
		expressions,
	})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandSide {
	Left,
	Right,
}

impl HandSide {
	fn prefix(self) -> &'static str {
		match self {
			Self::Left => "left",
			Self::Right => "right",
		}
	}

	fn arm_rest_axis(self) -> [f32; 3] {
		match self {
			Self::Left => [-1.0, 0.0, 0.0],
			Self::Right => [1.0, 0.0, 0.0],
		}
	}

	fn upper_arm_bone(self) -> HumanoidBone {
		match self {
			Self::Left => HumanoidBone::LeftUpperArm,
			Self::Right => HumanoidBone::RightUpperArm,
		}
	}

	fn lower_arm_bone(self) -> HumanoidBone {
		match self {
			Self::Left => HumanoidBone::LeftLowerArm,
			Self::Right => HumanoidBone::RightLowerArm,
		}
	}

	fn hand_bone(self) -> HumanoidBone {
		match self {
			Self::Left => HumanoidBone::LeftHand,
			Self::Right => HumanoidBone::RightHand,
		}
	}

	fn upper_leg_bone(self) -> HumanoidBone {
		match self {
			Self::Left => HumanoidBone::LeftUpperLeg,
			Self::Right => HumanoidBone::RightUpperLeg,
		}
	}

	fn lower_leg_bone(self) -> HumanoidBone {
		match self {
			Self::Left => HumanoidBone::LeftLowerLeg,
			Self::Right => HumanoidBone::RightLowerLeg,
		}
	}

	fn foot_bone(self) -> HumanoidBone {
		match self {
			Self::Left => HumanoidBone::LeftFoot,
			Self::Right => HumanoidBone::RightFoot,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LostSignalBehavior {
	RestPose,
	Hold,
	Drop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LostSignalPart {
	Head,
	Hands,
	Arms,
}

fn lost_signal_behavior(value: &str) -> LostSignalBehavior {
	match value.trim().to_ascii_lowercase().as_str() {
		"hold" => LostSignalBehavior::Hold,
		"drop" | "send-none" | "none" => LostSignalBehavior::Drop,
		_ => LostSignalBehavior::RestPose,
	}
}

fn lost_signal_behavior_name(value: &str) -> &'static str {
	match lost_signal_behavior(value) {
		LostSignalBehavior::RestPose => "rest-pose",
		LostSignalBehavior::Hold => "hold",
		LostSignalBehavior::Drop => "drop",
	}
}

fn lost_signal_part_behavior(rules: &MediaPipePostProcessRules, part: LostSignalPart) -> &str {
	match part {
		LostSignalPart::Head => non_empty_or(&rules.lost_signal_head_behavior, &rules.lost_signal_behavior),
		LostSignalPart::Hands => non_empty_or(&rules.lost_signal_hands_behavior, &rules.lost_signal_behavior),
		LostSignalPart::Arms => non_empty_or(&rules.lost_signal_arms_behavior, &rules.lost_signal_behavior),
	}
}

fn lost_signal_part_rest_pose_blend(rules: &MediaPipePostProcessRules, part: LostSignalPart) -> f32 {
	match part {
		LostSignalPart::Head => rules.lost_signal_head_rest_pose_blend,
		LostSignalPart::Hands => rules.lost_signal_hands_rest_pose_blend,
		LostSignalPart::Arms => rules.lost_signal_arms_rest_pose_blend,
	}
	.clamp(0.0, 1.0)
}

fn lost_signal_part_hold_ns(rules: &MediaPipePostProcessRules, part: LostSignalPart) -> u64 {
	let seconds = match part {
		LostSignalPart::Head => rules.lost_signal_head_hold_seconds,
		LostSignalPart::Hands => rules.lost_signal_hands_hold_seconds,
		LostSignalPart::Arms => rules.lost_signal_arms_hold_seconds,
	};
	(seconds.clamp(0.0, 30.0) * 1_000_000_000.0).round() as u64
}

fn lost_signal_recovery_ns(rules: &MediaPipePostProcessRules) -> u64 {
	(rules.lost_signal_recovery_seconds.clamp(0.0, 5.0) * 1_000_000_000.0).round() as u64
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
	if value.trim().is_empty() { fallback } else { value }
}

fn rest_pose_upper_arm_rotation(side: HandSide, i_pose_blend: f32) -> [f32; 4] {
	let i_pose = quat_from_to(side.arm_rest_axis(), [0.0, -1.0, 0.0]);
	quat_array_nlerp(IDENTITY_QUAT_ARRAY, i_pose, i_pose_blend)
}

fn body_bone_sample(bone: HumanoidBone, rotation: [f32; 4]) -> BoneSample {
	body_bone_sample_with_confidence(bone, rotation, 1.0)
}

fn body_bone(frame: &UNMotionFrame, bone: HumanoidBone) -> Option<&BoneSample> {
	frame
		.body
		.as_ref()
		.and_then(|body| body.humanoid.as_ref())
		.and_then(|humanoid| humanoid.bones.iter().find(|sample| sample.bone == bone))
}

fn upsert_body_bone(frame: &mut UNMotionFrame, bone: BoneSample) {
	let body = frame.body.get_or_insert_with(|| BodyMotion {
		tracking_state: TrackingState::Recovering,
		confidence: bone.confidence,
		humanoid: Some(HumanoidPose {
			root: None,
			bones: Vec::new(),
		}),
	});
	body.tracking_state = TrackingState::Recovering;
	body.confidence = body.confidence.max(bone.confidence);
	let humanoid = body.humanoid.get_or_insert_with(|| HumanoidPose {
		root: None,
		bones: Vec::new(),
	});
	if let Some(existing) = humanoid.bones.iter_mut().find(|sample| sample.bone == bone.bone) {
		*existing = bone;
	} else {
		humanoid.bones.push(bone);
	}
}

fn body_bone_sample_with_confidence(bone: HumanoidBone, rotation: [f32; 4], confidence: f32) -> BoneSample {
	BoneSample {
		bone,
		transform: TransformSample {
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
		},
		confidence: confidence.clamp(0.0, 1.0),
		source_index: Some(0),
		state: SampleState::Valid,
	}
}

fn head_transform_from_signals(signals: &[MotionSignal]) -> Option<TransformSample> {
	head_rotation_from_signals(signals).map(|(rotation, _)| TransformSample {
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
	})
}

fn head_rotation_from_signals(signals: &[MotionSignal]) -> Option<([f32; 4], f32)> {
	let (yaw, yaw_confidence) = scalar_signal_with_confidence_from_signals(signals, "head.yaw").unwrap_or((0.0, 0.0));
	let (pitch, pitch_confidence) = scalar_signal_with_confidence_from_signals(signals, "head.pitch").unwrap_or((0.0, 0.0));
	let (roll, roll_confidence) = scalar_signal_with_confidence_from_signals(signals, "head.roll").unwrap_or((0.0, 0.0));
	let confidence = yaw_confidence.max(pitch_confidence).max(roll_confidence);
	if confidence <= 0.0 {
		return None;
	}
	let rotation = euler_radians_to_quat_array(-pitch * 0.65, yaw * 0.85, -roll * 0.55);
	Some((rotation, confidence))
}

fn head_confidence_from_signals(signals: &[MotionSignal]) -> f32 {
	["head.yaw", "head.pitch", "head.roll"]
		.into_iter()
		.filter_map(|name| scalar_signal_with_confidence_from_signals(signals, name).map(|(_, confidence)| confidence))
		.reduce(f32::max)
		.unwrap_or(0.0)
}

fn upper_arm_local_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let target = arm_segment_direction_from_signals(signals, side.prefix(), "shoulder", "elbow")?;
	if let Some(mut forearm) = arm_segment_direction_from_signals(signals, side.prefix(), "elbow", "wrist") {
		if let Some(normal) = signal_vec3_from_signals(signals, &format!("hand.{}.palm.normal", side.prefix()))
			&& upper_arm_can_share_wrist_twist(target, forearm, forearm, normal)
		{
			forearm = mix3_array(forearm, normal, 0.35).unwrap_or(forearm);
		}
		if let Some(rotation) = quat_from_basis(side.arm_rest_axis(), [0.0, 0.0, 1.0], target, forearm) {
			return Some(rotation);
		}
	}
	Some(quat_from_to(side.arm_rest_axis(), target))
}

fn lower_arm_local_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let global = lower_arm_global_rotation_from_signals(signals, side)?;
	let parent = upper_arm_local_rotation_from_signals(signals, side)?;
	Some(quat_mul(quat_inverse(parent), global))
}

fn lower_arm_global_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let target = arm_segment_direction_from_signals(signals, side.prefix(), "elbow", "wrist")?;
	if let Some(normal) = signal_vec3_from_signals(signals, &format!("hand.{}.palm.normal", side.prefix()))
		&& let Some(rotation) = quat_from_basis(side.arm_rest_axis(), [0.0, -1.0, 0.0], target, normal)
	{
		return Some(rotation);
	}
	Some(quat_from_to(side.arm_rest_axis(), target))
}

fn hand_local_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let prefix = side.prefix();
	let forward = signal_vec3_from_signals(signals, &format!("hand.{prefix}.palm.forward"))?;
	let normal = signal_vec3_from_signals(signals, &format!("hand.{prefix}.palm.normal"))
		.or_else(|| signal_vec3_from_signals(signals, &format!("hand.{prefix}.palm.across")))?;
	let global = quat_from_basis(side.arm_rest_axis(), [0.0, -1.0, 0.0], forward, normal)?;
	let parent = lower_arm_global_rotation_from_signals(signals, side)?;
	Some(quat_mul(quat_inverse(parent), global))
}

fn upper_arm_can_share_wrist_twist(upper: [f32; 3], lower: [f32; 3], plane: [f32; 3], normal: [f32; 3]) -> bool {
	if dot3_array(upper, lower) > 0.65 {
		return false;
	}
	projected_twist_angle_abs_array(plane, normal, upper).is_some_and(|angle| angle > std::f32::consts::FRAC_PI_4)
}

fn projected_twist_angle_abs_array(a: [f32; 3], b: [f32; 3], axis: [f32; 3]) -> Option<f32> {
	let axis = normalize3_array(axis)?;
	let a = project_onto_plane_array(a, axis)?;
	let b = project_onto_plane_array(b, axis)?;
	Some(dot3_array(axis, cross3_array(a, b)).atan2(dot3_array(a, b)).abs())
}

fn project_onto_plane_array(v: [f32; 3], normal: [f32; 3]) -> Option<[f32; 3]> {
	normalize3_array([
		v[0] - (normal[0] * dot3_array(v, normal)),
		v[1] - (normal[1] * dot3_array(v, normal)),
		v[2] - (normal[2] * dot3_array(v, normal)),
	])
}

fn mix3_array(a: [f32; 3], b: [f32; 3], amount: f32) -> Option<[f32; 3]> {
	let t = amount.clamp(0.0, 1.0);
	normalize3_array([
		(a[0] * (1.0 - t)) + (b[0] * t),
		(a[1] * (1.0 - t)) + (b[1] * t),
		(a[2] * (1.0 - t)) + (b[2] * t),
	])
}

fn chest_rotation_from_signals(signals: &[MotionSignal]) -> Option<([f32; 4], f32)> {
	let left_shoulder = body_point_from_signals(signals, "torso.left.shoulder")?;
	let right_shoulder = body_point_from_signals(signals, "torso.right.shoulder")?;
	let left_hip = body_point_from_signals(signals, "torso.left.hip")?;
	let right_hip = body_point_from_signals(signals, "torso.right.hip")?;
	let shoulder_across = normalize3_array([
		right_shoulder.0[0] - left_shoulder.0[0],
		right_shoulder.0[1] - left_shoulder.0[1],
		right_shoulder.0[2] - left_shoulder.0[2],
	])?;
	let shoulder_mid = midpoint_array(left_shoulder.0, right_shoulder.0);
	let hip_mid = midpoint_array(left_hip.0, right_hip.0);
	let up = normalize3_array([
		shoulder_mid[0] - hip_mid[0],
		shoulder_mid[1] - hip_mid[1],
		shoulder_mid[2] - hip_mid[2],
	])?;
	Some((
		quat_from_basis([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], shoulder_across, up).unwrap_or_else(|| quat_from_to([0.0, 1.0, 0.0], up)),
		left_shoulder.1.min(right_shoulder.1).min(left_hip.1).min(right_hip.1),
	))
}

fn hips_rotation_from_signals(signals: &[MotionSignal]) -> Option<([f32; 4], f32)> {
	let left = body_point_from_signals(signals, "leg.left.hip")?;
	let right = body_point_from_signals(signals, "leg.right.hip")?;
	let across = normalize3_array([right.0[0] - left.0[0], right.0[1] - left.0[1], right.0[2] - left.0[2]])?;
	let raw = quat_from_to([1.0, 0.0, 0.0], across);
	Some((quat_array_nlerp(IDENTITY_QUAT_ARRAY, raw, 0.35), left.1.min(right.1)))
}

fn upper_leg_local_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let target = leg_segment_direction_from_signals(signals, side.prefix(), "hip", "knee")?;
	Some(quat_from_to([0.0, -1.0, 0.0], target))
}

fn lower_leg_local_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let global = lower_leg_global_rotation_from_signals(signals, side)?;
	let parent = upper_leg_local_rotation_from_signals(signals, side)?;
	Some(quat_mul(quat_inverse(parent), global))
}

fn lower_leg_global_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let target = leg_segment_direction_from_signals(signals, side.prefix(), "knee", "ankle")?;
	Some(quat_from_to([0.0, -1.0, 0.0], target))
}

fn foot_local_rotation_from_signals(signals: &[MotionSignal], side: HandSide) -> Option<[f32; 4]> {
	let prefix = side.prefix();
	let heel = body_point_from_signals(signals, &format!("foot.{prefix}.heel"))?;
	let toe = body_point_from_signals(signals, &format!("foot.{prefix}.index"))?;
	let forward = normalize3_array([toe.0[0] - heel.0[0], toe.0[1] - heel.0[1], toe.0[2] - heel.0[2]])?;
	let global = quat_from_to([0.0, 0.0, 1.0], forward);
	let parent = lower_leg_global_rotation_from_signals(signals, side)?;
	Some(quat_mul(quat_inverse(parent), global))
}

fn arm_segment_direction_from_signals(signals: &[MotionSignal], prefix: &str, from: &str, to: &str) -> Option<[f32; 3]> {
	let from = arm_point_from_signals(signals, prefix, from)?;
	let to = arm_point_from_signals(signals, prefix, to)?;
	normalize3_array([to[0] - from[0], to[1] - from[1], to[2] - from[2]])
}

fn leg_segment_direction_from_signals(signals: &[MotionSignal], prefix: &str, from: &str, to: &str) -> Option<[f32; 3]> {
	let from = body_point_from_signals(signals, &format!("leg.{prefix}.{from}"))?.0;
	let to = body_point_from_signals(signals, &format!("leg.{prefix}.{to}"))?.0;
	normalize3_array([to[0] - from[0], to[1] - from[1], to[2] - from[2]])
}

fn arm_point_from_signals(signals: &[MotionSignal], prefix: &str, joint: &str) -> Option<[f32; 3]> {
	Some([
		scalar_signal_from_signals(signals, &format!("arm.{prefix}.{joint}.x"))?,
		scalar_signal_from_signals(signals, &format!("arm.{prefix}.{joint}.y"))?,
		scalar_signal_from_signals(signals, &format!("arm.{prefix}.{joint}.z")).unwrap_or(0.0),
	])
}

fn body_point_from_signals(signals: &[MotionSignal], prefix: &str) -> Option<([f32; 3], f32)> {
	let (x, cx) = scalar_signal_with_confidence_from_signals(signals, &format!("{prefix}.x"))?;
	let (y, cy) = scalar_signal_with_confidence_from_signals(signals, &format!("{prefix}.y"))?;
	let (z, cz) = scalar_signal_with_confidence_from_signals(signals, &format!("{prefix}.z"))?;
	Some(([x, y, z], cx.min(cy).min(cz)))
}

fn signal_vec3_from_signals(signals: &[MotionSignal], prefix: &str) -> Option<[f32; 3]> {
	Some([
		scalar_signal_from_signals(signals, &format!("{prefix}.x"))?,
		scalar_signal_from_signals(signals, &format!("{prefix}.y"))?,
		scalar_signal_from_signals(signals, &format!("{prefix}.z"))?,
	])
}

fn scalar_signal_with_confidence_from_signals(signals: &[MotionSignal], name: &str) -> Option<(f32, f32)> {
	signals.iter().find_map(|signal| {
		if signal.name == name {
			if let MotionSignalValue::Scalar(value) = signal.value {
				return Some((value.clamp(-1.0, 1.0), signal.confidence.clamp(0.0, 1.0)));
			}
		}
		None
	})
}

fn scalar_signal_from_signals(signals: &[MotionSignal], name: &str) -> Option<f32> {
	signals.iter().find_map(|signal| {
		if signal.name == name {
			if let MotionSignalValue::Scalar(value) = signal.value {
				return Some(value.clamp(-1.0, 1.0));
			}
		}
		None
	})
}

const IDENTITY_QUAT_ARRAY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn quat_from_to(from: [f32; 3], to: [f32; 3]) -> [f32; 4] {
	let Some(from) = normalize3_array(from) else {
		return IDENTITY_QUAT_ARRAY;
	};
	let Some(to) = normalize3_array(to) else {
		return IDENTITY_QUAT_ARRAY;
	};
	let dot = dot3_array(from, to).clamp(-1.0, 1.0);
	if dot > 0.9995 {
		return IDENTITY_QUAT_ARRAY;
	}
	if dot < -0.9995 {
		let axis = if from[0].abs() < 0.9 {
			normalize3_array(cross3_array(from, [1.0, 0.0, 0.0])).unwrap_or([0.0, 1.0, 0.0])
		} else {
			normalize3_array(cross3_array(from, [0.0, 1.0, 0.0])).unwrap_or([0.0, 0.0, 1.0])
		};
		return [axis[0], axis[1], axis[2], 0.0];
	}
	let axis = cross3_array(from, to);
	normalize_quat_array([axis[0], axis[1], axis[2], 1.0 + dot])
}

fn quat_from_basis(from_primary: [f32; 3], from_secondary: [f32; 3], to_primary: [f32; 3], to_secondary: [f32; 3]) -> Option<[f32; 4]> {
	let from = orthonormal_basis_array(from_primary, from_secondary)?;
	let to = orthonormal_basis_array(to_primary, to_secondary)?;
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
	Some(quat_from_rotation_matrix_array(matrix))
}

fn orthonormal_basis_array(primary: [f32; 3], secondary: [f32; 3]) -> Option<[[f32; 3]; 3]> {
	let x = normalize3_array(primary)?;
	let projected = dot3_array(secondary, x);
	let y = normalize3_array([
		secondary[0] - (x[0] * projected),
		secondary[1] - (x[1] * projected),
		secondary[2] - (x[2] * projected),
	])?;
	let z = normalize3_array(cross3_array(x, y))?;
	Some([x, y, z])
}

fn quat_from_rotation_matrix_array(matrix: [[f32; 3]; 3]) -> [f32; 4] {
	let trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
	if trace > 0.0 {
		let scale = (trace + 1.0).sqrt() * 2.0;
		return normalize_quat_array([
			(matrix[2][1] - matrix[1][2]) / scale,
			(matrix[0][2] - matrix[2][0]) / scale,
			(matrix[1][0] - matrix[0][1]) / scale,
			0.25 * scale,
		]);
	}
	if matrix[0][0] > matrix[1][1] && matrix[0][0] > matrix[2][2] {
		let scale = (1.0 + matrix[0][0] - matrix[1][1] - matrix[2][2]).sqrt() * 2.0;
		return normalize_quat_array([
			0.25 * scale,
			(matrix[0][1] + matrix[1][0]) / scale,
			(matrix[0][2] + matrix[2][0]) / scale,
			(matrix[2][1] - matrix[1][2]) / scale,
		]);
	}
	if matrix[1][1] > matrix[2][2] {
		let scale = (1.0 + matrix[1][1] - matrix[0][0] - matrix[2][2]).sqrt() * 2.0;
		return normalize_quat_array([
			(matrix[0][1] + matrix[1][0]) / scale,
			0.25 * scale,
			(matrix[1][2] + matrix[2][1]) / scale,
			(matrix[0][2] - matrix[2][0]) / scale,
		]);
	}
	let scale = (1.0 + matrix[2][2] - matrix[0][0] - matrix[1][1]).sqrt() * 2.0;
	normalize_quat_array([
		(matrix[0][2] + matrix[2][0]) / scale,
		(matrix[1][2] + matrix[2][1]) / scale,
		0.25 * scale,
		(matrix[1][0] - matrix[0][1]) / scale,
	])
}

fn quat_inverse(q: [f32; 4]) -> [f32; 4] {
	let normalized = normalize_quat_array(q);
	[-normalized[0], -normalized[1], -normalized[2], normalized[3]]
}

fn quat_mul(left: [f32; 4], right: [f32; 4]) -> [f32; 4] {
	normalize_quat_array([
		(left[3] * right[0]) + (left[0] * right[3]) + (left[1] * right[2]) - (left[2] * right[1]),
		(left[3] * right[1]) - (left[0] * right[2]) + (left[1] * right[3]) + (left[2] * right[0]),
		(left[3] * right[2]) + (left[0] * right[1]) - (left[1] * right[0]) + (left[2] * right[3]),
		(left[3] * right[3]) - (left[0] * right[0]) - (left[1] * right[1]) - (left[2] * right[2]),
	])
}

fn normalize_quat_array(q: [f32; 4]) -> [f32; 4] {
	let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
	if len <= 1e-6 {
		IDENTITY_QUAT_ARRAY
	} else {
		[q[0] / len, q[1] / len, q[2] / len, q[3] / len]
	}
}

fn quatf_nlerp(previous: &Quatf, current: &Quatf, alpha: f32) -> Quatf {
	let previous = [previous.x, previous.y, previous.z, previous.w];
	let current = [current.x, current.y, current.z, current.w];
	let blended = quat_array_nlerp(previous, current, alpha);
	Quatf {
		x: blended[0],
		y: blended[1],
		z: blended[2],
		w: blended[3],
	}
}

fn quat_array_nlerp(previous: [f32; 4], current: [f32; 4], alpha: f32) -> [f32; 4] {
	let previous = normalize_quat_array(previous);
	let mut current = normalize_quat_array(current);
	if quat_dot_array(previous, current) < 0.0 {
		current = [-current[0], -current[1], -current[2], -current[3]];
	}
	let alpha = alpha.clamp(0.0, 1.0);
	normalize_quat_array([
		(previous[0] * (1.0 - alpha)) + (current[0] * alpha),
		(previous[1] * (1.0 - alpha)) + (current[1] * alpha),
		(previous[2] * (1.0 - alpha)) + (current[2] * alpha),
		(previous[3] * (1.0 - alpha)) + (current[3] * alpha),
	])
}

fn quatf_angle_rad(previous: &Quatf, current: &Quatf) -> f32 {
	let previous = normalize_quat_array([previous.x, previous.y, previous.z, previous.w]);
	let current = normalize_quat_array([current.x, current.y, current.z, current.w]);
	let dot = quat_dot_array(previous, current).abs().clamp(-1.0, 1.0);
	2.0 * dot.acos()
}

fn quat_dot_array(left: [f32; 4], right: [f32; 4]) -> f32 {
	(left[0] * right[0]) + (left[1] * right[1]) + (left[2] * right[2]) + (left[3] * right[3])
}

fn normalize3_array(v: [f32; 3]) -> Option<[f32; 3]> {
	let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
	if len <= 1e-6 {
		None
	} else {
		Some([v[0] / len, v[1] / len, v[2] / len])
	}
}

fn dot3_array(a: [f32; 3], b: [f32; 3]) -> f32 {
	(a[0] * b[0]) + (a[1] * b[1]) + (a[2] * b[2])
}

fn cross3_array(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
	[
		(a[1] * b[2]) - (a[2] * b[1]),
		(a[2] * b[0]) - (a[0] * b[2]),
		(a[0] * b[1]) - (a[1] * b[0]),
	]
}

fn hand_motion_from_native(hand: &NativeHand, config: &MediaPipePostProcessConfig) -> Option<HandMotion> {
	let confidence = hand.confidence.max(hand.handedness_score).clamp(0.0, 1.0);
	if confidence < config.min_landmark_confidence {
		return None;
	}
	let side = match hand.handedness_is_right {
		0 => "left",
		1 => "right",
		_ => return None,
	};
	let landmarks = hand_points(hand);
	Some(HandMotion {
		tracking_state: TrackingState::Valid,
		confidence,
		// Hand landmarks are reliable for fingers, but this camera-space wrist target is not a
		// humanoid hand-bone local transform. Arm/IK signals own the wrist/hand bone pose.
		wrist: None,
		fingers: if config.include_fingers {
			typed_finger_poses(side, &landmarks, confidence)
		} else {
			Vec::new()
		},
	})
}

fn typed_finger_poses(side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT], confidence: f32) -> Vec<FingerPose> {
	let finger_specs = [
		(Finger::Thumb, "thumb", [0, 1, 2, 3, 4]),
		(Finger::Index, "index", [0, 5, 6, 7, 8]),
		(Finger::Middle, "middle", [0, 9, 10, 11, 12]),
		(Finger::Ring, "ring", [0, 13, 14, 15, 16]),
		(Finger::Little, "little", [0, 17, 18, 19, 20]),
	];
	finger_specs
		.into_iter()
		.map(|(finger, name, indices)| {
			let [root, mcp, pip, dip, tip] = indices;
			let curls = finger_joint_curls(landmarks, name, [root, mcp, pip, dip, tip]);
			let spread = finger_spread_radians(landmarks, name, mcp, pip, side);
			FingerPose {
				finger,
				joints: ["Proximal", "Intermediate", "Distal"]
					.into_iter()
					.zip(curls)
					.map(|(segment, curl)| TransformSample {
						translation: None,
						rotation: Some(finger_joint_rotation(side, name, segment, curl, spread)),
						scale: None,
						linear_velocity: None,
						angular_velocity: None,
					})
					.collect(),
				confidence,
			}
		})
		.collect()
}

fn finger_joint_rotation(side: &str, finger: &str, segment: &str, joint_curl: f32, spread: f32) -> Quatf {
	let rest = format!("{}{}", typed_finger_rest_prefix(finger), segment);
	if finger == "thumb" {
		return euler_radians_to_quatf(0.0, joint_curl.clamp(0.0, std::f32::consts::PI) * typed_side_sign(side), 0.0);
	}
	let curl_angle = joint_curl.clamp(0.0, std::f32::consts::PI) * typed_side_sign(side);
	let spread_angle = if rest.ends_with("Proximal") { spread } else { 0.0 };
	euler_radians_to_quatf(0.0, spread_angle, curl_angle)
}

fn typed_finger_rest_prefix(finger: &str) -> &'static str {
	match finger {
		"thumb" => "Thumb",
		"index" => "Index",
		"middle" => "Middle",
		"ring" => "Ring",
		"little" => "Little",
		_ => "",
	}
}

fn finger_spread_radians(landmarks: &[Point3; HAND_LANDMARK_COUNT], finger: &str, mcp_index: usize, pip_index: usize, side: &str) -> f32 {
	if finger == "middle" || finger == "thumb" {
		return 0.0;
	}
	let Some(normal) = palm_normal(landmarks) else {
		return 0.0;
	};
	let Some(middle_dir) = project_to_plane_normalized(sub3(landmarks[10], landmarks[9]), normal) else {
		return 0.0;
	};
	let Some(finger_dir) = project_to_plane_normalized(sub3(landmarks[pip_index], landmarks[mcp_index]), normal) else {
		return 0.0;
	};
	// MediaPipe already gives 3D hand landmarks. Abduction is the signed angle in the
	// palm plane from the middle-finger proximal direction to the target proximal bone.
	let yaw = signed_angle_around(middle_dir, finger_dir, normal) * typed_side_sign(side);
	match side {
		"left" | "right" => yaw,
		_ => 0.0,
	}
}

fn palm_normal(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> Option<Point3> {
	let across = sub3(landmarks[5], landmarks[17]);
	let forward = sub3(landmarks[9], landmarks[0]);
	normalize3(cross3(across, forward))
}

fn project_to_plane_normalized(v: Point3, normal: Point3) -> Option<Point3> {
	normalize3(sub3(v, scale3(normal, dot3(v, normal))))
}

fn signed_angle_around(from: Point3, to: Point3, normal: Point3) -> f32 {
	let sin = dot3(normal, cross3(from, to));
	let cos = dot3(from, to).clamp(-1.0, 1.0);
	sin.atan2(cos)
}

fn typed_side_sign(side: &str) -> f32 {
	match side {
		"left" => 1.0,
		"right" => -1.0,
		_ => 1.0,
	}
}

fn euler_radians_to_quatf(pitch: f32, yaw: f32, roll: f32) -> Quatf {
	let (sx, cx) = ((pitch * 0.5).sin(), (pitch * 0.5).cos());
	let (sy, cy) = ((yaw * 0.5).sin(), (yaw * 0.5).cos());
	let (sz, cz) = ((roll * 0.5).sin(), (roll * 0.5).cos());
	Quatf {
		x: (sx * cy * cz) + (cx * sy * sz),
		y: (cx * sy * cz) - (sx * cy * sz),
		z: (cx * cy * sz) + (sx * sy * cz),
		w: (cx * cy * cz) - (sx * sy * sz),
	}
}

fn euler_radians_to_quat_array(pitch: f32, yaw: f32, roll: f32) -> [f32; 4] {
	let quat = euler_radians_to_quatf(pitch, yaw, roll);
	[quat.x, quat.y, quat.z, quat.w]
}

fn push_arm_pose_signals(signals: &mut Vec<MotionSignal>, pose: &NativePose, config: &MediaPipePostProcessConfig) {
	if pose.landmark_count < 17 {
		return;
	}
	for (side, shoulder, elbow, wrist) in [("left", 11, 13, 15), ("right", 12, 14, 16)] {
		if pose_arm_side_requires_tracked_hand(signals, side, config) {
			continue;
		}
		push_arm_pose_side_signals(signals, side, pose, shoulder, elbow, wrist, config);
	}
}

fn pose_arm_side_requires_tracked_hand(signals: &[MotionSignal], side: &str, config: &MediaPipePostProcessConfig) -> bool {
	let present_signal = format!("hand.{side}.present");
	config.hands_enabled
		&& !signals.iter().any(|signal| {
			signal.name == present_signal
				&& matches!(signal.value, MotionSignalValue::Scalar(value) if value > 0.0)
				&& signal.confidence >= config.min_landmark_confidence
		})
}

fn push_arm_pose_side_signals(
	signals: &mut Vec<MotionSignal>,
	side: &str,
	pose: &NativePose,
	shoulder_index: usize,
	elbow_index: usize,
	wrist_index: usize,
	config: &MediaPipePostProcessConfig,
) {
	let confidence = landmark_confidence([
		pose.landmarks[shoulder_index],
		pose.landmarks[elbow_index],
		pose.landmarks[wrist_index],
	]);
	if confidence < config.min_landmark_confidence {
		return;
	}
	let use_world = pose.world_landmark_count > wrist_index as u32;
	let shoulder = pose_arm_point(pose, shoulder_index, use_world);
	let elbow = pose_arm_point(pose, elbow_index, use_world);
	let wrist = pose_arm_point(pose, wrist_index, use_world);
	let bend = arm_bend_signal(shoulder, elbow, wrist);

	for (name, value) in [
		(format!("arm.{side}.shoulder.x"), shoulder.x),
		(format!("arm.{side}.shoulder.y"), shoulder.y),
		(format!("arm.{side}.shoulder.z"), shoulder.z),
		(format!("arm.{side}.elbow.x"), elbow.x),
		(format!("arm.{side}.elbow.y"), elbow.y),
		(format!("arm.{side}.elbow.z"), elbow.z),
		(format!("arm.{side}.wrist.x"), wrist.x),
		(format!("arm.{side}.wrist.y"), wrist.y),
		(format!("arm.{side}.wrist.z"), wrist.z),
		(format!("arm.{side}.elbow.bend"), bend),
		(
			format!("arm.{side}.upper.angle"),
			((shoulder.y - elbow.y).atan2(elbow.x - shoulder.x) / std::f32::consts::PI).clamp(-1.0, 1.0),
		),
		(
			format!("arm.{side}.lower.angle"),
			((elbow.y - wrist.y).atan2(wrist.x - elbow.x) / std::f32::consts::PI).clamp(-1.0, 1.0),
		),
	] {
		push_scalar(signals, &name, value.clamp(-1.0, 1.0), confidence);
	}
}

fn pose_arm_point(pose: &NativePose, index: usize, use_world: bool) -> Point3 {
	let landmark = if use_world {
		pose.world_landmarks[index]
	} else {
		pose.landmarks[index]
	};
	if use_world {
		Point3 {
			x: landmark.x,
			y: -landmark.y,
			z: -landmark.z,
		}
	} else {
		Point3 {
			x: landmark.x - 0.5,
			y: 0.5 - landmark.y,
			z: -landmark.z,
		}
	}
}

fn arm_bend_signal(shoulder: Point3, elbow: Point3, wrist: Point3) -> f32 {
	let upper = normalize3_zero(sub3(elbow, shoulder));
	let lower = normalize3_zero(sub3(wrist, elbow));
	(1.0 - dot3(upper, lower)).clamp(0.0, 2.0) * 0.5
}

fn push_torso_signals(signals: &mut Vec<MotionSignal>, pose: &NativePose, config: &MediaPipePostProcessConfig) {
	if pose.landmark_count < 25 {
		return;
	}
	for (side, shoulder, hip) in [("left", 11, 23), ("right", 12, 24)] {
		let confidence = landmark_confidence([pose.landmarks[shoulder], pose.landmarks[hip]]);
		if confidence < config.min_landmark_confidence {
			continue;
		}
		let use_world = pose.world_landmark_count > hip as u32;
		let shoulder = pose_body_point(pose, shoulder, use_world);
		let hip = pose_body_point(pose, hip, use_world);
		for (name, value) in [
			(format!("torso.{side}.shoulder.x"), shoulder.x),
			(format!("torso.{side}.shoulder.y"), shoulder.y),
			(format!("torso.{side}.shoulder.z"), shoulder.z),
			(format!("torso.{side}.hip.x"), hip.x),
			(format!("torso.{side}.hip.y"), hip.y),
			(format!("torso.{side}.hip.z"), hip.z),
		] {
			push_scalar(signals, &name, value.clamp(-1.0, 1.0), confidence);
		}
	}
}

fn push_leg_signals(signals: &mut Vec<MotionSignal>, pose: &NativePose, config: &MediaPipePostProcessConfig) {
	if pose.landmark_count < 29 {
		return;
	}
	for (side, hip, knee, ankle) in [("left", 23, 25, 27), ("right", 24, 26, 28)] {
		let confidence = landmark_confidence([pose.landmarks[hip], pose.landmarks[knee], pose.landmarks[ankle]]);
		if confidence < config.min_landmark_confidence {
			continue;
		}
		let use_world = pose.world_landmark_count > ankle as u32;
		let hip = pose_body_point(pose, hip, use_world);
		let knee = pose_body_point(pose, knee, use_world);
		let ankle = pose_body_point(pose, ankle, use_world);
		for (name, value) in [
			(format!("leg.{side}.hip.x"), hip.x),
			(format!("leg.{side}.hip.y"), hip.y),
			(format!("leg.{side}.hip.z"), hip.z),
			(format!("leg.{side}.knee.x"), knee.x),
			(format!("leg.{side}.knee.y"), knee.y),
			(format!("leg.{side}.knee.z"), knee.z),
			(format!("leg.{side}.ankle.x"), ankle.x),
			(format!("leg.{side}.ankle.y"), ankle.y),
			(format!("leg.{side}.ankle.z"), ankle.z),
		] {
			push_scalar(signals, &name, value.clamp(-1.0, 1.0), confidence);
		}
	}
}

fn push_feet_signals(signals: &mut Vec<MotionSignal>, pose: &NativePose, config: &MediaPipePostProcessConfig) {
	if pose.landmark_count < 33 {
		return;
	}
	for (side, ankle, heel, foot_index) in [("left", 27, 29, 31), ("right", 28, 30, 32)] {
		let confidence = landmark_confidence([pose.landmarks[ankle], pose.landmarks[heel], pose.landmarks[foot_index]]);
		if confidence < config.min_landmark_confidence {
			continue;
		}
		let use_world = pose.world_landmark_count > foot_index as u32;
		let ankle = pose_body_point(pose, ankle, use_world);
		let heel = pose_body_point(pose, heel, use_world);
		let foot_index = pose_body_point(pose, foot_index, use_world);
		for (name, value) in [
			(format!("foot.{side}.ankle.x"), ankle.x),
			(format!("foot.{side}.ankle.y"), ankle.y),
			(format!("foot.{side}.ankle.z"), ankle.z),
			(format!("foot.{side}.heel.x"), heel.x),
			(format!("foot.{side}.heel.y"), heel.y),
			(format!("foot.{side}.heel.z"), heel.z),
			(format!("foot.{side}.index.x"), foot_index.x),
			(format!("foot.{side}.index.y"), foot_index.y),
			(format!("foot.{side}.index.z"), foot_index.z),
		] {
			push_scalar(signals, &name, value.clamp(-1.0, 1.0), confidence);
		}
	}
}

fn pose_body_point(pose: &NativePose, index: usize, use_world: bool) -> Point3 {
	pose_arm_point(pose, index, use_world)
}

fn push_arm_ik_from_hand_signals(signals: &mut Vec<MotionSignal>, config: &MediaPipePostProcessConfig) {
	for side in ["left", "right"] {
		if has_arm_side_signals(signals, side) {
			continue;
		}
		push_arm_ik_side_from_hand_signals(signals, side, config);
	}
}

fn has_arm_side_signals(signals: &[MotionSignal], side: &str) -> bool {
	["shoulder", "elbow", "wrist"].iter().all(|joint| {
		["x", "y", "z"]
			.iter()
			.all(|axis| signal_value(signals, &format!("arm.{side}.{joint}.{axis}")).is_some())
	})
}

fn push_arm_ik_side_from_hand_signals(signals: &mut Vec<MotionSignal>, side: &str, config: &MediaPipePostProcessConfig) {
	let Some(x) = signal_value(signals, &format!("hand.{side}.wrist.x")) else {
		return;
	};
	let Some(y) = signal_value(signals, &format!("hand.{side}.wrist.y")) else {
		return;
	};
	let Some(z) = signal_value(signals, &format!("hand.{side}.wrist.z")) else {
		return;
	};
	let Some(present) = signal_value(signals, &format!("hand.{side}.present")) else {
		return;
	};
	let confidence = x.1.min(y.1).min(z.1).min(present.1);
	if confidence < config.min_landmark_confidence {
		return;
	}

	let side_sign = if side == "left" { -1.0 } else { 1.0 };
	let crossed_right_hand = config.rules.crossed_hand_heuristic && side == "right" && x.0 > 0.02 && y.0 < 0.0;
	let shoulder = if crossed_right_hand {
		Point3 {
			x: side_sign * 0.34,
			y: 0.12,
			z: 0.08,
		}
	} else {
		Point3 {
			x: side_sign * 0.3,
			y: 0.17,
			z: 0.02,
		}
	};
	let wrist = Point3 {
		x: x.0,
		y: y.0,
		z: if crossed_right_hand { z.0 + 0.25 } else { z.0 },
	};
	let preferred = crossed_right_hand.then_some(Point3 {
		x: side_sign * -0.25,
		y: -0.65,
		z: -0.55,
	});
	let (elbow, bend) = solve_arm_ik(side_sign, shoulder, wrist, preferred);

	for (name, value) in [
		(format!("arm.{side}.shoulder.x"), shoulder.x),
		(format!("arm.{side}.shoulder.y"), shoulder.y),
		(format!("arm.{side}.shoulder.z"), shoulder.z),
		(format!("arm.{side}.elbow.x"), elbow.x),
		(format!("arm.{side}.elbow.y"), elbow.y),
		(format!("arm.{side}.elbow.z"), elbow.z),
		(format!("arm.{side}.wrist.x"), wrist.x),
		(format!("arm.{side}.wrist.y"), wrist.y),
		(format!("arm.{side}.wrist.z"), wrist.z),
		(format!("arm.{side}.elbow.bend"), bend),
		(
			format!("arm.{side}.upper.angle"),
			((shoulder.y - elbow.y).atan2(elbow.x - shoulder.x) / std::f32::consts::PI).clamp(-1.0, 1.0),
		),
		(
			format!("arm.{side}.lower.angle"),
			((elbow.y - wrist.y).atan2(wrist.x - elbow.x) / std::f32::consts::PI).clamp(-1.0, 1.0),
		),
	] {
		push_scalar(signals, &name, value.clamp(-1.0, 1.0), confidence);
	}
}

fn landmark_confidence<const N: usize>(landmarks: [un_motion_mediapipe_native::NativeLandmark; N]) -> f32 {
	let sum = landmarks
		.iter()
		.map(|landmark| landmark.visibility.max(landmark.presence).clamp(0.0, 1.0))
		.sum::<f32>();
	sum / N as f32
}

fn signal_scalar(name: &str, value: f32, confidence: f32) -> MotionSignal {
	scalar(name, value, confidence)
}

fn push_scalar(signals: &mut Vec<MotionSignal>, name: &str, value: f32, confidence: f32) {
	signals.push(scalar(name, value, confidence));
}

fn scalar(name: &str, value: f32, confidence: f32) -> MotionSignal {
	MotionSignal {
		name: name.to_string(),
		value: MotionSignalValue::Scalar(value),
		confidence,
		source_index: Some(0),
		state: SampleState::Valid,
	}
}

fn average_confidence(signals: &[MotionSignal]) -> f32 {
	if signals.is_empty() {
		0.0
	} else {
		signals.iter().map(|signal| signal.confidence).sum::<f32>() / signals.len() as f32
	}
}

fn post_process_rules_summary(rules: &MediaPipePostProcessRules) -> String {
	let mut entries = [
		("hold_lost_landmarks", rules.hold_lost_landmarks),
		("ease_recovery", rules.ease_recovery),
		("limit_rotation_jumps", rules.limit_rotation_jumps),
		("head_source_switch_blend", rules.head_source_switch_blend),
		("head_from_pose", rules.head_from_pose),
		("head_from_face_matrix", rules.head_from_face_matrix),
		("head_reconcile", rules.head_reconcile),
		("neutral_eye_fallback", rules.neutral_eye_fallback),
		("hand_camera_target", rules.hand_camera_target),
		("hand_orientation", rules.hand_orientation),
		("finger_derived", rules.finger_derived),
		("arm_from_pose", rules.arm_from_pose),
		("arm_ik_from_hands", rules.arm_ik_from_hands),
		("crossed_hand_heuristic", rules.crossed_hand_heuristic),
		("coordinate_correction", rules.coordinate_correction),
		("final_clamp", rules.final_clamp),
	]
	.into_iter()
	.map(|(name, enabled)| format!("{name}:{}", if enabled { "on" } else { "off" }))
	.collect::<Vec<_>>();
	entries.push(format!(
		"lost_signal_behavior:{}",
		lost_signal_behavior_name(&rules.lost_signal_behavior)
	));
	entries.push(format!(
		"lost_signal_rest_pose_blend:{:.2}",
		rules.lost_signal_rest_pose_blend.clamp(0.0, 1.0)
	));
	entries.push(format!(
		"lost_signal_hold_seconds:{:.1}",
		rules.lost_signal_hold_seconds.clamp(0.0, 30.0)
	));
	entries.join(",")
}

fn apply_tracking_transforms(signals: Vec<MotionSignal>, config: &MediaPipePostProcessConfig) -> Vec<MotionSignal> {
	let coordinate_corrected = signals.into_iter().map(|signal| {
		if config.rules.coordinate_correction {
			apply_vmc_coordinate_correction(signal)
		} else {
			signal
		}
	});
	let side_swapped = coordinate_corrected.map(|signal| {
		if config.mirror_mode == "swap-sides" {
			swap_signal_side(signal)
		} else {
			signal
		}
	});
	if config.mirror_mode != "mirror-output" && config.mirror_mode != "swap-sides" {
		return side_swapped.collect();
	}
	side_swapped.map(apply_user_horizontal_mirror).collect()
}

fn apply_vmc_coordinate_correction(signal: MotionSignal) -> MotionSignal {
	if signal.name.starts_with("face.") {
		return signal;
	}
	flip_signal_if(signal, should_flip_vmc_coordinate_signal)
}

fn apply_user_horizontal_mirror(signal: MotionSignal) -> MotionSignal {
	flip_signal_if(signal, should_flip_user_horizontal_signal)
}

fn flip_signal_if(mut signal: MotionSignal, should_flip: impl Fn(&str) -> bool) -> MotionSignal {
	if !should_flip(&signal.name) {
		return signal;
	}
	if let MotionSignalValue::Scalar(value) = signal.value {
		signal.value = MotionSignalValue::Scalar(-value);
	}
	signal
}

fn should_flip_vmc_coordinate_signal(name: &str) -> bool {
	name == "head.yaw"
		|| name.ends_with(".yaw")
		|| name.ends_with(".wrist.x")
		|| name.ends_with(".shoulder.x")
		|| name.ends_with(".elbow.x")
		|| name.ends_with(".hip.x")
		|| name.ends_with(".knee.x")
		|| name.ends_with(".ankle.x")
		|| name.ends_with(".heel.x")
		|| name.ends_with(".index.x")
		|| name.ends_with(".palm.forward.x")
		|| name.ends_with(".palm.across.x")
		|| name.ends_with(".palm.normal.y")
		|| name.ends_with(".palm.normal.z")
		|| name.ends_with(".upper.angle")
		|| name.ends_with(".lower.angle")
}

fn should_flip_user_horizontal_signal(name: &str) -> bool {
	should_flip_vmc_coordinate_signal(name) || name == "head.roll" || name.ends_with(".palm.roll") || name.ends_with(".wrist.roll")
}

fn swap_signal_side(mut signal: MotionSignal) -> MotionSignal {
	if signal.name.contains(".left.") {
		signal.name = signal.name.replace(".left.", ".right.");
	} else if signal.name.contains(".right.") {
		signal.name = signal.name.replace(".right.", ".left.");
	}
	signal
}

fn blendshape_name(bytes: [u8; 64]) -> String {
	let len = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
	String::from_utf8_lossy(&bytes[..len]).to_string()
}

#[derive(Clone, Copy)]
struct Point3 {
	x: f32,
	y: f32,
	z: f32,
}

fn hand_points(hand: &NativeHand) -> [Point3; HAND_LANDMARK_COUNT] {
	let mut points = [Point3 { x: 0.0, y: 0.0, z: 0.0 }; HAND_LANDMARK_COUNT];
	for (index, landmark) in hand.landmarks.iter().enumerate() {
		points[index] = Point3 {
			x: landmark.x,
			y: landmark.y,
			z: landmark.z,
		};
	}
	points
}

fn hand_world_points(hand: &NativeHand) -> [Point3; HAND_LANDMARK_COUNT] {
	let mut points = [Point3 { x: 0.0, y: 0.0, z: 0.0 }; HAND_LANDMARK_COUNT];
	for (index, landmark) in hand.world_landmarks.iter().enumerate() {
		points[index] = Point3 {
			x: landmark.x,
			y: -landmark.y,
			z: -landmark.z,
		};
	}
	points
}

fn hand_camera_target(side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT], config: &MediaPipePostProcessConfig) -> Point3 {
	let wrist = landmarks[0];
	let middle_mcp = landmarks[9];
	let palm_center = average_points([landmarks[0], landmarks[5], landmarks[9], landmarks[13], landmarks[17]]);
	let image_palm = projected_distance(wrist, middle_mcp, config.input_width, config.input_height).max(0.015);
	let camera = camera_model(config.input_width, config.input_height, config.camera_diagonal_view_angle_deg);
	let ray = camera_ray_from_normalized(palm_center.x, palm_center.y, camera);
	let depth_meters = ((0.085 * camera.focal_diag) / image_palm).clamp(0.18, 1.6);
	let depth = ((depth_meters - 0.2) / 1.25).clamp(0.0, 1.0);
	let side_bias = if side == "left" { -0.03 } else { 0.03 };
	let lateral_scale = 0.55 + (depth * 0.8);
	Point3 {
		x: ((ray.x * depth_meters * lateral_scale) + side_bias).clamp(-1.0, 1.0),
		y: ((ray.y * depth_meters * 2.65) + 0.18).clamp(-1.0, 1.0),
		z: (((0.62 - depth) * 1.35) - 0.15).clamp(-1.0, 1.0),
	}
}

fn raw_hand_wrist_target(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> Point3 {
	let wrist = landmarks[0];
	Point3 {
		x: (wrist.x - 0.5).clamp(-1.0, 1.0),
		y: (0.5 - wrist.y).clamp(-1.0, 1.0),
		z: (-wrist.z).clamp(-1.0, 1.0),
	}
}

#[derive(Clone, Copy)]
struct CameraModel {
	tan_x: f32,
	tan_y: f32,
	focal_diag: f32,
}

fn camera_model(width: u32, height: u32, diagonal_fov_deg: f32) -> CameraModel {
	let aspect = width.max(1) as f32 / height.max(1) as f32;
	let diag_rad = diagonal_fov_deg.clamp(30.0, 170.0).to_radians();
	let tan_diag = (diag_rad * 0.5).tan().max(1e-4);
	let denom = aspect.hypot(1.0).max(1e-4);
	let tan_x = tan_diag * aspect / denom;
	let tan_y = tan_diag / denom;
	let focal_diag = denom / (2.0 * tan_diag);
	CameraModel { tan_x, tan_y, focal_diag }
}

fn camera_ray_from_normalized(x: f32, y: f32, camera: CameraModel) -> Point3 {
	let cx = (x - 0.5) * 2.0 * camera.tan_x;
	let cy = (0.5 - y) * 2.0 * camera.tan_y;
	let length = (cx.mul_add(cx, cy.mul_add(cy, 1.0))).sqrt().max(1e-6);
	Point3 {
		x: cx / length,
		y: cy / length,
		z: -1.0 / length,
	}
}

fn average_points<const N: usize>(points: [Point3; N]) -> Point3 {
	let sum = points.iter().fold(Point3 { x: 0.0, y: 0.0, z: 0.0 }, |acc, point| Point3 {
		x: acc.x + point.x,
		y: acc.y + point.y,
		z: acc.z + point.z,
	});
	let scale = 1.0 / N.max(1) as f32;
	Point3 {
		x: sum.x * scale,
		y: sum.y * scale,
		z: sum.z * scale,
	}
}

fn midpoint_array(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
	[(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]
}

fn projected_distance(a: Point3, b: Point3, width: u32, height: u32) -> f32 {
	let aspect = width.max(1) as f32 / height.max(1) as f32;
	((a.x - b.x) * aspect).hypot(a.y - b.y)
}

fn signal_value(signals: &[MotionSignal], name: &str) -> Option<(f32, f32)> {
	signals
		.iter()
		.find(|signal| signal.name == name)
		.and_then(|signal| match signal.value {
			MotionSignalValue::Scalar(value) => Some((value, signal.confidence)),
			_ => None,
		})
}

fn solve_arm_ik(side_sign: f32, shoulder: Point3, wrist: Point3, preferred_override: Option<Point3>) -> (Point3, f32) {
	let upper_len = 0.48_f32;
	let lower_len = 0.46_f32;
	let shoulder_to_wrist = sub3(wrist, shoulder);
	let distance = length3(shoulder_to_wrist).clamp(0.08, upper_len + lower_len - 0.01);
	let axis = normalize3_or(
		shoulder_to_wrist,
		Point3 {
			x: side_sign,
			y: 0.0,
			z: 0.0,
		},
	);
	let along = ((upper_len * upper_len) - (lower_len * lower_len) + (distance * distance)) / (2.0 * distance);
	let height = ((upper_len * upper_len) - (along * along)).max(0.0).sqrt();
	let base = add3(shoulder, scale3(axis, along));
	let preferred = normalize3_or(
		preferred_override.unwrap_or(Point3 { x: 0.0, y: -0.55, z: -0.3 }),
		Point3 { x: 0.0, y: -1.0, z: 0.0 },
	);
	let plane = normalize3(sub3(preferred, scale3(axis, dot3(preferred, axis))))
		.or_else(|| normalize3(cross3(axis, Point3 { x: 0.0, y: 1.0, z: 0.0 })))
		.unwrap_or(Point3 { x: 0.0, y: -1.0, z: 0.0 });
	let elbow = add3(base, scale3(plane, height));
	(elbow, (height / upper_len).clamp(0.0, 1.0))
}

fn push_finger_curl_signals(signals: &mut Vec<MotionSignal>, side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT], confidence: f32) {
	push_scalar(signals, &format!("hand.{side}.thumb.curl"), thumb_curl(landmarks), confidence);
	push_joint_curl_signals(signals, side, "thumb", landmarks, [0, 1, 2, 3, 4], confidence);
	for (finger, curl_indices, joint_indices) in [
		("index", [5, 6, 7, 8], [0, 5, 6, 7, 8]),
		("middle", [9, 10, 11, 12], [0, 9, 10, 11, 12]),
		("ring", [13, 14, 15, 16], [0, 13, 14, 15, 16]),
		("little", [17, 18, 19, 20], [0, 17, 18, 19, 20]),
	] {
		push_scalar(
			signals,
			&format!("hand.{side}.{finger}.curl"),
			finger_curl(landmarks, curl_indices),
			confidence,
		);
		push_joint_curl_signals(signals, side, finger, landmarks, joint_indices, confidence);
	}
}

fn push_joint_curl_signals(
	signals: &mut Vec<MotionSignal>,
	side: &str,
	finger: &str,
	landmarks: &[Point3; HAND_LANDMARK_COUNT],
	indices: [usize; 5],
	confidence: f32,
) {
	let [root, mcp, pip, dip, tip] = indices;
	let curls = finger_joint_curls(landmarks, finger, [root, mcp, pip, dip, tip]);
	push_scalar(signals, &format!("hand.{side}.{finger}.mcp.curl"), curls[0], confidence);
	push_scalar(signals, &format!("hand.{side}.{finger}.pip.curl"), curls[1], confidence);
	push_scalar(signals, &format!("hand.{side}.{finger}.dip.curl"), curls[2], confidence);
}

fn push_finger_spread_signals(signals: &mut Vec<MotionSignal>, side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT], confidence: f32) {
	for (finger, mcp, pip) in [
		("thumb", 1, 2),
		("index", 5, 6),
		("middle", 9, 10),
		("ring", 13, 14),
		("little", 17, 18),
	] {
		push_scalar(
			signals,
			&format!("hand.{side}.{finger}.spread"),
			finger_spread_radians(landmarks, finger, mcp, pip, side),
			confidence,
		);
	}
}

fn finger_pinch(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	(1.0 - distance3d(landmarks[4], landmarks[8]) / (hand_palm_scale(landmarks) * 0.95)).clamp(0.0, 1.0)
}

fn hand_open(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	let wrist = landmarks[0];
	let tip_spread = (distance3d(wrist, landmarks[8])
		+ distance3d(wrist, landmarks[12])
		+ distance3d(wrist, landmarks[16])
		+ distance3d(wrist, landmarks[20]))
		/ 4.0;
	let mcp_spread = (distance3d(wrist, landmarks[5])
		+ distance3d(wrist, landmarks[9])
		+ distance3d(wrist, landmarks[13])
		+ distance3d(wrist, landmarks[17]))
		/ 4.0;
	((tip_spread - mcp_spread) / hand_palm_scale(landmarks)).clamp(0.0, 1.0)
}

fn palm_roll(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	let index = landmarks[5];
	let little = landmarks[17];
	((index.y - little.y).atan2(index.x - little.x) / std::f32::consts::PI).clamp(-1.0, 1.0)
}

fn wrist_rotation_signals(side: &str, landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> [(&'static str, f32); 12] {
	let wrist = landmarks[0];
	let index = landmarks[5];
	let middle = landmarks[9];
	let little = landmarks[17];
	let forward = normalize3_zero(sub3(middle, wrist));
	let across = normalize3_zero(sub3(index, little));
	let mut normal = normalize3_zero(cross3(across, forward));
	if side == "right" {
		normal = scale3(normal, -1.0);
	}
	let side_sign = if side == "left" { 1.0 } else { -1.0 };
	let pitch = (forward.y.atan2((forward.x * forward.x + forward.z * forward.z).sqrt()) / 1.2).clamp(-1.0, 1.0);
	let yaw = (forward.x.atan2(-forward.z) / 1.2).clamp(-1.0, 1.0) * side_sign;
	let roll = (across.y.atan2(across.x) / 1.2).clamp(-1.0, 1.0) * side_sign;
	[
		("palm.forward.x", forward.x),
		("palm.forward.y", forward.y),
		("palm.forward.z", forward.z),
		("palm.across.x", across.x),
		("palm.across.y", across.y),
		("palm.across.z", across.z),
		("palm.normal.x", normal.x),
		("palm.normal.y", normal.y),
		("palm.normal.z", normal.z),
		("wrist.pitch", pitch),
		("wrist.yaw", yaw),
		("wrist.roll", roll),
	]
}

fn thumb_curl(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	let wrist = landmarks[0];
	let thumb_tip = landmarks[4];
	let index_mcp = landmarks[5];
	let closed_distance = distance3d(thumb_tip, index_mcp);
	let open_distance = distance3d(thumb_tip, wrist);
	(1.0 - closed_distance / open_distance.max(hand_palm_scale(landmarks))).clamp(0.0, 1.0)
}

fn finger_curl(landmarks: &[Point3; HAND_LANDMARK_COUNT], indices: [usize; 4]) -> f32 {
	let [mcp, pip, dip, tip] = indices;
	let chain_length = distance3d(landmarks[mcp], landmarks[pip])
		+ distance3d(landmarks[pip], landmarks[dip])
		+ distance3d(landmarks[dip], landmarks[tip]);
	if chain_length <= 1e-5 {
		return 0.0;
	}
	(1.0 - distance3d(landmarks[mcp], landmarks[tip]) / chain_length).clamp(0.0, 1.0)
}

fn finger_joint_curls(landmarks: &[Point3; HAND_LANDMARK_COUNT], finger: &str, indices: [usize; 5]) -> [f32; 3] {
	let [root, mcp, pip, dip, tip] = indices;
	if finger == "thumb" {
		return [
			joint_curl(landmarks[root], landmarks[mcp], landmarks[pip]),
			joint_curl(landmarks[mcp], landmarks[pip], landmarks[dip]),
			joint_curl(landmarks[pip], landmarks[dip], landmarks[tip]),
		];
	}
	let Some(plane_normal) = finger_flexion_plane_normal(landmarks, mcp, pip) else {
		let raw = [
			joint_curl(landmarks[root], landmarks[mcp], landmarks[pip]),
			joint_curl(landmarks[mcp], landmarks[pip], landmarks[dip]),
			joint_curl(landmarks[pip], landmarks[dip], landmarks[tip]),
		];
		return reconcile_joint_curls_with_finger_chord(landmarks, [mcp, pip, dip, tip], raw);
	};
	let raw = [
		joint_curl_in_plane(landmarks[root], landmarks[mcp], landmarks[pip], plane_normal),
		joint_curl_in_plane(landmarks[mcp], landmarks[pip], landmarks[dip], plane_normal),
		joint_curl_in_plane(landmarks[pip], landmarks[dip], landmarks[tip], plane_normal),
	];
	reconcile_joint_curls_with_finger_chord(landmarks, [mcp, pip, dip, tip], raw)
}

fn limit_joint_curls_by_finger_chord(landmarks: &[Point3; HAND_LANDMARK_COUNT], indices: [usize; 4], raw: [f32; 3]) -> [f32; 3] {
	let max_total_curl = finger_curl(landmarks, indices) * std::f32::consts::PI;
	let raw_total = raw.iter().copied().sum::<f32>();
	if raw_total <= max_total_curl || raw_total <= 1e-5 {
		return raw;
	}
	let scale = max_total_curl / raw_total;
	raw.map(|curl| curl * scale)
}

fn reconcile_joint_curls_with_finger_chord(landmarks: &[Point3; HAND_LANDMARK_COUNT], indices: [usize; 4], raw: [f32; 3]) -> [f32; 3] {
	let limited = limit_joint_curls_by_finger_chord(landmarks, indices, raw);
	let chord_total = circular_arc_total_curl_from_chord(landmarks, indices);
	let raw_total = limited.iter().copied().sum::<f32>();
	if chord_total <= raw_total || chord_total < std::f32::consts::FRAC_PI_2 {
		return limited;
	}
	distribute_total_finger_curl(limited, chord_total.min(std::f32::consts::PI * 1.65))
}

fn circular_arc_total_curl_from_chord(landmarks: &[Point3; HAND_LANDMARK_COUNT], indices: [usize; 4]) -> f32 {
	let [mcp, pip, dip, tip] = indices;
	let chain_length = distance3d(landmarks[mcp], landmarks[pip])
		+ distance3d(landmarks[pip], landmarks[dip])
		+ distance3d(landmarks[dip], landmarks[tip]);
	if chain_length <= 1e-5 {
		return 0.0;
	}
	let chord_ratio = (distance3d(landmarks[mcp], landmarks[tip]) / chain_length).clamp(0.0, 1.0);
	if chord_ratio >= 0.92 {
		return 0.0;
	}
	let mut lo = 0.0_f32;
	let mut hi = std::f32::consts::PI * 1.95;
	for _ in 0..24 {
		let mid = (lo + hi) * 0.5;
		let arc_ratio = if mid.abs() <= 1e-5 {
			1.0
		} else {
			(mid * 0.5).sin().abs() / (mid * 0.5)
		};
		if arc_ratio > chord_ratio {
			lo = mid;
		} else {
			hi = mid;
		}
	}
	(lo + hi) * 0.5
}

fn distribute_total_finger_curl(raw: [f32; 3], target_total: f32) -> [f32; 3] {
	let raw_total = raw.iter().copied().sum::<f32>();
	let weights = [0.34_f32, 0.44_f32, 0.22_f32];
	if raw_total > 1e-5 {
		let raw_weights = raw.map(|curl| curl / raw_total);
		let blended = [
			(raw_weights[0] * 0.35) + (weights[0] * 0.65),
			(raw_weights[1] * 0.35) + (weights[1] * 0.65),
			(raw_weights[2] * 0.35) + (weights[2] * 0.65),
		];
		return [
			(target_total * blended[0]).min(1.35),
			(target_total * blended[1]).min(1.75),
			(target_total * blended[2]).min(1.15),
		];
	}
	[
		(target_total * weights[0]).min(1.35),
		(target_total * weights[1]).min(1.75),
		(target_total * weights[2]).min(1.15),
	]
}

fn finger_flexion_plane_normal(landmarks: &[Point3; HAND_LANDMARK_COUNT], mcp_index: usize, pip_index: usize) -> Option<Point3> {
	let palm_normal = palm_normal(landmarks)?;
	let proximal = project_to_plane_normalized(sub3(landmarks[pip_index], landmarks[mcp_index]), palm_normal)?;
	normalize3(cross3(proximal, palm_normal))
}

fn joint_curl_in_plane(a: Point3, b: Point3, c: Point3, plane_normal: Point3) -> f32 {
	let ba = project_to_plane_normalized(sub3(a, b), plane_normal).unwrap_or_else(|| normalize3_zero(sub3(a, b)));
	let bc = project_to_plane_normalized(sub3(c, b), plane_normal).unwrap_or_else(|| normalize3_zero(sub3(c, b)));
	let dot = dot3(ba, bc).clamp(-1.0, 1.0);
	let angle = dot.acos();
	(std::f32::consts::PI - angle).clamp(0.0, std::f32::consts::PI)
}

fn joint_curl(a: Point3, b: Point3, c: Point3) -> f32 {
	let ba = normalize3_zero(sub3(a, b));
	let bc = normalize3_zero(sub3(c, b));
	let dot = dot3(ba, bc).clamp(-1.0, 1.0);
	let angle = dot.acos();
	(std::f32::consts::PI - angle).clamp(0.0, std::f32::consts::PI)
}

fn hand_palm_scale(landmarks: &[Point3; HAND_LANDMARK_COUNT]) -> f32 {
	distance3d(landmarks[0], landmarks[9]).max(0.08)
}

fn distance3d(a: Point3, b: Point3) -> f32 {
	let d = sub3(a, b);
	(d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
}

fn sub3(a: Point3, b: Point3) -> Point3 {
	Point3 {
		x: a.x - b.x,
		y: a.y - b.y,
		z: a.z - b.z,
	}
}

fn add3(a: Point3, b: Point3) -> Point3 {
	Point3 {
		x: a.x + b.x,
		y: a.y + b.y,
		z: a.z + b.z,
	}
}

fn scale3(v: Point3, scale: f32) -> Point3 {
	Point3 {
		x: v.x * scale,
		y: v.y * scale,
		z: v.z * scale,
	}
}

fn cross3(a: Point3, b: Point3) -> Point3 {
	Point3 {
		x: a.y * b.z - a.z * b.y,
		y: a.z * b.x - a.x * b.z,
		z: a.x * b.y - a.y * b.x,
	}
}

fn dot3(a: Point3, b: Point3) -> f32 {
	a.x * b.x + a.y * b.y + a.z * b.z
}

fn length3(v: Point3) -> f32 {
	(v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

fn normalize3(v: Point3) -> Option<Point3> {
	let len = length3(v);
	(len > 1e-5).then_some(Point3 {
		x: v.x / len,
		y: v.y / len,
		z: v.z / len,
	})
}

fn normalize3_or(v: Point3, fallback: Point3) -> Point3 {
	normalize3(v).unwrap_or(fallback)
}

fn normalize3_zero(v: Point3) -> Point3 {
	normalize3(v).unwrap_or(Point3 { x: 0.0, y: 0.0, z: 0.0 })
}

fn now_unix_ns() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_nanos() as u64)
		.unwrap_or(0)
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_mediapipe_native::{FACE_LANDMARK_COUNT, NativeFaceBlendshape, NativeHands, NativeLandmark};

	#[test]
	fn normal_coordinate_correction_does_not_flip_local_roll_axes() {
		let out = apply_tracking_transforms(
			vec![
				scalar("head.yaw", 0.20, 1.0),
				scalar("head.roll", 0.25, 1.0),
				scalar("hand.left.wrist.x", 0.30, 1.0),
				scalar("hand.left.wrist.roll", 0.40, 1.0),
				scalar("hand.left.palm.roll", 0.50, 1.0),
				scalar("hand.left.palm.forward.x", 0.70, 1.0),
				scalar("hand.left.palm.normal.z", 0.80, 1.0),
				scalar("arm.left.wrist.x", 0.60, 1.0),
			],
			&config_with_mirror("normal"),
		);

		assert_signal_near(&out, "head.yaw", -0.20);
		assert_signal_near(&out, "head.roll", 0.25);
		assert_signal_near(&out, "hand.left.wrist.x", -0.30);
		assert_signal_near(&out, "hand.left.wrist.roll", 0.40);
		assert_signal_near(&out, "hand.left.palm.roll", 0.50);
		assert_signal_near(&out, "hand.left.palm.forward.x", -0.70);
		assert_signal_near(&out, "hand.left.palm.normal.z", -0.80);
		assert_signal_near(&out, "arm.left.wrist.x", -0.60);
	}

	#[test]
	fn mirror_output_flips_local_roll_axes_after_coordinate_correction() {
		let out = apply_tracking_transforms(
			vec![
				scalar("head.yaw", 0.20, 1.0),
				scalar("head.roll", 0.25, 1.0),
				scalar("hand.left.wrist.x", 0.30, 1.0),
				scalar("hand.left.wrist.roll", 0.40, 1.0),
				scalar("hand.left.palm.roll", 0.50, 1.0),
				scalar("hand.left.palm.forward.x", 0.70, 1.0),
				scalar("hand.left.palm.normal.z", 0.80, 1.0),
			],
			&config_with_mirror("mirror-output"),
		);

		assert_signal_near(&out, "head.yaw", 0.20);
		assert_signal_near(&out, "head.roll", -0.25);
		assert_signal_near(&out, "hand.left.wrist.x", 0.30);
		assert_signal_near(&out, "hand.left.wrist.roll", -0.40);
		assert_signal_near(&out, "hand.left.palm.roll", -0.50);
		assert_signal_near(&out, "hand.left.palm.forward.x", 0.70);
		assert_signal_near(&out, "hand.left.palm.normal.z", 0.80);
	}

	#[test]
	fn face_head_rotation_uses_pose_direction_when_axis_sign_conflicts() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 1.0;
		native.face.matrix_rows = 4;
		native.face.matrix_cols = 4;
		native.face.matrix[0] = 1.0;
		native.face.matrix[4] = 0.30;
		native.face.matrix[5] = 1.0;
		native.face.matrix[10] = 1.0;
		native.pose.landmark_count = 9;
		native.pose.landmarks[0] = lm(0.50, 0.48);
		native.pose.landmarks[2] = lm(0.35, 0.42);
		native.pose.landmarks[5] = lm(0.65, 0.52);
		native.pose.landmarks[7] = lm(0.22, 0.48);
		native.pose.landmarks[8] = lm(0.78, 0.48);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		let roll = scalar_value(&frame, "head.roll").expect("head roll");

		assert!(roll < -0.4, "head roll should follow pose sign, got {roll}");
	}

	#[test]
	fn face_head_pitch_is_not_reconciled_to_weak_pose_sign() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 1.0;
		native.face.matrix_rows = 4;
		native.face.matrix_cols = 4;
		native.face.matrix[0] = 1.0;
		native.face.matrix[5] = 0.94;
		native.face.matrix[6] = -0.34;
		native.face.matrix[9] = 0.34;
		native.face.matrix[10] = 0.94;
		native.pose.landmark_count = 9;
		native.pose.landmarks[0] = lm(0.50, 0.56);
		native.pose.landmarks[2] = lm(0.42, 0.42);
		native.pose.landmarks[5] = lm(0.58, 0.42);
		native.pose.landmarks[7] = lm(0.30, 0.48);
		native.pose.landmarks[8] = lm(0.70, 0.48);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		let pitch = scalar_value(&frame, "head.pitch").expect("head pitch");

		assert!(
			pitch > 0.4,
			"face head pitch should keep the face source sign instead of weak pose sign, got {pitch}"
		);
	}

	#[test]
	fn native_face_landmarks_drive_head_yaw_when_matrix_is_missing() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 1.0;
		native.face.landmarks[1] = lm3(0.66, 0.42, 0.0);
		native.face.landmarks[33] = lm3(0.44, 0.39, 0.0);
		native.face.landmarks[263] = lm3(0.58, 0.40, 0.0);
		native.face.landmarks[152] = lm3(0.52, 0.72, 0.0);
		native.face.landmarks[234] = lm3(0.34, 0.50, 0.0);
		native.face.landmarks[454] = lm3(0.66, 0.50, 0.0);
		native.pose.landmark_count = 9;
		native.pose.landmarks[0] = lm(0.52, 0.45);
		native.pose.landmarks[2] = lm(0.60, 0.40);
		native.pose.landmarks[5] = lm(0.40, 0.40);
		native.pose.landmarks[7] = lm(0.72, 0.50);
		native.pose.landmarks[8] = lm(0.28, 0.50);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		let yaw = scalar_value(&frame, "head.yaw").expect("head yaw");

		assert!(
			yaw < -0.7,
			"face landmarks should preserve strong profile yaw after coordinate correction, got {yaw}"
		);
	}

	#[test]
	fn native_face_landmark_head_pitch_neutralizes_frontal_nose_drop() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 1.0;
		native.face.landmarks[1] = lm3(0.50, 0.54, 0.0);
		native.face.landmarks[33] = lm3(0.42, 0.40, 0.0);
		native.face.landmarks[263] = lm3(0.58, 0.40, 0.0);
		native.face.landmarks[152] = lm3(0.50, 0.80, 0.0);
		native.face.landmarks[234] = lm3(0.32, 0.55, 0.0);
		native.face.landmarks[454] = lm3(0.68, 0.55, 0.0);
		native.face.landmarks[61] = lm3(0.45, 0.62, 0.0);
		native.face.landmarks[291] = lm3(0.55, 0.62, 0.0);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		let pitch = scalar_value(&frame, "head.pitch").expect("head pitch");
		let head = body_bone(&frame, HumanoidBone::Head).expect("head bone");
		let rotation = head.transform.rotation.expect("head rotation");

		assert!(
			pitch.abs() < 0.02,
			"frontal face landmark fallback should not look down, got {pitch}"
		);
		assert!(
			rotation.x.abs() < 0.02,
			"frontal head X rotation should stay near neutral: {rotation:?}"
		);
	}

	#[test]
	fn face_pose_model_overrides_face_matrix_pitch_from_performer_neutral() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 1.0;
		native.face.matrix_rows = 4;
		native.face.matrix_cols = 4;
		native.face.matrix[0] = 1.0;
		native.face.matrix[5] = 0.94;
		native.face.matrix[6] = -0.34;
		native.face.matrix[9] = 0.34;
		native.face.matrix[10] = 0.94;
		native.face.landmarks[1] = lm3(0.50, 0.54, 0.0);
		native.face.landmarks[33] = lm3(0.42, 0.40, 0.0);
		native.face.landmarks[263] = lm3(0.58, 0.40, 0.0);
		native.face.landmarks[152] = lm3(0.50, 0.80, 0.0);
		native.face.landmarks[234] = lm3(0.32, 0.55, 0.0);
		native.face.landmarks[454] = lm3(0.68, 0.55, 0.0);
		native.face.landmarks[61] = lm3(0.45, 0.62, 0.0);
		native.face.landmarks[291] = lm3(0.55, 0.62, 0.0);

		let mut processor = MediaPipePostProcessor::new(MediaPipePostProcessConfig {
			face_pose_model: Some(FacePoseModelConfig {
				enabled: true,
				neutral_nose_drop_eye_mouth: 0.636,
			}),
			..MediaPipePostProcessConfig::default()
		});
		let frame = processor.process_native_output(&input, &native);
		let pitch = scalar_value(&frame, "head.pitch").expect("head pitch");

		assert!(
			pitch.abs() < 0.03,
			"face model should replace biased matrix pitch with performer-neutral landmark pitch, got {pitch}"
		);
		assert!(
			frame.metadata.notes.iter().any(|note| note.starts_with("mediapipe.face_metrics ")),
			"face metrics note should be available for live model sampling"
		);
	}

	#[test]
	fn holistic_face_landmarks_emit_face_metrics_for_model_sampling() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.holistic.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.holistic.face.confidence = 1.0;
		native.holistic.face.landmarks[1] = lm3(0.50, 0.54, 0.0);
		native.holistic.face.landmarks[33] = lm3(0.42, 0.40, 0.0);
		native.holistic.face.landmarks[263] = lm3(0.58, 0.40, 0.0);
		native.holistic.face.landmarks[152] = lm3(0.50, 0.80, 0.0);
		native.holistic.face.landmarks[234] = lm3(0.32, 0.55, 0.0);
		native.holistic.face.landmarks[454] = lm3(0.68, 0.55, 0.0);
		native.holistic.face.landmarks[61] = lm3(0.45, 0.62, 0.0);
		native.holistic.face.landmarks[291] = lm3(0.55, 0.62, 0.0);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);

		assert!(
			frame.metadata.notes.iter().any(|note| note.starts_with("mediapipe.face_metrics ")),
			"holistic face metrics note should be available for live model sampling"
		);
	}

	#[test]
	fn stabilizer_holds_head_during_short_observation_loss() {
		let mut processor = MediaPipePostProcessor::default();
		let mut tracked = NativeMediaPipeOutput::default();
		tracked.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		tracked.face.confidence = 1.0;
		tracked.face.landmarks[1] = lm3(0.66, 0.42, 0.0);
		tracked.face.landmarks[33] = lm3(0.44, 0.39, 0.0);
		tracked.face.landmarks[263] = lm3(0.58, 0.40, 0.0);
		tracked.face.landmarks[152] = lm3(0.52, 0.72, 0.0);
		tracked.face.landmarks[234] = lm3(0.34, 0.50, 0.0);
		tracked.face.landmarks[454] = lm3(0.66, 0.50, 0.0);

		let first = processor.process_native_output_with_sequence(1, 0, &tracked);
		assert!(body_bone(&first, HumanoidBone::Head).is_some());
		let held = processor.process_native_output_with_sequence(2, 100_000_000, &NativeMediaPipeOutput::default());
		let head = body_bone(&held, HumanoidBone::Head).expect("held head");

		assert_eq!(head.state, SampleState::Held);
		assert!(held.metadata.notes.iter().any(|note| note == "mediapipe.stability head=held"));
	}

	#[test]
	fn stabilizer_holds_hand_during_short_observation_loss() {
		let mut processor = MediaPipePostProcessor::default();
		let mut tracked = NativeMediaPipeOutput {
			hands: NativeHands {
				hand_count: 1,
				..NativeHands::default()
			},
			..NativeMediaPipeOutput::default()
		};
		tracked.hands.hands[0].landmark_count = HAND_LANDMARK_COUNT as u32;
		tracked.hands.hands[0].handedness_is_right = 1;
		tracked.hands.hands[0].handedness_score = 1.0;
		for index in 0..HAND_LANDMARK_COUNT {
			tracked.hands.hands[0].landmarks[index] = lm3(0.45 + index as f32 * 0.003, 0.55 - index as f32 * 0.004, 0.01);
		}

		let first = processor.process_native_output_with_sequence(1, 0, &tracked);
		assert!(first.right_hand.is_some());
		let held = processor.process_native_output_with_sequence(2, 100_000_000, &NativeMediaPipeOutput::default());
		let right = held.right_hand.expect("held right hand");

		assert_eq!(right.tracking_state, TrackingState::Recovering);
		assert_eq!(right.fingers.len(), 5);
	}

	#[test]
	fn stabilizer_holds_arm_chain_during_short_pose_loss() {
		let mut processor = MediaPipePostProcessor::new(MediaPipePostProcessConfig {
			hands_enabled: false,
			..MediaPipePostProcessConfig::default()
		});
		let mut tracked = NativeMediaPipeOutput::default();
		tracked.pose.landmark_count = 17;
		tracked.pose.world_landmark_count = 17;
		for index in [11, 12, 13, 14, 15, 16] {
			tracked.pose.landmarks[index] = lm3(0.5, 0.5, 0.0);
		}
		tracked.pose.world_landmarks[11] = lm3(0.20, -0.40, -0.04);
		tracked.pose.world_landmarks[13] = lm3(0.28, -0.20, -0.10);
		tracked.pose.world_landmarks[15] = lm3(0.39, -0.30, -0.26);
		tracked.pose.world_landmarks[12] = lm3(-0.10, -0.40, -0.04);
		tracked.pose.world_landmarks[14] = lm3(-0.14, -0.21, -0.13);
		tracked.pose.world_landmarks[16] = lm3(-0.08, -0.36, -0.31);

		let first = processor.process_native_output_with_sequence(1, 0, &tracked);
		assert!(body_bone(&first, HumanoidBone::LeftUpperArm).is_some());
		let held = processor.process_native_output_with_sequence(2, 100_000_000, &NativeMediaPipeOutput::default());
		let left_upper = body_bone(&held, HumanoidBone::LeftUpperArm).expect("held left upper arm");
		let right_lower = body_bone(&held, HumanoidBone::RightLowerArm).expect("held right lower arm");

		assert_eq!(left_upper.state, SampleState::Held);
		assert_eq!(right_lower.state, SampleState::Held);
		assert!(held.metadata.notes.iter().any(|note| note == "mediapipe.stability left_arm=held"));
		assert!(held.metadata.notes.iter().any(|note| note == "mediapipe.stability right_arm=held"));
	}

	#[test]
	fn stabilizer_blends_head_recovery_after_short_observation_loss() {
		let mut stabilizer = MotionStabilizer::default();
		let tracked_quality = ObservationSet {
			head: ObservationQuality::new(1.0, "test"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};
		let lost_quality = ObservationSet {
			head: ObservationQuality::new(0.0, "missing"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};
		let mut first = UNMotionFrame::new(1);
		first.header.capture_timestamp_ns = 0;
		upsert_body_bone(&mut first, body_bone_sample(HumanoidBone::Head, IDENTITY_QUAT_ARRAY));
		stabilizer.apply(&mut first, &tracked_quality, &MediaPipePostProcessRules::default());

		let mut held = UNMotionFrame::new(2);
		held.header.capture_timestamp_ns = 100_000_000;
		stabilizer.apply(&mut held, &lost_quality, &MediaPipePostProcessRules::default());
		assert_eq!(body_bone(&held, HumanoidBone::Head).expect("held head").state, SampleState::Held);

		let target = euler_radians_to_quat_array(0.0, 1.2, 0.0);
		let mut recovered = UNMotionFrame::new(3);
		recovered.header.capture_timestamp_ns = 200_000_000;
		upsert_body_bone(&mut recovered, body_bone_sample(HumanoidBone::Head, target));
		stabilizer.apply(&mut recovered, &tracked_quality, &MediaPipePostProcessRules::default());
		let rotation = body_bone(&recovered, HumanoidBone::Head)
			.and_then(|bone| bone.transform.rotation.as_ref())
			.expect("recovered head rotation");

		assert!(rotation.y > 0.0, "recovered head should move toward the reacquired observation");
		assert!(
			rotation.y < target[1],
			"recovered head should not snap to the raw reacquired observation"
		);
		assert!(
			recovered
				.metadata
				.notes
				.iter()
				.any(|note| note == "mediapipe.stability head=recovering")
		);
	}

	#[test]
	fn stabilizer_blends_head_source_switch_without_loss() {
		let mut stabilizer = MotionStabilizer::default();
		let face_quality = ObservationSet {
			head: ObservationQuality::new(1.0, "face_matrix"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};
		let pose_quality = ObservationSet {
			head: ObservationQuality::new(1.0, "pose_world"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};

		let mut first = UNMotionFrame::new(1);
		first.header.capture_timestamp_ns = 0;
		upsert_body_bone(&mut first, body_bone_sample(HumanoidBone::Head, [0.0, 0.38, 0.0, 0.925]));
		stabilizer.apply(&mut first, &face_quality, &MediaPipePostProcessRules::default());

		let mut switched = UNMotionFrame::new(2);
		switched.header.capture_timestamp_ns = 33_000_000;
		upsert_body_bone(&mut switched, body_bone_sample(HumanoidBone::Head, [0.0, -0.38, 0.0, 0.925]));
		stabilizer.apply(&mut switched, &pose_quality, &MediaPipePostProcessRules::default());

		let rotation = body_bone(&switched, HumanoidBone::Head)
			.and_then(|bone| bone.transform.rotation.as_ref())
			.expect("switched head rotation");
		assert!(
			rotation.y > -0.2,
			"source switch should damp an opposite-axis one-frame head jump, got {rotation:?}"
		);
		assert!(
			switched
				.metadata
				.notes
				.iter()
				.any(|note| note == "mediapipe.stability head=source_switch from=face_matrix to=pose_world")
		);
	}

	#[test]
	fn stabilizer_respects_disabled_post_process_stability_rules() {
		let mut stabilizer = MotionStabilizer::default();
		let tracked_quality = ObservationSet {
			head: ObservationQuality::new(1.0, "face_matrix"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};
		let lost_quality = ObservationSet {
			head: ObservationQuality::new(0.0, "missing"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};
		let rules = MediaPipePostProcessRules {
			hold_lost_landmarks: false,
			ease_recovery: false,
			limit_rotation_jumps: false,
			head_source_switch_blend: false,
			..MediaPipePostProcessRules::default()
		};

		let mut first = UNMotionFrame::new(1);
		first.header.capture_timestamp_ns = 0;
		upsert_body_bone(&mut first, body_bone_sample(HumanoidBone::Head, IDENTITY_QUAT_ARRAY));
		stabilizer.apply(&mut first, &tracked_quality, &rules);

		let mut lost = UNMotionFrame::new(2);
		lost.header.capture_timestamp_ns = 100_000_000;
		stabilizer.apply(&mut lost, &lost_quality, &rules);
		assert!(body_bone(&lost, HumanoidBone::Head).is_none());

		let raw_target = euler_radians_to_quat_array(0.0, 2.8, 0.0);
		let mut jumped = UNMotionFrame::new(3);
		jumped.header.capture_timestamp_ns = 133_000_000;
		upsert_body_bone(&mut jumped, body_bone_sample(HumanoidBone::Head, raw_target));
		stabilizer.apply(&mut jumped, &tracked_quality, &rules);
		let rotation = body_bone(&jumped, HumanoidBone::Head)
			.and_then(|bone| bone.transform.rotation)
			.expect("raw head rotation");
		assert!((rotation.x - raw_target[0]).abs() < 1e-6);
		assert!((rotation.y - raw_target[1]).abs() < 1e-6);
		assert!((rotation.z - raw_target[2]).abs() < 1e-6);
		assert!((rotation.w - raw_target[3]).abs() < 1e-6);
		assert!(jumped.metadata.notes.iter().all(|note| !note.contains("mediapipe.stability head=")));
	}

	#[test]
	fn stabilizer_limits_impossible_head_rotation_jump() {
		let mut stabilizer = MotionStabilizer::default();
		let quality = ObservationSet {
			head: ObservationQuality::new(1.0, "face_matrix"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};

		let mut first = UNMotionFrame::new(1);
		first.header.capture_timestamp_ns = 0;
		upsert_body_bone(&mut first, body_bone_sample(HumanoidBone::Head, IDENTITY_QUAT_ARRAY));
		stabilizer.apply(&mut first, &quality, &MediaPipePostProcessRules::default());

		let raw_target = euler_radians_to_quat_array(0.0, 2.8, 0.0);
		let mut jumped = UNMotionFrame::new(2);
		jumped.header.capture_timestamp_ns = 33_000_000;
		upsert_body_bone(&mut jumped, body_bone_sample(HumanoidBone::Head, raw_target));
		stabilizer.apply(&mut jumped, &quality, &MediaPipePostProcessRules::default());

		let rotation = body_bone(&jumped, HumanoidBone::Head)
			.and_then(|bone| bone.transform.rotation.as_ref())
			.expect("limited head rotation");
		assert!(
			rotation.y.abs() < raw_target[1].abs() * 0.6,
			"impossible one-frame head jump should be capped, got {rotation:?}"
		);
		assert!(
			jumped
				.metadata
				.notes
				.iter()
				.any(|note| note == "mediapipe.stability head=rotation_limited")
		);
	}

	#[test]
	fn stabilizer_blends_hand_recovery_after_short_observation_loss() {
		let mut stabilizer = MotionStabilizer::default();
		let tracked_quality = ObservationSet {
			head: ObservationQuality::new(0.0, "missing"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(1.0, "test"),
		};
		let lost_quality = ObservationSet {
			head: ObservationQuality::new(0.0, "missing"),
			arms: ObservationQuality::new(0.0, "missing"),
			left_hand: ObservationQuality::new(0.0, "missing"),
			right_hand: ObservationQuality::new(0.0, "missing"),
		};
		let mut first = UNMotionFrame::new(1);
		first.header.capture_timestamp_ns = 0;
		first.right_hand = Some(test_single_joint_hand(IDENTITY_QUAT_ARRAY));
		stabilizer.apply(&mut first, &tracked_quality, &MediaPipePostProcessRules::default());

		let mut held = UNMotionFrame::new(2);
		held.header.capture_timestamp_ns = 100_000_000;
		stabilizer.apply(&mut held, &lost_quality, &MediaPipePostProcessRules::default());
		assert_eq!(
			held.right_hand.as_ref().expect("held hand").tracking_state,
			TrackingState::Recovering
		);

		let target = euler_radians_to_quat_array(0.0, 0.0, 1.2);
		let mut recovered = UNMotionFrame::new(3);
		recovered.header.capture_timestamp_ns = 200_000_000;
		recovered.right_hand = Some(test_single_joint_hand(target));
		stabilizer.apply(&mut recovered, &tracked_quality, &MediaPipePostProcessRules::default());
		let rotation = recovered.right_hand.as_ref().expect("recovered hand").fingers[0].joints[0]
			.rotation
			.as_ref()
			.expect("joint rotation");

		assert!(rotation.z > 0.0, "recovered finger should move toward the reacquired observation");
		assert!(
			rotation.z < target[2],
			"recovered finger should not snap to the raw reacquired observation"
		);
	}

	#[test]
	fn native_pose_output_emits_head_signals() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.pose.landmark_count = 9;
		native.pose.landmarks[0] = lm(0.52, 0.56);
		native.pose.landmarks[2] = lm(0.60, 0.45);
		native.pose.landmarks[5] = lm(0.40, 0.47);
		native.pose.landmarks[7] = lm(0.72, 0.50);
		native.pose.landmarks[8] = lm(0.28, 0.50);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		assert_eq!(frame.header.sequence, 7);
		assert!(frame.signals.iter().any(|signal| signal.name == "head.yaw"));
		assert_eq!(frame.sources[0].state, TrackingState::Valid);
	}

	#[test]
	fn native_pose_world_output_prefers_3d_head_signals() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.pose.landmark_count = 17;
		native.pose.world_landmark_count = 17;
		for index in [0, 2, 5, 7, 8, 11, 12] {
			native.pose.landmarks[index] = lm3(0.5, 0.5, 0.0);
		}
		native.pose.world_landmarks[0] = lm3(0.05, -0.62, -0.18);
		native.pose.world_landmarks[2] = lm3(0.03, -0.66, -0.17);
		native.pose.world_landmarks[5] = lm3(0.00, -0.66, -0.17);
		native.pose.world_landmarks[7] = lm3(0.10, -0.64, -0.08);
		native.pose.world_landmarks[8] = lm3(-0.10, -0.64, -0.08);
		native.pose.world_landmarks[11] = lm3(0.18, -0.42, -0.04);
		native.pose.world_landmarks[12] = lm3(-0.18, -0.42, -0.04);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);

		assert_signal_near(&frame.signals, "head.yaw", -0.3846154);
		assert!(scalar_value(&frame, "head.pitch").is_some());
		assert!(scalar_value(&frame, "head.roll").is_some());
		assert!(body_has_bone(&frame, HumanoidBone::Head));
	}

	#[test]
	fn native_pose_world_output_emits_arm_signals_without_hands() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.pose.landmark_count = 17;
		native.pose.world_landmark_count = 17;
		for index in [11, 12, 13, 14, 15, 16] {
			native.pose.landmarks[index] = lm3(0.5, 0.5, 0.0);
		}
		native.pose.world_landmarks[11] = lm3(0.20, -0.40, -0.04);
		native.pose.world_landmarks[13] = lm3(0.28, -0.20, -0.10);
		native.pose.world_landmarks[15] = lm3(0.39, -0.30, -0.26);
		native.pose.world_landmarks[12] = lm3(-0.10, -0.40, -0.04);
		native.pose.world_landmarks[14] = lm3(-0.14, -0.21, -0.13);
		native.pose.world_landmarks[16] = lm3(-0.08, -0.36, -0.31);

		let frame = MediaPipePostProcessor::new(MediaPipePostProcessConfig {
			hands_enabled: false,
			..MediaPipePostProcessConfig::default()
		})
		.process_native_output(&input, &native);

		assert_signal_near(&frame.signals, "arm.left.shoulder.x", -0.20);
		assert_signal_near(&frame.signals, "arm.left.shoulder.y", 0.40);
		assert_signal_near(&frame.signals, "arm.right.shoulder.x", 0.10);
		assert_signal_near(&frame.signals, "arm.right.wrist.z", 0.31);
		assert!(frame.signals.iter().all(|signal| signal.name != "hand.left.present"));
	}

	#[test]
	fn native_pose_world_output_emits_torso_legs_and_feet_when_enabled() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.pose.landmark_count = 33;
		native.pose.world_landmark_count = 33;
		for index in [11, 12, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32] {
			native.pose.landmarks[index] = lm3(0.5, 0.5, 0.0);
			native.pose.world_landmarks[index] = lm3(index as f32 * 0.01, -0.20, -0.30);
		}
		let config = MediaPipePostProcessConfig {
			torso_enabled: true,
			legs_enabled: true,
			feet_enabled: true,
			..MediaPipePostProcessConfig::default()
		};

		let frame = MediaPipePostProcessor::new(config).process_native_output(&input, &native);

		assert_signal_near(&frame.signals, "torso.left.hip.y", 0.20);
		assert_signal_near(&frame.signals, "leg.right.knee.z", 0.30);
		assert_signal_near(&frame.signals, "foot.left.index.x", -0.31);
		for bone in [
			HumanoidBone::Hips,
			HumanoidBone::Chest,
			HumanoidBone::LeftUpperLeg,
			HumanoidBone::RightUpperLeg,
			HumanoidBone::LeftLowerLeg,
			HumanoidBone::RightLowerLeg,
			HumanoidBone::LeftFoot,
			HumanoidBone::RightFoot,
		] {
			assert!(body_has_bone(&frame, bone), "missing typed body bone {bone:?}");
		}
		let hips = body_bone(&frame, HumanoidBone::Hips).expect("hips");
		let hips_rotation = hips.transform.rotation.expect("hips rotation");
		assert!(hips.transform.translation.is_none());
		assert!(
			(hips_rotation.x.abs() + hips_rotation.y.abs() + hips_rotation.z.abs()) > 0.01,
			"hips should rotate weakly when legs are enabled: {hips_rotation:?}"
		);
	}

	#[test]
	fn native_pose_world_torso_without_legs_does_not_emit_hips_bone() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.pose.landmark_count = 33;
		native.pose.world_landmark_count = 33;
		for index in [11, 12, 23, 24] {
			native.pose.landmarks[index] = lm3(0.5, 0.5, 0.0);
		}
		native.pose.world_landmarks[11] = lm3(0.20, -0.40, -0.04);
		native.pose.world_landmarks[12] = lm3(-0.20, -0.40, -0.04);
		native.pose.world_landmarks[23] = lm3(0.15, -0.80, -0.02);
		native.pose.world_landmarks[24] = lm3(-0.15, -0.80, -0.02);
		let config = MediaPipePostProcessConfig {
			torso_enabled: true,
			legs_enabled: false,
			feet_enabled: false,
			..MediaPipePostProcessConfig::default()
		};

		let frame = MediaPipePostProcessor::new(config).process_native_output(&input, &native);

		assert_signal_near(&frame.signals, "torso.left.hip.y", 0.80);
		assert!(body_has_bone(&frame, HumanoidBone::Chest));
		assert!(!body_has_bone(&frame, HumanoidBone::Hips));
		assert!(!body_has_bone(&frame, HumanoidBone::LeftUpperLeg));
		assert!(!body_has_bone(&frame, HumanoidBone::LeftFoot));
		assert!(
			frame
				.body
				.as_ref()
				.and_then(|body| body.humanoid.as_ref())
				.and_then(|humanoid| humanoid.root.as_ref())
				.is_none()
		);
	}

	#[test]
	fn native_pose_world_torso_yaw_rotates_chest_not_hips() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.pose.landmark_count = 33;
		native.pose.world_landmark_count = 33;
		for index in [11, 12, 23, 24, 25, 26] {
			native.pose.landmarks[index] = lm3(0.5, 0.5, 0.0);
		}
		native.pose.world_landmarks[11] = lm3(0.20, -0.40, 0.20);
		native.pose.world_landmarks[12] = lm3(-0.20, -0.40, -0.20);
		native.pose.world_landmarks[23] = lm3(0.15, -0.80, 0.00);
		native.pose.world_landmarks[24] = lm3(-0.15, -0.80, 0.00);
		native.pose.world_landmarks[25] = lm3(0.15, -1.20, 0.00);
		native.pose.world_landmarks[26] = lm3(-0.15, -1.20, 0.00);
		let config = MediaPipePostProcessConfig {
			torso_enabled: true,
			legs_enabled: true,
			feet_enabled: false,
			..MediaPipePostProcessConfig::default()
		};

		let frame = MediaPipePostProcessor::new(config).process_native_output(&input, &native);
		let hips = body_bone(&frame, HumanoidBone::Hips).expect("hips");
		let hips_rotation = hips.transform.rotation.expect("hips rotation");
		let chest = body_bone(&frame, HumanoidBone::Chest).expect("chest");
		let chest_rotation = chest.transform.rotation.expect("chest rotation");

		assert!(
			(hips_rotation.x.abs() + hips_rotation.y.abs() + hips_rotation.z.abs()) < 0.50,
			"hips should be damped, not fully driven by webcam hip yaw: {hips_rotation:?}"
		);
		assert!(
			(chest_rotation.x.abs() + chest_rotation.y.abs() + chest_rotation.z.abs()) > 0.10,
			"torso yaw should be represented by Chest, not Hips: {chest_rotation:?}"
		);
	}

	#[test]
	fn empty_native_output_marks_source_lost() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let native = NativeMediaPipeOutput::default();
		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		assert!(frame.signals.is_empty());
		assert_eq!(frame.sources[0].state, TrackingState::Lost);
	}

	#[test]
	fn native_hand_output_emits_presence_signal() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput {
			hands: NativeHands {
				hand_count: 1,
				..NativeHands::default()
			},
			..NativeMediaPipeOutput::default()
		};
		native.hands.hands[0].landmark_count = HAND_LANDMARK_COUNT as u32;
		native.hands.hands[0].handedness_is_right = 1;
		native.hands.hands[0].handedness_score = 0.9;

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		assert!(frame.signals.iter().any(|signal| signal.name == "hand.right.present"));
	}

	#[test]
	fn native_hand_output_populates_typed_hand_motion() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 320, 240, vec![0; 320 * 240 * 3]).unwrap();
		let mut native = NativeMediaPipeOutput {
			hands: NativeHands {
				hand_count: 1,
				..NativeHands::default()
			},
			..NativeMediaPipeOutput::default()
		};
		native.hands.hands[0].landmark_count = HAND_LANDMARK_COUNT as u32;
		native.hands.hands[0].handedness_is_right = 1;
		native.hands.hands[0].handedness_score = 0.9;
		for index in 0..HAND_LANDMARK_COUNT {
			native.hands.hands[0].landmarks[index] = lm3(0.45 + index as f32 * 0.003, 0.55 - index as f32 * 0.004, 0.01);
		}

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		let right = frame.right_hand.expect("right hand motion");
		assert!(
			right.wrist.is_none(),
			"typed hand must not overwrite arm/IK-owned humanoid hand bone"
		);
		assert_eq!(right.fingers.len(), 5);
		assert!(right.fingers.iter().all(|finger| finger.joints.len() == 3));
	}

	#[test]
	fn pose_arm_output_neutralizes_when_hands_are_missing() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 320, 240, vec![0; 320 * 240 * 3]).unwrap();
		let mut native = NativeMediaPipeOutput {
			pose: NativePose {
				landmark_count: 17,
				world_landmark_count: 17,
				..NativePose::default()
			},
			..NativeMediaPipeOutput::default()
		};
		for (index, landmark) in [
			(11, lm3(0.40, 0.40, 0.0)),
			(13, lm3(0.35, 0.55, 0.0)),
			(15, lm3(0.30, 0.70, 0.0)),
			(12, lm3(0.60, 0.40, 0.0)),
			(14, lm3(0.65, 0.55, 0.0)),
			(16, lm3(0.70, 0.70, 0.0)),
		] {
			native.pose.landmarks[index] = landmark;
			native.pose.world_landmarks[index] = landmark;
		}

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);

		for (bone, expected) in [
			(
				HumanoidBone::LeftUpperArm,
				rest_pose_upper_arm_rotation(HandSide::Left, MediaPipePostProcessRules::default().lost_signal_rest_pose_blend),
			),
			(HumanoidBone::LeftLowerArm, IDENTITY_QUAT_ARRAY),
			(HumanoidBone::LeftHand, IDENTITY_QUAT_ARRAY),
			(
				HumanoidBone::RightUpperArm,
				rest_pose_upper_arm_rotation(HandSide::Right, MediaPipePostProcessRules::default().lost_signal_rest_pose_blend),
			),
			(HumanoidBone::RightLowerArm, IDENTITY_QUAT_ARRAY),
			(HumanoidBone::RightHand, IDENTITY_QUAT_ARRAY),
		] {
			let sample = body_bone(&frame, bone).expect("rest-pose arm/hand bone");
			assert_eq!(sample.state, SampleState::Valid);
			let rotation = sample.transform.rotation.expect("rest-pose rotation");
			assert_quat_near(&rotation, expected, 1e-5);
		}
		assert!(
			frame
				.metadata
				.notes
				.iter()
				.any(|note| note.contains("arms=") && note.contains("pose_chain_hands_missing"))
		);
		assert!(
			frame
				.metadata
				.notes
				.iter()
				.any(|note| note == "mediapipe.stability left_arm=rest_pose_hands_missing")
		);
	}

	#[test]
	fn hand_ik_arm_quality_keeps_hand_inferred_arms_live_when_pose_chain_is_missing() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 320, 240, vec![0; 320 * 240 * 3]).unwrap();
		let mut native = NativeMediaPipeOutput {
			hands: NativeHands {
				hand_count: 1,
				..NativeHands::default()
			},
			..NativeMediaPipeOutput::default()
		};
		native.hands.hands[0].landmark_count = HAND_LANDMARK_COUNT as u32;
		native.hands.hands[0].handedness_is_right = 1;
		native.hands.hands[0].handedness_score = 1.0;
		for index in 0..HAND_LANDMARK_COUNT {
			native.hands.hands[0].landmarks[index] = lm3(0.45 + index as f32 * 0.003, 0.55 - index as f32 * 0.004, 0.01);
		}

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);

		assert!(body_has_bone(&frame, HumanoidBone::RightUpperArm));
		assert!(body_has_bone(&frame, HumanoidBone::RightLowerArm));
		assert!(frame.metadata.notes.iter().any(|note| note.contains("arms=0.700(hand_ik)")));
		assert!(
			frame.metadata.notes.iter().all(|note| !note.contains("right_arm=held")),
			"hand-inferred arm should remain live instead of being overwritten by stale hold"
		);
	}

	#[test]
	fn typed_index_intermediate_rotation_uses_local_roll_axis() {
		let rotation = finger_joint_rotation("right", "index", "Intermediate", 0.8, 0.2);
		let expected_z = (-0.8_f32 * 0.5).sin();
		let expected_w = (-0.8_f32 * 0.5).cos();
		assert!(rotation.x.abs() < 1e-5);
		assert!(rotation.y.abs() < 1e-5);
		assert!((rotation.z - expected_z).abs() < 1e-5);
		assert!((rotation.w - expected_w).abs() < 1e-5);
	}

	#[test]
	fn typed_proximal_rotation_applies_direct_spread_angle() {
		let rotation = finger_joint_rotation("left", "index", "Proximal", 0.4, -0.2);
		assert!(rotation.y < -0.08, "proximal spread should stay on local yaw: {rotation:?}");
		assert!(rotation.z > 0.15, "curl should stay on local roll: {rotation:?}");
	}

	#[test]
	fn lower_arm_tracks_hand_palm_twist_without_changing_direction() {
		let signals = vec![
			scalar("arm.left.elbow.x", 0.0, 1.0),
			scalar("arm.left.elbow.y", 0.0, 1.0),
			scalar("arm.left.elbow.z", 0.0, 1.0),
			scalar("arm.left.wrist.x", -1.0, 1.0),
			scalar("arm.left.wrist.y", 0.0, 1.0),
			scalar("arm.left.wrist.z", 0.0, 1.0),
			scalar("hand.left.palm.forward.x", -1.0, 1.0),
			scalar("hand.left.palm.forward.y", 0.0, 1.0),
			scalar("hand.left.palm.forward.z", 0.0, 1.0),
			scalar("hand.left.palm.across.x", 0.0, 1.0),
			scalar("hand.left.palm.across.y", 1.0, 1.0),
			scalar("hand.left.palm.across.z", 0.0, 1.0),
			scalar("hand.left.palm.normal.x", 0.0, 1.0),
			scalar("hand.left.palm.normal.y", 0.0, 1.0),
			scalar("hand.left.palm.normal.z", 1.0, 1.0),
		];

		let lower = lower_arm_global_rotation_from_signals(&signals, HandSide::Left).expect("lower arm rotation");
		let hand = hand_local_rotation_from_signals(&signals, HandSide::Left).expect("hand local rotation");

		assert!(
			lower[0].abs() > 0.6,
			"forearm should carry wrist-axis palm twist while preserving elbow-to-wrist direction: {lower:?}"
		);
		assert!(
			hand[0].abs() < 1e-5 && hand[1].abs() < 1e-5 && hand[2].abs() < 1e-5,
			"matching palm twist should not remain as visible hand-local wrist break: {hand:?}"
		);
	}

	#[test]
	fn upper_arm_uses_forearm_direction_as_elbow_bend_plane() {
		let signals = vec![
			scalar("arm.left.shoulder.x", 0.0, 1.0),
			scalar("arm.left.shoulder.y", 0.0, 1.0),
			scalar("arm.left.shoulder.z", 0.0, 1.0),
			scalar("arm.left.elbow.x", -1.0, 1.0),
			scalar("arm.left.elbow.y", 0.0, 1.0),
			scalar("arm.left.elbow.z", 0.0, 1.0),
			scalar("arm.left.wrist.x", -1.0, 1.0),
			scalar("arm.left.wrist.y", 1.0, 1.0),
			scalar("arm.left.wrist.z", 0.0, 1.0),
		];

		let upper = upper_arm_local_rotation_from_signals(&signals, HandSide::Left).expect("upper arm rotation");
		assert!(
			upper[0].abs() > 0.6,
			"upper arm should carry elbow bend-plane twist instead of leaving it undefined: {upper:?}"
		);
	}

	#[test]
	fn typed_thumb_rotation_uses_local_yaw_axis() {
		let left = finger_joint_rotation("left", "thumb", "Proximal", 0.6, 0.0);
		let right = finger_joint_rotation("right", "thumb", "Proximal", 0.6, 0.0);
		assert!(left.y > 0.25, "left thumb curl should use +Y axis: {left:?}");
		assert!(right.y < -0.25, "right thumb curl should mirror on Y axis: {right:?}");
		assert!(
			left.z.abs() < 1e-5 && right.z.abs() < 1e-5,
			"thumb curl must not use four-finger roll axis"
		);
	}

	#[test]
	fn typed_head_pitch_uses_avatar_x_axis_sign() {
		let rotation = head_rotation_from_signals(&[scalar("head.pitch", 1.0, 1.0)])
			.expect("head rotation")
			.0;
		assert!(
			rotation[0] < -0.30,
			"positive head.pitch should map to avatar look-up X sign: {rotation:?}"
		);
	}

	#[test]
	fn typed_head_roll_uses_avatar_z_axis_sign() {
		let rotation = head_rotation_from_signals(&[scalar("head.roll", 1.0, 1.0)])
			.expect("head rotation")
			.0;
		assert!(rotation[2] < -0.25, "positive head.roll should map to avatar Z sign: {rotation:?}");
	}

	#[test]
	fn joint_curl_returns_radians_without_empirical_gate() {
		let straight = joint_curl(p3(0.0, 0.0, 0.0), p3(1.0, 0.0, 0.0), p3(2.0, 0.0, 0.0));
		assert!(straight.abs() < 1e-5);
		let right_angle = joint_curl(p3(0.0, 0.0, 0.0), p3(1.0, 0.0, 0.0), p3(1.0, 1.0, 0.0));
		assert!((right_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
	}

	#[test]
	fn joint_curl_in_plane_ignores_lateral_abduction() {
		let flexion_plane_normal = p3(1.0, 0.0, 0.0);
		let curl = joint_curl_in_plane(p3(0.0, -1.0, 0.0), p3(0.0, 0.0, 0.0), p3(0.7, 1.0, 0.0), flexion_plane_normal);
		assert!(curl.abs() < 1e-5, "lateral spread alone should not become curl: {curl}");
		let curled = joint_curl_in_plane(p3(0.0, -1.0, 0.0), p3(0.0, 0.0, 0.0), p3(0.0, 0.7, -0.7), flexion_plane_normal);
		assert!(curled > 0.5, "motion inside the flexion plane should remain curl: {curled}");
	}

	#[test]
	fn finger_chord_limits_open_finger_local_zigzag() {
		let mut landmarks = [p3(0.0, 0.0, 0.0); HAND_LANDMARK_COUNT];
		landmarks[5] = p3(0.0, 0.0, 0.0);
		landmarks[6] = p3(0.0, 1.0, 0.0);
		landmarks[7] = p3(0.0, 2.0, 0.0);
		landmarks[8] = p3(0.0, 2.95, 0.0);
		let limited = limit_joint_curls_by_finger_chord(&landmarks, [5, 6, 7, 8], [0.4, 0.4, 0.4]);
		assert!(
			limited.iter().sum::<f32>() < 0.2,
			"nearly straight endpoint geometry should suppress local zigzag curls: {limited:?}"
		);
	}

	#[test]
	fn compact_finger_chord_restores_occluded_fist_curl() {
		let mut landmarks = [p3(0.0, 0.0, 0.0); HAND_LANDMARK_COUNT];
		landmarks[5] = p3(0.0, 0.0, 0.0);
		landmarks[6] = p3(0.0, 1.0, 0.0);
		landmarks[7] = p3(0.0, 1.4, -0.85);
		landmarks[8] = p3(0.0, 0.45, -1.05);

		let raw = [0.3, 0.35, 0.25];
		let reconciled = reconcile_joint_curls_with_finger_chord(&landmarks, [5, 6, 7, 8], raw);
		assert!(
			reconciled.iter().sum::<f32>() > raw.iter().sum::<f32>() * 1.8,
			"compact fingertip-to-mcp chord should infer hidden fist curl: {reconciled:?}"
		);
	}

	#[test]
	fn compact_finger_curl_is_anatomically_distributed() {
		let reconciled = distribute_total_finger_curl([0.1, 2.7, 0.05], 3.6);
		assert!(reconciled[0] > 0.8, "MCP should share compact fist curl: {reconciled:?}");
		assert!(reconciled[1] < 1.76, "PIP should stay anatomically bounded: {reconciled:?}");
		assert!(reconciled[2] > 0.5, "DIP should share compact fist curl: {reconciled:?}");
	}

	#[test]
	fn near_straight_finger_chord_does_not_invent_peace_sign_curl() {
		let mut landmarks = [p3(0.0, 0.0, 0.0); HAND_LANDMARK_COUNT];
		landmarks[5] = p3(0.0, 0.0, 0.0);
		landmarks[6] = p3(0.0, 1.0, 0.0);
		landmarks[7] = p3(0.0, 2.0, 0.0);
		landmarks[8] = p3(0.0, 2.95, 0.0);

		let raw = [0.02, 0.03, 0.04];
		let reconciled = reconcile_joint_curls_with_finger_chord(&landmarks, [5, 6, 7, 8], raw);
		assert!(
			reconciled.iter().sum::<f32>() < 0.12,
			"near-straight visible fingers should stay open: {reconciled:?}"
		);
	}

	#[test]
	fn finger_spread_uses_signed_palm_plane_angle() {
		let landmarks = spread_test_landmarks();
		let index = finger_spread_radians(&landmarks, "index", 5, 6, "left");
		let middle = finger_spread_radians(&landmarks, "middle", 9, 10, "left");
		let little = finger_spread_radians(&landmarks, "little", 17, 18, "left");
		assert!(index < -0.15, "index should abduct opposite little finger: {index}");
		assert!(middle.abs() < 1e-5, "middle is the palm-plane spread reference: {middle}");
		assert!(little > 0.15, "little should abduct away from the middle finger: {little}");
	}

	#[test]
	fn right_hand_spread_uses_mirrored_local_yaw() {
		let landmarks = spread_test_landmarks();
		let left_little = finger_spread_radians(&landmarks, "little", 17, 18, "left");
		let right_little = finger_spread_radians(&landmarks, "little", 17, 18, "right");
		assert!(
			(left_little + right_little).abs() < 1e-5,
			"right hand local yaw must mirror left hand local yaw"
		);
	}

	#[test]
	fn thumb_spread_is_not_mapped_to_four_finger_abduction_axis() {
		let landmarks = spread_test_landmarks();
		let thumb = finger_spread_radians(&landmarks, "thumb", 1, 2, "left");
		assert!(
			thumb.abs() < 1e-5,
			"thumb opposition needs a thumb-specific basis, not middle-finger abduction"
		);
	}

	#[test]
	fn native_face_without_eye_blendshapes_emits_neutral_eye_signals() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 0.9;
		native.face.matrix_rows = 4;
		native.face.matrix_cols = 4;
		native.face.matrix[0] = 1.0;
		native.face.matrix[5] = 1.0;
		native.face.matrix[10] = 1.0;

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		assert!(frame.signals.iter().any(|signal| signal.name == "eye.left.yaw"));
		assert!(frame.signals.iter().any(|signal| signal.name == "eye.right.pitch"));
	}

	#[test]
	fn native_face_blendshapes_populate_typed_face_motion() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 0.9;
		native.face.matrix_rows = 4;
		native.face.matrix_cols = 4;
		native.face.matrix[0] = 1.0;
		native.face.matrix[5] = 1.0;
		native.face.matrix[10] = 1.0;
		native.face.blendshape_count = 1;
		native.face.blendshapes[0] = NativeFaceBlendshape {
			name: fixed_name_bytes("mouthSmileLeft"),
			score: 0.7,
		};

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		let face = frame.face.expect("typed face motion");
		assert!(
			face.head.is_some(),
			"face motion should carry head transform when head signals exist"
		);
		let smile = face
			.expressions
			.iter()
			.find(|sample| sample.name == "mouthSmileLeft")
			.expect("mouthSmileLeft expression");
		assert!((smile.value - 0.7).abs() < 0.0001);
	}

	#[test]
	fn eye_open_bias_remaps_only_blink_and_wide_blendshapes() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 0.9;
		native.face.matrix_rows = 4;
		native.face.matrix_cols = 4;
		native.face.matrix[0] = 1.0;
		native.face.matrix[5] = 1.0;
		native.face.matrix[10] = 1.0;
		native.face.blendshape_count = 4;
		native.face.blendshapes[0] = NativeFaceBlendshape {
			name: fixed_name_bytes("eyeBlinkLeft"),
			score: 0.12,
		};
		native.face.blendshapes[1] = NativeFaceBlendshape {
			name: fixed_name_bytes("eyeBlinkRight"),
			score: 0.40,
		};
		native.face.blendshapes[2] = NativeFaceBlendshape {
			name: fixed_name_bytes("eyeWideLeft"),
			score: 0.02,
		};
		native.face.blendshapes[3] = NativeFaceBlendshape {
			name: fixed_name_bytes("mouthSmileLeft"),
			score: 0.60,
		};
		let mut processor = MediaPipePostProcessor::new(MediaPipePostProcessConfig {
			eye_open_bias: 1.0,
			..MediaPipePostProcessConfig::default()
		});

		let frame = processor.process_native_output(&input, &native);

		assert_signal_near(&frame.signals, "face.eyeBlinkLeft", 0.0);
		assert_signal_near(&frame.signals, "face.eyeBlinkRight", 0.0);
		assert_signal_near(&frame.signals, "face.eyeWideLeft", 0.02 + 0.60 * 0.98);
		assert_signal_near(&frame.signals, "face.mouthSmileLeft", 0.60);
	}

	fn spread_test_landmarks() -> [Point3; HAND_LANDMARK_COUNT] {
		let mut landmarks = [p3(0.0, 0.0, 0.0); HAND_LANDMARK_COUNT];
		landmarks[0] = p3(0.0, 0.0, 0.0);
		landmarks[5] = p3(-0.25, 1.0, 0.0);
		landmarks[6] = p3(-0.45, 2.0, 0.0);
		landmarks[9] = p3(0.0, 1.05, 0.0);
		landmarks[10] = p3(0.0, 2.05, 0.0);
		landmarks[17] = p3(0.25, 1.0, 0.0);
		landmarks[18] = p3(0.45, 2.0, 0.0);
		landmarks
	}

	fn p3(x: f32, y: f32, z: f32) -> Point3 {
		Point3 { x, y, z }
	}

	#[test]
	fn native_pose_head_fallback_keeps_eyes_valid_when_face_is_missing() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.pose.landmark_count = 9;
		native.pose.landmarks[0] = lm(0.50, 0.56);
		native.pose.landmarks[2] = lm(0.60, 0.45);
		native.pose.landmarks[5] = lm(0.40, 0.47);
		native.pose.landmarks[7] = lm(0.72, 0.50);
		native.pose.landmarks[8] = lm(0.28, 0.50);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		assert!(frame.signals.iter().any(|signal| signal.name == "eye.left.yaw"));
		assert!(frame.signals.iter().any(|signal| signal.name == "eye.right.pitch"));
	}

	#[test]
	fn saturated_native_face_head_falls_back_to_pose_head() {
		let input = ImageFrame::new_rgb8(7, 123, "file:test", 1, 1, vec![0, 0, 0]).unwrap();
		let mut native = NativeMediaPipeOutput::default();
		native.face.landmark_count = FACE_LANDMARK_COUNT as u32;
		native.face.confidence = 1.0;
		native.face.matrix_rows = 4;
		native.face.matrix_cols = 4;
		native.face.matrix[0] = 1.0;
		native.face.matrix[5] = 1.0;
		native.face.matrix[6] = 1.0;
		native.face.matrix[10] = 1.0;
		native.pose.landmark_count = 9;
		native.pose.landmarks[0] = lm(0.50, 0.45);
		native.pose.landmarks[2] = lm(0.60, 0.50);
		native.pose.landmarks[5] = lm(0.40, 0.50);
		native.pose.landmarks[7] = lm(0.72, 0.50);
		native.pose.landmarks[8] = lm(0.28, 0.50);

		let frame = MediaPipePostProcessor::default().process_native_output(&input, &native);
		let pitch = scalar_value(&frame, "head.pitch").expect("head pitch");
		assert!(pitch.abs() < 0.3);
	}

	#[test]
	fn camera_model_uses_normalized_focal_length() {
		let camera = camera_model(320, 240, 120.0);
		assert!(camera.focal_diag > 0.1);
		assert!(camera.focal_diag < 2.0);
	}

	#[test]
	fn native_hand_camera_target_does_not_saturate_depth_for_typical_palm() {
		let config = MediaPipePostProcessConfig {
			input_width: 320,
			input_height: 240,
			camera_diagonal_view_angle_deg: 120.0,
			..MediaPipePostProcessConfig::default()
		};
		let mut landmarks = [Point3 { x: 0.5, y: 0.5, z: 0.0 }; HAND_LANDMARK_COUNT];
		landmarks[0] = Point3 { x: 0.5, y: 0.62, z: 0.0 };
		landmarks[5] = Point3 { x: 0.43, y: 0.50, z: 0.0 };
		landmarks[9] = Point3 { x: 0.5, y: 0.44, z: 0.0 };
		landmarks[13] = Point3 { x: 0.57, y: 0.50, z: 0.0 };
		landmarks[17] = Point3 { x: 0.63, y: 0.56, z: 0.0 };

		let target = hand_camera_target("right", &landmarks, &config);
		assert!(target.z > -0.9);
		assert!(target.z < 0.9);
		assert!(target.x.abs() < 0.5);
	}

	fn lm(x: f32, y: f32) -> NativeLandmark {
		lm3(x, y, 0.0)
	}

	fn lm3(x: f32, y: f32, z: f32) -> NativeLandmark {
		NativeLandmark {
			x,
			y,
			z,
			visibility: 1.0,
			presence: 1.0,
		}
	}

	fn config_with_mirror(mirror_mode: &str) -> MediaPipePostProcessConfig {
		MediaPipePostProcessConfig {
			mirror_mode: mirror_mode.to_string(),
			..MediaPipePostProcessConfig::default()
		}
	}

	fn scalar_value(frame: &UNMotionFrame, name: &str) -> Option<f32> {
		frame.signals.iter().find_map(|signal| {
			if signal.name != name {
				return None;
			}
			match signal.value {
				MotionSignalValue::Scalar(value) => Some(value),
				_ => None,
			}
		})
	}

	fn test_single_joint_hand(rotation: [f32; 4]) -> HandMotion {
		HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			wrist: None,
			fingers: vec![FingerPose {
				finger: Finger::Index,
				joints: vec![TransformSample {
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
				}],
				confidence: 1.0,
			}],
		}
	}

	fn body_has_bone(frame: &UNMotionFrame, bone: HumanoidBone) -> bool {
		frame
			.body
			.as_ref()
			.and_then(|body| body.humanoid.as_ref())
			.map(|humanoid| humanoid.bones.iter().any(|sample| sample.bone == bone))
			.unwrap_or(false)
	}

	fn assert_signal_near(signals: &[MotionSignal], name: &str, expected: f32) {
		let value = signal_value(signals, name)
			.map(|(value, _confidence)| value)
			.unwrap_or_else(|| panic!("missing signal {name}"));
		assert!((value - expected).abs() < 0.0001, "signal {name} expected {expected}, got {value}");
	}

	fn assert_quat_near(actual: &Quatf, expected: [f32; 4], epsilon: f32) {
		assert!(
			(actual.x - expected[0]).abs() <= epsilon,
			"quat x expected {}, got {}",
			expected[0],
			actual.x
		);
		assert!(
			(actual.y - expected[1]).abs() <= epsilon,
			"quat y expected {}, got {}",
			expected[1],
			actual.y
		);
		assert!(
			(actual.z - expected[2]).abs() <= epsilon,
			"quat z expected {}, got {}",
			expected[2],
			actual.z
		);
		assert!(
			(actual.w - expected[3]).abs() <= epsilon,
			"quat w expected {}, got {}",
			expected[3],
			actual.w
		);
	}
}
