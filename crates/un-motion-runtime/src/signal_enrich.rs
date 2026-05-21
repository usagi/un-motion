use un_motion_frame::{Finger, FingerPose, HandMotion, MotionSignalValue, Quatf, TrackingState, TransformSample, UNMotionFrame, Vec3f};

pub(crate) fn enrich_frame_with_signal_derived_motion(frame: &mut UNMotionFrame) -> bool {
	// This runtime hook is only a compatibility fallback for frames that do not
	// yet carry first-class hand motion. Body/face construction belongs to the
	// engine post-process stage, not to VMC-derived output conversion.
	enrich_frame_with_signal_derived_hands(frame)
}

fn enrich_frame_with_signal_derived_hands(frame: &mut UNMotionFrame) -> bool {
	let mut inserted = false;
	if frame.left_hand.is_none() {
		if let Some(hand) = signal_derived_hand(frame, "left") {
			frame.left_hand = Some(hand);
			inserted = true;
		}
	}
	if frame.right_hand.is_none() {
		if let Some(hand) = signal_derived_hand(frame, "right") {
			frame.right_hand = Some(hand);
			inserted = true;
		}
	}
	inserted
}

fn signal_derived_hand(frame: &UNMotionFrame, side: &str) -> Option<HandMotion> {
	let present = scalar_signal(frame, &format!("hand.{side}.present")).unwrap_or(0.0);
	let has_wrist = scalar_signal(frame, &format!("hand.{side}.wrist.x")).is_some()
		|| scalar_signal(frame, &format!("hand.{side}.wrist.y")).is_some()
		|| scalar_signal(frame, &format!("hand.{side}.wrist.z")).is_some();
	let has_fingers = ["thumb", "index", "middle", "ring", "little"]
		.into_iter()
		.any(|finger| scalar_signal(frame, &format!("hand.{side}.{finger}.curl")).is_some());
	if present <= 0.0 && !has_wrist && !has_fingers {
		return None;
	}

	Some(HandMotion {
		tracking_state: TrackingState::Valid,
		confidence: signal_confidence(frame, &format!("hand.{side}.present")).unwrap_or(1.0),
		wrist: signal_derived_wrist(frame, side),
		fingers: signal_derived_finger_poses(frame, side),
	})
}

fn signal_derived_wrist(frame: &UNMotionFrame, side: &str) -> Option<TransformSample> {
	let translation = match (
		scalar_signal(frame, &format!("hand.{side}.wrist.x")),
		scalar_signal(frame, &format!("hand.{side}.wrist.y")),
		scalar_signal(frame, &format!("hand.{side}.wrist.z")),
	) {
		(Some(x), Some(y), Some(z)) => Some(Vec3f { x, y, z }),
		_ => None,
	};
	let rotation = match (
		scalar_signal(frame, &format!("hand.{side}.wrist.pitch")),
		scalar_signal(frame, &format!("hand.{side}.wrist.yaw")),
		scalar_signal(frame, &format!("hand.{side}.wrist.roll")),
	) {
		(Some(pitch), Some(yaw), Some(roll)) => Some(euler_radians_to_quatf(pitch * 0.65, yaw * 0.85, roll * 0.55)),
		_ => None,
	};
	if translation.is_none() && rotation.is_none() {
		return None;
	}
	Some(TransformSample {
		translation,
		rotation,
		scale: None,
		linear_velocity: None,
		angular_velocity: None,
	})
}

