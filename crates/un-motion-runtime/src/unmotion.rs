use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use un_motion_frame::{BoneSample, ExpressionSample, HandMotion, HumanoidBone, SampleState, TrackingState, UNMotionFrame};
use un_motion_output_vmc::vmc_frame_pose_for_unmotion_frame;

use crate::signal_enrich::enrich_frame_with_signal_derived_motion;
use crate::{MotionPart, PartTrackingStatus, StreamId, StreamSnapshot, StreamState, TransformSample, upsert_part_diagnostic};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MotionFrameStreamConfig {
	pub stream_id: StreamId,
	pub stale_after_ns: u64,
}

impl MotionFrameStreamConfig {
	pub fn new(stream_id: StreamId) -> Self {
		Self {
			stream_id,
			stale_after_ns: 500_000_000,
		}
	}

	pub fn with_stale_after_ns(mut self, stale_after_ns: u64) -> Self {
		self.stale_after_ns = stale_after_ns;
		self
	}
}

pub trait MotionFrameSource: Send + 'static {
	fn next_frame(&mut self) -> anyhow::Result<Option<UNMotionFrame>>;
	/// 任意: ロックフリーなテレメトリハンドル。`Some(...)` を返すと、Capturer の
	/// runtime loop が `runtime_snapshot` の前に `load(Relaxed)` でカウンタを読み、
	/// `OutputTelemetry.sources` に積む。`None` のままなら何も計上されない。
	fn telemetry_handle(&self) -> Option<crate::SourceTelemetryHandle> {
		None
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct MotionFrameStreamPoll {
	pub stream_id: StreamId,
	pub frames: Vec<UNMotionFrame>,
	pub state: StreamState,
	pub snapshot: StreamSnapshot,
}

impl MotionFrameStreamPoll {
	pub fn is_idle(&self) -> bool {
		self.frames.is_empty()
	}
}

pub struct MotionFrameStreamWorker<S> {
	config: MotionFrameStreamConfig,
	source: S,
	state: StreamState,
}

impl<S> MotionFrameStreamWorker<S>
where
	S: MotionFrameSource,
{
	pub fn new(config: MotionFrameStreamConfig, source: S) -> Self {
		let state = StreamState::new(config.stream_id.clone());
		Self { config, source, state }
	}

	pub fn stream_id(&self) -> &StreamId {
		&self.config.stream_id
	}

	pub fn state(&self) -> &StreamState {
		&self.state
	}

	pub fn poll_once(&mut self) -> anyhow::Result<MotionFrameStreamPoll> {
		let observed_at_ns = now_unix_ns();
		let mut frames = Vec::new();
		if let Some(frame) = self.source.next_frame()? {
			apply_unmotion_frame_to_stream_state(&mut self.state, &frame, observed_at_ns);
			frames.push(frame);
		}

		Ok(MotionFrameStreamPoll {
			stream_id: self.config.stream_id.clone(),
			frames,
			state: self.state.clone(),
			snapshot: self.state.snapshot_at(now_unix_ns(), self.config.stale_after_ns),
		})
	}
}

#[derive(Debug)]
enum MotionFrameStreamWorkerControl {
	Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MotionFrameStreamWorkerMessage {
	Poll(MotionFrameStreamPoll),
	Error { stream_id: StreamId, message: String },
	Stopped { stream_id: StreamId },
}

pub struct MotionFrameStreamWorkerHandle {
	pub stream_id: StreamId,
	/// `MotionFrameSource::telemetry_handle()` で取得した
	/// ロックフリーカウンタへの参照。Capturer の runtime loop が
	/// `runtime_snapshot` 直前に `Arc<SourceStageAtomics>::load(Relaxed)` で読む。
	pub telemetry: Option<crate::SourceTelemetryHandle>,
	control_tx: Sender<MotionFrameStreamWorkerControl>,
	join: Option<JoinHandle<()>>,
}

impl MotionFrameStreamWorkerHandle {
	pub fn stop(&self) {
		let _ = self.control_tx.send(MotionFrameStreamWorkerControl::Stop);
	}

	pub fn join(mut self) -> thread::Result<()> {
		self.stop();
		if let Some(join) = self.join.take() { join.join() } else { Ok(()) }
	}
}

impl Drop for MotionFrameStreamWorkerHandle {
	fn drop(&mut self) {
		let _ = self.control_tx.send(MotionFrameStreamWorkerControl::Stop);
	}
}

pub fn spawn_motion_frame_stream_worker<S>(
	config: MotionFrameStreamConfig,
	source: S,
	output_tx: Sender<MotionFrameStreamWorkerMessage>,
	idle_sleep: Duration,
) -> anyhow::Result<MotionFrameStreamWorkerHandle>
where
	S: MotionFrameSource,
{
	let telemetry = source.telemetry_handle();
	let mut worker = MotionFrameStreamWorker::new(config, source);
	let stream_id = worker.stream_id().clone();
	let (control_tx, control_rx) = mpsc::channel();
	let thread_stream_id = stream_id.clone();
	let join = thread::spawn(move || run_frame_stream_worker(&mut worker, control_rx, output_tx, idle_sleep));
	Ok(MotionFrameStreamWorkerHandle {
		stream_id: thread_stream_id,
		telemetry,
		control_tx,
		join: Some(join),
	})
}

fn run_frame_stream_worker<S>(
	worker: &mut MotionFrameStreamWorker<S>,
	control_rx: Receiver<MotionFrameStreamWorkerControl>,
	output_tx: Sender<MotionFrameStreamWorkerMessage>,
	idle_sleep: Duration,
) where
	S: MotionFrameSource,
{
	loop {
		if matches!(control_rx.try_recv(), Ok(MotionFrameStreamWorkerControl::Stop)) {
			break;
		}

		match worker.poll_once() {
			Ok(poll) => {
				let idle = poll.is_idle();
				if !idle && output_tx.send(MotionFrameStreamWorkerMessage::Poll(poll)).is_err() {
					break;
				}
				if idle {
					thread::sleep(idle_sleep);
				}
			}
			Err(error) => {
				if output_tx
					.send(MotionFrameStreamWorkerMessage::Error {
						stream_id: worker.stream_id().clone(),
						message: error.to_string(),
					})
					.is_err()
				{
					break;
				}
				thread::sleep(idle_sleep);
			}
		}
	}

	let _ = output_tx.send(MotionFrameStreamWorkerMessage::Stopped {
		stream_id: worker.stream_id().clone(),
	});
}

pub fn stream_state_from_unmotion_frame(stream_id: StreamId, frame: &UNMotionFrame, observed_at_ns: u64) -> StreamState {
	let mut state = StreamState::new(stream_id);
	apply_unmotion_frame_to_stream_state(&mut state, frame, observed_at_ns);
	state
}

pub fn apply_unmotion_frame_to_stream_state(state: &mut StreamState, frame: &UNMotionFrame, observed_at_ns: u64) {
	state.packet_count = state.packet_count.saturating_add(1);
	state.first_packet_at_ns.get_or_insert(observed_at_ns);
	state.last_packet_at_ns = Some(observed_at_ns);

	let mut canonical_frame = frame.clone();
	enrich_frame_with_signal_derived_motion(&mut canonical_frame);
	let pose = vmc_frame_pose_for_unmotion_frame(&canonical_frame);
	let mut event_count = 0_u64;
	state.root = None;
	state.bones.clear();
	state.blendshapes.clear();
	state.part_diagnostics.clear();
	observe_unmotion_part_diagnostics(&mut state.part_diagnostics, &canonical_frame);
	if let Some(root) = pose.root {
		state.root = Some(TransformSample::new(root.name, root.position, root.rotation));
		event_count += 1;
	}
	for bone in pose.bones {
		let sample = TransformSample::new(bone.name, bone.position, bone.rotation);
		state.bones.insert(sample.name.clone(), sample);
		event_count += 1;
	}
	for (name, value) in pose.blendshapes {
		state.blendshapes.insert(name, value);
		event_count += 1;
	}
	if event_count > 0 {
		state.last_event_at_ns = Some(observed_at_ns);
		state.event_count = state.event_count.saturating_add(event_count);
	}
}

fn observe_unmotion_part_diagnostics(
	diagnostics: &mut std::collections::BTreeMap<MotionPart, crate::PartDiagnostic>,
	frame: &UNMotionFrame,
) {
	if let Some(body) = frame.body.as_ref() {
		if let Some(humanoid) = body.humanoid.as_ref() {
			for bone in &humanoid.bones {
				upsert_part_diagnostic(
					diagnostics,
					part_for_humanoid_bone(bone.bone),
					status_for_bone(bone),
					bone.confidence,
				);
			}
			if let Some(root) = humanoid.root.as_ref() {
				upsert_part_diagnostic(
					diagnostics,
					MotionPart::Torso,
					status_for_tracking_state(body.tracking_state),
					body.confidence.max(transform_confidence_hint(root)),
				);
			}
		}
	}
	if let Some(face) = frame.face.as_ref() {
		if face.head.is_some() {
			upsert_part_diagnostic(
				diagnostics,
				MotionPart::Head,
				status_for_tracking_state(face.tracking_state),
				face.confidence,
			);
		}
		if !face.expressions.is_empty() {
			for expression in &face.expressions {
				observe_expression_diagnostic(diagnostics, expression, face.tracking_state);
			}
		}
	}
	observe_hand_diagnostic(diagnostics, MotionPart::LeftHand, frame.left_hand.as_ref());
	observe_hand_diagnostic(diagnostics, MotionPart::RightHand, frame.right_hand.as_ref());
}

fn observe_expression_diagnostic(
	diagnostics: &mut std::collections::BTreeMap<MotionPart, crate::PartDiagnostic>,
	expression: &ExpressionSample,
	fallback_state: TrackingState,
) {
	let status = status_for_sample_state(expression.state).unwrap_or_else(|| status_for_tracking_state(fallback_state));
	let part = if expression.name.to_ascii_lowercase().contains("eye") {
		MotionPart::Eyes
	} else {
		MotionPart::Face
	};
	upsert_part_diagnostic(diagnostics, part, status, expression.confidence);
}

fn observe_hand_diagnostic(
	diagnostics: &mut std::collections::BTreeMap<MotionPart, crate::PartDiagnostic>,
	part: MotionPart,
	hand: Option<&HandMotion>,
) {
	let Some(hand) = hand else {
		return;
	};
	upsert_part_diagnostic(diagnostics, part, status_for_tracking_state(hand.tracking_state), hand.confidence);
}

fn part_for_humanoid_bone(bone: HumanoidBone) -> MotionPart {
	match bone {
		HumanoidBone::Head | HumanoidBone::Neck | HumanoidBone::Jaw | HumanoidBone::LeftEye | HumanoidBone::RightEye => MotionPart::Head,
		HumanoidBone::LeftShoulder | HumanoidBone::LeftUpperArm | HumanoidBone::LeftLowerArm => MotionPart::LeftArm,
		HumanoidBone::RightShoulder | HumanoidBone::RightUpperArm | HumanoidBone::RightLowerArm => MotionPart::RightArm,
		HumanoidBone::LeftHand => MotionPart::LeftHand,
		HumanoidBone::RightHand => MotionPart::RightHand,
		HumanoidBone::Hips | HumanoidBone::Spine | HumanoidBone::Chest | HumanoidBone::UpperChest => MotionPart::Torso,
		HumanoidBone::LeftUpperLeg | HumanoidBone::LeftLowerLeg => MotionPart::LeftLeg,
		HumanoidBone::RightUpperLeg | HumanoidBone::RightLowerLeg => MotionPart::RightLeg,
		HumanoidBone::LeftFoot | HumanoidBone::LeftToes => MotionPart::LeftFoot,
		HumanoidBone::RightFoot | HumanoidBone::RightToes => MotionPart::RightFoot,
	}
}

fn status_for_bone(bone: &BoneSample) -> PartTrackingStatus {
	status_for_sample_state(bone.state).unwrap_or(PartTrackingStatus::Estimated)
}

fn status_for_sample_state(state: SampleState) -> Option<PartTrackingStatus> {
	match state {
		SampleState::Valid => Some(PartTrackingStatus::Estimated),
		SampleState::Held => Some(PartTrackingStatus::Held),
		_ => None,
	}
}

fn status_for_tracking_state(state: TrackingState) -> PartTrackingStatus {
	match state {
		TrackingState::Valid => PartTrackingStatus::Estimated,
		TrackingState::Recovering => PartTrackingStatus::Recovering,
		TrackingState::Lost => PartTrackingStatus::Lost,
		_ => PartTrackingStatus::Estimated,
	}
}

fn transform_confidence_hint(_transform: &un_motion_frame::TransformSample) -> f32 {
	0.0
}

fn now_unix_ns() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_nanos() as u64)
		.unwrap_or(0)
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_frame::{
		BodyMotion, BoneSample, ExpressionSample, FaceMotion, HumanoidBone, HumanoidPose, Quatf, SampleState, TrackingState,
		TransformSample as FrameTransformSample, Vec3f,
	};

	struct VecFrameSource {
		frames: std::collections::VecDeque<UNMotionFrame>,
	}

	impl VecFrameSource {
		fn new(frames: Vec<UNMotionFrame>) -> Self {
			Self { frames: frames.into() }
		}
	}

	impl MotionFrameSource for VecFrameSource {
		fn next_frame(&mut self) -> anyhow::Result<Option<UNMotionFrame>> {
			Ok(self.frames.pop_front())
		}
	}

	fn frame_transform(position: [f32; 3], rotation: [f32; 4]) -> FrameTransformSample {
		FrameTransformSample {
			translation: Some(Vec3f {
				x: position[0],
				y: position[1],
				z: position[2],
			}),
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

	fn face_with_expression(name: &str, value: f32) -> FaceMotion {
		FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: name.to_string(),
				value,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		}
	}

	#[test]
	fn converts_direct_unmotion_frame_to_stream_state() {
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: Some(frame_transform([0.25, 1.0, -0.5], [0.1, 0.2, 0.3, 0.9])),
				bones: vec![
					BoneSample {
						bone: HumanoidBone::Head,
						transform: frame_transform([0.0, 0.1, 0.0], [0.0, 0.25, 0.0, 0.9682458]),
						confidence: 1.0,
						source_index: Some(0),
						state: SampleState::Valid,
					},
					BoneSample {
						bone: HumanoidBone::LeftHand,
						transform: frame_transform([-0.4, 0.7, 0.0], [0.0, 0.0, 0.0, 1.0]),
						confidence: 0.8,
						source_index: Some(0),
						state: SampleState::Valid,
					},
					BoneSample {
						bone: HumanoidBone::RightHand,
						transform: frame_transform([0.4, 0.7, 0.0], [0.0, 0.0, 0.0, 1.0]),
						confidence: 0.3,
						source_index: Some(0),
						state: SampleState::Held,
					},
				],
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

		let state = stream_state_from_unmotion_frame(StreamId::new("unmotion:stream-1"), &frame, 100);

		assert_eq!(state.packet_count, 1);
		assert_eq!(state.last_event_at_ns, Some(100));
		assert_eq!(state.root.as_ref().expect("root").position, [0.25, 1.0, -0.5]);
		assert_eq!(state.bones["Head"].rotation, [0.0, 0.25, 0.0, 0.9682458]);
		assert_eq!(state.blendshapes["Joy"], 0.75);
		assert_eq!(state.part_diagnostics[&MotionPart::Head].status, PartTrackingStatus::Estimated);
		assert_eq!(state.part_diagnostics[&MotionPart::Face].status, PartTrackingStatus::Estimated);
		assert_eq!(state.part_diagnostics[&MotionPart::LeftHand].status, PartTrackingStatus::Estimated);
		assert_eq!(state.part_diagnostics[&MotionPart::LeftHand].confidence, 0.8);
		assert_eq!(state.part_diagnostics[&MotionPart::RightHand].status, PartTrackingStatus::Held);
		assert_eq!(state.part_diagnostics[&MotionPart::RightHand].confidence, 0.3);
	}

	#[test]
	fn converts_face_motion_unmotion_frame_to_stream_state() {
		let mut frame = UNMotionFrame::new(2);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "eyeBlinkLeft".to_string(),
				value: 0.75,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});

		let state = stream_state_from_unmotion_frame(StreamId::new("unmotion:stream-2"), &frame, 200);

		assert_eq!(state.blendshapes["eyeBlinkLeft"], 0.75);
		assert_eq!(state.last_event_at_ns, Some(200));
	}

	#[test]
	fn applying_unmotion_frames_preserves_counters_and_replaces_latest_values() {
		let mut state = StreamState::new(StreamId::new("unmotion:stream-3"));
		let mut first = UNMotionFrame::new(3);
		first.face = Some(face_with_expression("jawOpen", 0.2));
		let mut second = UNMotionFrame::new(4);
		second.face = Some(face_with_expression("jawOpen", 0.8));

		apply_unmotion_frame_to_stream_state(&mut state, &first, 300);
		apply_unmotion_frame_to_stream_state(&mut state, &second, 400);

		assert_eq!(state.packet_count, 2);
		assert_eq!(state.first_packet_at_ns, Some(300));
		assert_eq!(state.last_packet_at_ns, Some(400));
		assert_eq!(state.blendshapes["jawOpen"], 0.8);
	}

	#[test]
	fn applying_unmotion_frame_clears_bones_that_disappear_from_snapshot() {
		let mut state = StreamState::new(StreamId::new("unmotion:stream-5"));
		let mut first = UNMotionFrame::new(5);
		first.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![BoneSample {
					bone: HumanoidBone::LeftUpperArm,
					transform: frame_transform([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				}],
			}),
		});
		let second = UNMotionFrame::new(6);

		apply_unmotion_frame_to_stream_state(&mut state, &first, 500);
		assert!(state.bones.contains_key("LeftUpperArm"));

		apply_unmotion_frame_to_stream_state(&mut state, &second, 600);

		assert!(!state.bones.contains_key("LeftUpperArm"));
		assert_eq!(state.packet_count, 2);
		assert_eq!(state.last_packet_at_ns, Some(600));
	}

	#[test]
	fn worker_polls_frame_source_into_stream_state() {
		let mut frame = UNMotionFrame::new(5);
		frame.face = Some(face_with_expression("jawOpen", 0.5));
		let mut worker = MotionFrameStreamWorker::new(
			MotionFrameStreamConfig::new(StreamId::new("unmotion:stream-4")).with_stale_after_ns(1_000_000),
			VecFrameSource::new(vec![frame]),
		);

		let poll = worker.poll_once().expect("poll");

		assert_eq!(poll.frames.len(), 1);
		assert_eq!(poll.stream_id, StreamId::new("unmotion:stream-4"));
		assert_eq!(poll.state.packet_count, 1);
		assert_eq!(poll.state.blendshapes["jawOpen"], 0.5);
		assert_eq!(poll.snapshot.stream_id, StreamId::new("unmotion:stream-4"));
	}

	#[test]
	fn spawned_worker_emits_poll_messages_until_stopped() {
		let mut frame = UNMotionFrame::new(6);
		frame.face = Some(face_with_expression("eyeBlinkLeft", 0.6));
		let (tx, rx) = mpsc::channel();
		let handle = spawn_motion_frame_stream_worker(
			MotionFrameStreamConfig::new(StreamId::new("unmotion:stream-5")),
			VecFrameSource::new(vec![frame]),
			tx,
			Duration::from_millis(1),
		)
		.expect("spawn worker");

		let poll = rx.recv_timeout(Duration::from_secs(1)).expect("poll");
		handle.join().expect("join");
		let stopped = rx.recv_timeout(Duration::from_secs(1)).expect("stopped");

		assert!(matches!(
			poll,
			MotionFrameStreamWorkerMessage::Poll(MotionFrameStreamPoll { stream_id, state, .. })
				if stream_id == StreamId::new("unmotion:stream-5") && state.blendshapes["eyeBlinkLeft"] == 0.6
		));
		assert!(matches!(
			stopped,
			MotionFrameStreamWorkerMessage::Stopped { stream_id } if stream_id == StreamId::new("unmotion:stream-5")
		));
	}
}
