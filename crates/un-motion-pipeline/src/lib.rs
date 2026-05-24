use std::collections::{HashMap, VecDeque};

use un_motion_frame::UNMotionFrame;
use un_motion_interfaces::{
	FrameProcessor, FrameQueue, ImageFrame, ImageFrameBuffer, ImageInferenceEngine, QueueOverflowPolicy, QueuePushResult, QueueStats,
};

pub type RingImageFrameBuffer = BoundedRingBuffer<ImageFrame>;

#[derive(Clone, Debug)]
pub struct BoundedRingBuffer<T> {
	items: VecDeque<T>,
	capacity: usize,
	overflow_policy: QueueOverflowPolicy,
	pushed_count: u64,
	popped_count: u64,
	replaced_old_count: u64,
	dropped_new_count: u64,
	blocked_count: u64,
	event_version: u64,
}

impl<T> BoundedRingBuffer<T> {
	pub fn new(capacity: usize) -> Self {
		Self::with_overflow_policy(capacity, QueueOverflowPolicy::DropOldest)
	}

	pub fn with_overflow_policy(capacity: usize, overflow_policy: QueueOverflowPolicy) -> Self {
		Self {
			items: VecDeque::with_capacity(capacity.max(1)),
			capacity: capacity.max(1),
			overflow_policy,
			pushed_count: 0,
			popped_count: 0,
			replaced_old_count: 0,
			dropped_new_count: 0,
			blocked_count: 0,
			event_version: 0,
		}
	}

	pub fn is_empty(&self) -> bool {
		self.items.is_empty()
	}

	pub fn overflow_policy(&self) -> QueueOverflowPolicy {
		self.overflow_policy
	}

	pub fn latest_ref(&self) -> Option<&T> {
		self.items.back()
	}
}

impl<T> Default for BoundedRingBuffer<T> {
	fn default() -> Self {
		Self::new(3)
	}
}

impl<T> FrameQueue<T> for BoundedRingBuffer<T> {
	fn push(&mut self, item: T) -> QueuePushResult {
		if self.items.len() >= self.capacity {
			match self.overflow_policy {
				QueueOverflowPolicy::DropOldest | QueueOverflowPolicy::ReplaceOld => {
					self.items.pop_front();
					self.replaced_old_count = self.replaced_old_count.saturating_add(1);
					self.items.push_back(item);
					self.pushed_count = self.pushed_count.saturating_add(1);
					self.event_version = self.event_version.saturating_add(1);
					return QueuePushResult::ReplacedOld;
				}
				QueueOverflowPolicy::DropNewest => {
					self.dropped_new_count = self.dropped_new_count.saturating_add(1);
					self.event_version = self.event_version.saturating_add(1);
					return QueuePushResult::DroppedNew;
				}
				QueueOverflowPolicy::BlockProducer => {
					self.blocked_count = self.blocked_count.saturating_add(1);
					return QueuePushResult::Blocked;
				}
			}
		}
		self.items.push_back(item);
		self.pushed_count = self.pushed_count.saturating_add(1);
		self.event_version = self.event_version.saturating_add(1);
		QueuePushResult::Accepted
	}

	fn pop_latest(&mut self) -> Option<T> {
		let latest = self.items.pop_back();
		if latest.is_some() {
			self.items.clear();
			self.popped_count = self.popped_count.saturating_add(1);
		}
		latest
	}

	fn pop_oldest(&mut self) -> Option<T> {
		let item = self.items.pop_front();
		if item.is_some() {
			self.popped_count = self.popped_count.saturating_add(1);
		}
		item
	}

	fn drain(&mut self, max: usize) -> Vec<T> {
		let count = max.min(self.items.len());
		let mut out = Vec::with_capacity(count);
		for _ in 0..count {
			if let Some(item) = self.items.pop_front() {
				out.push(item);
				self.popped_count = self.popped_count.saturating_add(1);
			}
		}
		out
	}

	fn stats(&self) -> QueueStats {
		QueueStats {
			len: self.items.len(),
			capacity: self.capacity,
			pushed: self.pushed_count,
			popped: self.popped_count,
			replaced_old: self.replaced_old_count,
			dropped_new: self.dropped_new_count,
			blocked: self.blocked_count,
			event_version: self.event_version,
		}
	}
}

impl ImageFrameBuffer for BoundedRingBuffer<ImageFrame> {
	fn push(&mut self, frame: ImageFrame) {
		let _ = <Self as FrameQueue<ImageFrame>>::push(self, frame);
	}

	fn latest(&self) -> Option<ImageFrame> {
		self.items.back().cloned()
	}

	fn latest_by_source(&self, source_id: &str) -> Option<ImageFrame> {
		self.items.iter().rev().find(|frame| frame.metadata.source_id == source_id).cloned()
	}

	fn read_batch(&mut self, max_frames: usize) -> Vec<ImageFrame> {
		FrameQueue::drain(self, max_frames)
	}