fn signal_derived_finger_poses(frame: &UNMotionFrame, side: &str) -> Vec<FingerPose> {
	let sibling_fold = hand_finger_fold(frame, side);
	[
		(Finger::Thumb, "thumb"),
		(Finger::Index, "index"),
		(Finger::Middle, "middle"),
		(Finger::Ring, "ring"),
		(Finger::Little, "little"),
	]
	.into_iter()
	.filter_map(|(finger, finger_name)| {
		let finger_curl = scalar_signal(frame, &format!("hand.{side}.{finger_name}.curl")).unwrap_or(0.0);
		let spread = scalar_signal(frame, &format!("hand.{side}.{finger_name}.spread")).unwrap_or(0.0);
		let has_any_joint = ["mcp", "pip", "dip"]
			.into_iter()
			.any(|joint| scalar_signal(frame, &format!("hand.{side}.{finger_name}.{joint}.curl")).is_some());
		if finger_curl == 0.0 && spread == 0.0 && !has_any_joint {
			return None;
		}
		let joints = [("Proximal", "mcp"), ("Intermediate", "pip"), ("Distal", "dip")]
			.into_iter()
			.map(|(segment, joint)| {
				let joint_curl = scalar_signal(frame, &format!("hand.{side}.{finger_name}.{joint}.curl")).unwrap_or(finger_curl);
				let rest = format!("{}{}", finger_rest_prefix(finger), segment);
				let factor = adjusted_finger_factor(
					finger_name,
					&rest,
					side,
					finger_curl,
					sibling_fold,
					joint_curl,
					base_finger_factor(finger_name, segment),
				);
				TransformSample {
					translation: None,
					rotation: Some(finger_curl_to_quatf(joint_curl, spread, factor, &rest, side, finger_curl)),
					scale: None,
					linear_velocity: None,
					angular_velocity: None,
				}
			})
			.collect();
		Some(FingerPose {
			finger,
			joints,
			confidence: signal_confidence(frame, &format!("hand.{side}.{finger_name}.curl")).unwrap_or(1.0),
		})
	})
	.collect()
}

fn hand_finger_fold(frame: &UNMotionFrame, side: &str) -> f32 {
	["index", "middle", "ring", "little"]
		.into_iter()
		.map(|finger| {
			scalar_signal(frame, &format!("hand.{side}.{finger}.curl"))
				.unwrap_or(0.0)
				.clamp(0.0, 1.0)
		})
		.sum::<f32>()
		/ 4.0
}

fn finger_rest_prefix(finger: Finger) -> &'static str {
	match finger {
		Finger::Thumb => "Thumb",
		Finger::Index => "Index",
		Finger::Middle => "Middle",
		Finger::Ring => "Ring",
		Finger::Little => "Little",
	}
}

fn base_finger_factor(finger: &str, segment: &str) -> f32 {
	match (finger, segment) {
		("thumb", "Proximal") => 0.35,
		("thumb", "Intermediate") => 0.25,
		("thumb", "Distal") => 0.25,
		("index", "Proximal") => 0.75,
		("index", "Intermediate") => 1.05,
		("index", "Distal") => 0.35,
		(_, "Proximal") => 1.35,
		(_, "Intermediate") => 1.25,
		(_, "Distal") => 0.9,
		_ => 0.0,
	}
}

fn adjusted_finger_factor(
	finger: &str,
	rest: &str,
	side: &str,
	finger_curl: f32,
	sibling_fold: f32,
	joint_curl: f32,
	fallback: f32,
) -> f32 {
	if finger != "index" {
		return fallback;
	}

	let left = side == "left";
	let thumb_grip = sibling_fold > 0.30 && sibling_fold < 0.55 && (finger_curl > 0.25 || joint_curl > 0.80);
	if thumb_grip {
		match rest {
			"IndexProximal" => 1.25,
			"IndexIntermediate" => 1.85,
			"IndexDistal" if !left => 1.6,
			"IndexDistal" => 4.6,
			_ => fallback,
		}
	} else if finger_curl > 0.60 && sibling_fold > 0.55 {
		match rest {
			"IndexProximal" => 1.4,
			"IndexIntermediate" => 1.8,
			"IndexDistal" if !left => 1.0,
			"IndexDistal" => 1.45,
			_ => fallback,
		}
	} else {
		fallback
	}
}

fn finger_curl_to_quatf(curl: f32, spread: f32, factor: f32, rest: &str, side: &str, finger_curl: f32) -> Quatf {
	if rest == "ThumbProximal" && is_thumb_opposition_case(curl, spread, side, finger_curl) {
		return thumb_proximal_to_quatf(curl, spread, side);
	}
	let curl_angle = curl.clamp(0.0, 1.0) * factor * side_sign(side);
	let spread_angle = if rest.ends_with("Proximal") {
		finger_spread_angle(spread, rest, side)
	} else {
		0.0
	};
	euler_radians_to_quatf(0.0, spread_angle, curl_angle)
}

