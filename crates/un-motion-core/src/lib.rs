use std::collections::HashMap;

use un_motion_frame::{BoneSample, HumanoidPose, UNMotionFrame};

pub mod control;
pub mod runtime_host;
pub mod server;
pub mod tray;
mod unmotion_source;

pub use control::{
	ActiveProfileRequest, CoreApiConfig, CoreControlState, CoreEvent, CoreEventKind, CoreSnapshot, DEFAULT_API_WORKER_THREADS,
	RuntimeStatus, default_api_worker_threads, logical_core_count, normalize_api_worker_threads,
};
pub use server::{configure_api, run_api_server};
pub use tray::{TrayOptions, run_core_with_tray};

// Profile schema は別 crate に分離した。Core の HTTP ハンドラ等から参照しやすいよう
// ここで re-export する。Supervisor 側は直接 `un-motion-profile-schema` を依存して
// core を引かない構成にする。
pub use un_motion_profile_schema::{
	CoreProfile, CoreProfileDocument, CoreProfileDocumentProfile, CoreProfileDocumentSource, CoreProfileDocumentStore,
	ProfileMediaPipeAdvancedSettings, ProfileModifierSettings, ProfilePipelineComponents, ProfileRuntimeSettings, document_from_profiles,
	document_profiles, normalize_profile_document,
};

#[derive(Clone, Debug)]
pub struct BoneSelectorPolicy {
	pub source_priority: HashMap<String, i32>,
	pub min_bone_confidence: f32,
	pub priority_weight: f32,
	pub confidence_weight: f32,
}

impl Default for BoneSelectorPolicy {
	fn default() -> Self {
		Self {
			source_priority: HashMap::new(),
			min_bone_confidence: 0.0,
			priority_weight: 1000.0,
			confidence_weight: 100.0,
		}
	}
}

pub fn select_humanoid_pose_per_bone(frames: &[UNMotionFrame], policy: &BoneSelectorPolicy) -> Option<HumanoidPose> {
	let mut best_by_bone: HashMap<u16, (f32, u64, u64, BoneSample)> = HashMap::new();

	for frame in frames {
		let Some(body) = &frame.body else {
			continue;
		};
		let Some(humanoid) = &body.humanoid else {
			continue;
		};

		for bone in &humanoid.bones {
			let confidence = normalized_confidence(bone.confidence);
			if confidence < policy.min_bone_confidence {
				continue;
			}

			let source_id = resolve_source_id(frame, bone).unwrap_or("unknown");
			let priority = *policy.source_priority.get(source_id).unwrap_or(&0) as f32;
			let score = (policy.priority_weight * priority) + (policy.confidence_weight * confidence);
			let key = bone.bone as u16;

			let replace = match best_by_bone.get(&key) {
				None => true,
				Some((best_score, best_ts, best_seq, _)) => {
					score > *best_score
						|| (score == *best_score
							&& (frame.header.capture_timestamp_ns > *best_ts
								|| (frame.header.capture_timestamp_ns == *best_ts && frame.header.sequence > *best_seq)))
				}
			};

			if replace {
				best_by_bone.insert(key, (score, frame.header.capture_timestamp_ns, frame.header.sequence, bone.clone()));
			}
		}
	}

	if best_by_bone.is_empty() {
		return None;
	}

	let mut bones: Vec<BoneSample> = best_by_bone.into_values().map(|(_, _, _, bone)| bone).collect();
	bones.sort_by_key(|bone| bone.bone as u16);

	Some(HumanoidPose { root: None, bones })
}

fn resolve_source_id<'a>(frame: &'a UNMotionFrame, bone: &BoneSample) -> Option<&'a str> {
	let idx = bone.source_index? as usize;
	frame.sources.get(idx).map(|s| s.source_id.as_str())
}

fn normalized_confidence(confidence: f32) -> f32 {
	if confidence.is_finite() { confidence.clamp(0.0, 1.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_frame::{
		BodyMotion, CoordinateSpace, Handedness, HumanoidBone, LengthUnit, MotionHeader, MotionSourceInfo, MotionSourceKind, SampleState,
		TimestampBasis, TrackingState, TransformSample,
	};

	fn build_frame(sequence: u64, capture_timestamp_ns: u64, source_id: &str, bones: Vec<(HumanoidBone, f32)>) -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(sequence);
		frame.header = MotionHeader {
			magic: MotionHeader::MAGIC,
			version_major: 0,
			version_minor: 1,
			sequence,
			timestamp_basis: TimestampBasis::Monotonic,
			capture_timestamp_ns,
			frame_timestamp_ns: capture_timestamp_ns,
			processed_timestamp_ns: capture_timestamp_ns,
			coordinate_space: CoordinateSpace::UNMotion,
			handedness: Handedness::Unknown,
			length_unit: LengthUnit::Normalized,
			stream_id: None,
			expected_dt_ns: None,
		};
		frame.sources.push(MotionSourceInfo {
			source_id: source_id.to_string(),
			source_kind: MotionSourceKind::VmcInput,
			display_name: None,
			confidence: 1.0,
			latency_ns: Some(0),
			state: TrackingState::Valid,
		});
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: bones
					.into_iter()
					.map(|(bone, confidence)| BoneSample {
						bone,
						transform: TransformSample {
							translation: None,
							rotation: None,
							scale: None,
							linear_velocity: None,
							angular_velocity: None,
						},
						confidence,
						source_index: Some(0),
						state: SampleState::Valid,
					})
					.collect(),
			}),
		});
		frame
	}

	#[test]
	fn selector_prefers_higher_priority_source_for_same_bone() {
		let low = build_frame(1, 100, "low", vec![(HumanoidBone::Head, 1.0)]);
		let high = build_frame(2, 90, "high", vec![(HumanoidBone::Head, 0.2)]);

		let mut policy = BoneSelectorPolicy::default();
		policy.source_priority.insert("low".to_string(), 1);
		policy.source_priority.insert("high".to_string(), 5);

		let pose = select_humanoid_pose_per_bone(&[low, high], &policy).expect("pose should exist");
		assert_eq!(pose.bones.len(), 1);
		assert!((pose.bones[0].confidence - 0.2).abs() < 1e-6);
	}

	#[test]
	fn selector_can_pick_bones_from_different_sources() {
		let source_a = build_frame(1, 100, "source-a", vec![(HumanoidBone::Head, 0.9)]);
		let source_b = build_frame(2, 100, "source-b", vec![(HumanoidBone::LeftHand, 0.9)]);

		let pose = select_humanoid_pose_per_bone(&[source_a, source_b], &BoneSelectorPolicy::default()).expect("pose should exist");

		assert_eq!(pose.bones.len(), 2);
		assert!(pose.bones.iter().any(|b| b.bone == HumanoidBone::Head));
		assert!(pose.bones.iter().any(|b| b.bone == HumanoidBone::LeftHand));
	}

	#[test]
	fn selector_respects_min_bone_confidence() {
		let frame = build_frame(1, 100, "source-a", vec![(HumanoidBone::Head, 0.3), (HumanoidBone::LeftHand, 0.7)]);
		let policy = BoneSelectorPolicy {
			min_bone_confidence: 0.5,
			..BoneSelectorPolicy::default()
		};

		let pose = select_humanoid_pose_per_bone(&[frame], &policy).expect("pose should exist");
		assert_eq!(pose.bones.len(), 1);
		assert_eq!(pose.bones[0].bone, HumanoidBone::LeftHand);
	}
}