	fn len(&self) -> usize {
		self.items.len()
	}

	fn capacity(&self) -> usize {
		self.capacity
	}

	fn dropped_count(&self) -> u64 {
		self.replaced_old_count.saturating_add(self.dropped_new_count)
	}

	fn event_version(&self) -> u64 {
		self.event_version
	}
}

#[derive(Clone, Debug, Default)]
pub struct RealtimeLatestBuffer<T> {
	latest: Option<T>,
	pushed_count: u64,
	popped_count: u64,
	replaced_old_count: u64,
	event_version: u64,
}

impl<T> RealtimeLatestBuffer<T> {
	pub fn new() -> Self {
		Self {
			latest: None,
			pushed_count: 0,
			popped_count: 0,
			replaced_old_count: 0,
			event_version: 0,
		}
	}

	pub fn peek_latest(&self) -> Option<&T> {
		self.latest.as_ref()
	}
}

impl<T> FrameQueue<T> for RealtimeLatestBuffer<T> {
	fn push(&mut self, item: T) -> QueuePushResult {
		let result = if self.latest.replace(item).is_some() {
			self.replaced_old_count = self.replaced_old_count.saturating_add(1);
			QueuePushResult::ReplacedOld
		} else {
			QueuePushResult::Accepted
		};
		self.pushed_count = self.pushed_count.saturating_add(1);
		self.event_version = self.event_version.saturating_add(1);
		result
	}

	fn pop_latest(&mut self) -> Option<T> {
		let latest = self.latest.take();
		if latest.is_some() {
			self.popped_count = self.popped_count.saturating_add(1);
		}
		latest
	}

	fn pop_oldest(&mut self) -> Option<T> {
		self.pop_latest()
	}

	fn drain(&mut self, max: usize) -> Vec<T> {
		if max == 0 {
			Vec::new()
		} else {
			self.pop_latest().into_iter().collect()
		}
	}

	fn stats(&self) -> QueueStats {
		QueueStats {
			len: usize::from(self.latest.is_some()),
			capacity: 1,
			pushed: self.pushed_count,
			popped: self.popped_count,
			replaced_old: self.replaced_old_count,
			dropped_new: 0,
			blocked: 0,
			event_version: self.event_version,
		}
	}
}

#[derive(Clone, Debug)]
pub struct PipelinePolicy {
	pub stale_timeout_ns: u64,
	pub hold_last_ticks: u32,
	pub hold_decay_per_tick: f32,
	pub source_priority: HashMap<String, i32>,
	pub source_min_confidence: HashMap<String, f32>,
	pub source_stale_timeout_ns: HashMap<String, u64>,
	pub priority_weight: f32,
	pub confidence_weight: f32,
	pub freshness_weight: f32,
}

impl Default for PipelinePolicy {
	fn default() -> Self {
		Self {
			stale_timeout_ns: u64::MAX,
			hold_last_ticks: 0,
			hold_decay_per_tick: 1.0,
			source_priority: HashMap::new(),
			source_min_confidence: HashMap::new(),
			source_stale_timeout_ns: HashMap::new(),
			priority_weight: 1000.0,
			confidence_weight: 100.0,
			freshness_weight: 1.0,
		}
	}
}

pub struct ImageInferencePipeline<E, P> {
	pub engine: E,
	pub post_processor: P,
}

impl<E, P> ImageInferencePipeline<E, P> {
	pub fn new(engine: E, post_processor: P) -> Self {
		Self { engine, post_processor }
	}
}