fn is_thumb_opposition_case(curl: f32, spread: f32, side: &str, finger_curl: f32) -> bool {
	curl > 0.25
		&& match side {
			"left" => spread > 0.35,
			"right" => spread < -0.35 && finger_curl < 0.60,
			_ => false,
		}
}

fn thumb_proximal_to_quatf(curl: f32, spread: f32, side: &str) -> Quatf {
	let opposition_angle = curl.clamp(0.0, 1.0) * 0.65;
	let abduction_angle = -spread.clamp(-1.0, 1.0) * 0.60;
	let curl_angle = curl.clamp(0.0, 1.0) * 0.12 * side_sign(side);
	euler_radians_to_quatf(opposition_angle, abduction_angle, curl_angle)
}

fn finger_spread_angle(spread: f32, rest: &str, side: &str) -> f32 {
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
	-spread * factor
}

fn canonical_finger_spread(spread: f32, side: &str) -> f32 {
	let spread = spread.clamp(-1.0, 1.0);
	if side == "right" { -spread } else { spread }
}

fn side_sign(side: &str) -> f32 {
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

fn signal_confidence(frame: &UNMotionFrame, name: &str) -> Option<f32> {
	frame
		.signals
		.iter()
		.find(|signal| signal.name == name)
		.map(|signal| signal.confidence.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_frame::{CoordinateSpace, MotionSignal, SampleState};

	fn scalar(name: &str, value: f32) -> MotionSignal {
		MotionSignal {
			name: name.to_string(),
			value: MotionSignalValue::Scalar(value),
			confidence: 0.9,
			source_index: Some(0),
			state: SampleState::Valid,
		}
	}

	#[test]
	fn enriches_signal_hand_fingers_into_first_class_unmotion_frame_hand_motion() {
		let mut frame = UNMotionFrame::new(10);
		frame.signals = vec![
			scalar("hand.right.present", 1.0),
			scalar("hand.right.wrist.x", 0.1),
			scalar("hand.right.wrist.y", 0.2),
			scalar("hand.right.wrist.z", 0.3),
			scalar("hand.right.index.curl", 0.8),
			scalar("hand.right.index.pip.curl", 0.8),
			scalar("hand.right.index.spread", 0.2),
		];

		assert!(enrich_frame_with_signal_derived_motion(&mut frame));

		let hand = frame.right_hand.expect("right hand should be enriched from signals");
		assert_eq!(hand.tracking_state, TrackingState::Valid);
		assert_eq!(hand.wrist.as_ref().and_then(|wrist| wrist.translation).unwrap().x, 0.1);
		let index = hand.fingers.iter().find(|pose| pose.finger == Finger::Index).expect("index finger");
		let rotation = index.joints[1].rotation.expect("index intermediate rotation");
		let expected_z = (-0.84_f32 * 0.5).sin();
		assert!((rotation.z - expected_z).abs() < 1e-5);
		assert_eq!(frame.header.coordinate_space, CoordinateSpace::Unknown);
	}

	#[test]
	fn enrichment_preserves_existing_typed_hand_motion() {
		let mut frame = UNMotionFrame::new(11);
		frame.right_hand = Some(HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 0.5,
			wrist: None,
			fingers: Vec::new(),
		});
		frame.signals = vec![scalar("hand.right.present", 1.0), scalar("hand.right.index.curl", 0.8)];

		enrich_frame_with_signal_derived_motion(&mut frame);

		let hand = frame.right_hand.expect("existing right hand should remain");
		assert!(hand.fingers.is_empty());
		assert_eq!(hand.confidence, 0.5);
	}

	#[test]
	fn enrichment_preserves_existing_coordinate_space() {
		let mut frame = UNMotionFrame::new(12);
		frame.header.coordinate_space = CoordinateSpace::UNMotion;
		frame.signals = vec![
			scalar("hand.left.present", 1.0),
			scalar("hand.left.index.curl", 0.1),
			scalar("hand.left.index.spread", -0.3),
		];

		assert!(enrich_frame_with_signal_derived_motion(&mut frame));

		assert_eq!(frame.header.coordinate_space, CoordinateSpace::UNMotion);
		assert!(frame.left_hand.is_some());
	}
}