impl<E, P, R> ImageInferencePipeline<E, P>
where
	E: ImageInferenceEngine<Output = R>,
	for<'a> P: FrameProcessor<(&'a ImageFrame, &'a R), UNMotionFrame>,
{
	pub fn process_frame(&mut self, frame: &ImageFrame) -> anyhow::Result<UNMotionFrame> {
		let raw = self.engine.process_image(frame)?;
		self.post_processor.process((frame, &raw))
	}

	pub fn process_latest<B>(&mut self, buffer: &mut B) -> anyhow::Result<Option<UNMotionFrame>>
	where
		B: ImageFrameBuffer,
	{
		let Some(frame) = buffer.latest() else {
			return Ok(None);
		};
		self.process_frame(&frame).map(Some)
	}

	pub fn drain_buffer<B>(&mut self, buffer: &mut B, max_frames: usize) -> anyhow::Result<Vec<UNMotionFrame>>
	where
		B: ImageFrameBuffer,
	{
		let frames = buffer.read_batch(max_frames);
		let mut out = Vec::with_capacity(frames.len());
		for frame in frames {
			out.push(self.process_frame(&frame)?);
		}
		Ok(out)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ring_image_frame_buffer_drops_oldest_when_full() {
		let mut buffer = RingImageFrameBuffer::new(2);
		for sequence in 0..3 {
			ImageFrameBuffer::push(
				&mut buffer,
				ImageFrame::new_rgb8(sequence, sequence, "camera:a", 1, 1, vec![sequence as u8, 0, 0]).unwrap(),
			);
		}

		assert_eq!(buffer.len(), 2);
		assert_eq!(buffer.capacity(), 2);
		assert_eq!(buffer.dropped_count(), 1);
		assert_eq!(buffer.event_version(), 3);
		assert_eq!(buffer.latest().expect("latest").metadata.sequence, 2);
		let batch = buffer.read_batch(8);
		assert_eq!(batch.iter().map(|frame| frame.metadata.sequence).collect::<Vec<_>>(), vec![1, 2]);
		assert!(buffer.is_empty());
	}

	#[test]
	fn ring_image_frame_buffer_finds_latest_by_source() {
		let mut buffer = RingImageFrameBuffer::new(4);
		ImageFrameBuffer::push(&mut buffer, ImageFrame::new_rgb8(1, 1, "camera:a", 1, 1, vec![1, 0, 0]).unwrap());
		ImageFrameBuffer::push(&mut buffer, ImageFrame::new_rgb8(2, 2, "camera:b", 1, 1, vec![2, 0, 0]).unwrap());
		ImageFrameBuffer::push(&mut buffer, ImageFrame::new_rgb8(3, 3, "camera:a", 1, 1, vec![3, 0, 0]).unwrap());

		assert_eq!(buffer.latest_by_source("camera:a").expect("camera a latest").metadata.sequence, 3);
		assert_eq!(buffer.latest_by_source("camera:b").expect("camera b latest").metadata.sequence, 2);
	}

	#[test]
	fn ring_image_frame_buffer_drop_newest_policy_keeps_existing_frames() {
		let mut buffer = RingImageFrameBuffer::with_overflow_policy(2, QueueOverflowPolicy::DropNewest);
		assert_eq!(
			FrameQueue::push(&mut buffer, ImageFrame::new_rgb8(1, 1, "camera:a", 1, 1, vec![1, 0, 0]).unwrap()),
			QueuePushResult::Accepted
		);
		assert_eq!(
			FrameQueue::push(&mut buffer, ImageFrame::new_rgb8(2, 2, "camera:a", 1, 1, vec![2, 0, 0]).unwrap()),
			QueuePushResult::Accepted
		);
		assert_eq!(
			FrameQueue::push(&mut buffer, ImageFrame::new_rgb8(3, 3, "camera:a", 1, 1, vec![3, 0, 0]).unwrap()),
			QueuePushResult::DroppedNew
		);

		assert_eq!(buffer.stats().dropped_new, 1);
		assert_eq!(
			buffer.drain(8).iter().map(|frame| frame.metadata.sequence).collect::<Vec<_>>(),
			vec![1, 2]
		);
	}

	#[test]
	fn realtime_latest_buffer_replaces_old_value() {
		let mut buffer = RealtimeLatestBuffer::new();
		assert_eq!(buffer.push(10_u32), QueuePushResult::Accepted);
		assert_eq!(buffer.push(20_u32), QueuePushResult::ReplacedOld);
		assert_eq!(buffer.stats().replaced_old, 1);
		assert_eq!(buffer.peek_latest(), Some(&20));
		assert_eq!(buffer.pop_latest(), Some(20));
		assert_eq!(buffer.stats().len, 0);
	}

	#[test]
	fn image_inference_pipeline_drains_image_buffer() {
		struct TestEngine;

		impl ImageInferenceEngine for TestEngine {
			type Output = u64;

			fn process_image(&mut self, frame: &ImageFrame) -> anyhow::Result<Self::Output> {
				Ok(frame.metadata.sequence + 100)
			}
		}

		struct TestPostProcessor;

		impl<'a> FrameProcessor<(&'a ImageFrame, &'a u64), UNMotionFrame> for TestPostProcessor {
			fn process(&mut self, input: (&'a ImageFrame, &'a u64)) -> anyhow::Result<UNMotionFrame> {
				Ok(UNMotionFrame::new(input.1 + input.0.metadata.sequence))
			}
		}

		let mut buffer = RingImageFrameBuffer::new(4);
		ImageFrameBuffer::push(&mut buffer, ImageFrame::new_rgb8(1, 1, "camera:a", 1, 1, vec![1, 0, 0]).unwrap());
		ImageFrameBuffer::push(&mut buffer, ImageFrame::new_rgb8(2, 2, "camera:a", 1, 1, vec![2, 0, 0]).unwrap());

		let mut pipeline = ImageInferencePipeline::new(TestEngine, TestPostProcessor);
		let frames = pipeline.drain_buffer(&mut buffer, 8).expect("drain");
		assert_eq!(frames.iter().map(|frame| frame.header.sequence).collect::<Vec<_>>(), vec![102, 104]);
		assert!(buffer.is_empty());
	}
}
