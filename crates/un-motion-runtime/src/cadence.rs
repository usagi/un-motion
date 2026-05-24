use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct OutputCadence {
	fps: u32,
	interval: Duration,
	next_tick: Instant,
}

impl OutputCadence {
	pub fn for_fps(fps: u32, now: Instant) -> Self {
		let fps = fps.max(1);
		Self {
			fps,
			interval: Duration::from_secs_f64(1.0 / f64::from(fps)),
			next_tick: now,
		}
	}

	pub fn fps(&self) -> u32 {
		self.fps
	}

	pub fn interval(&self) -> Duration {
		self.interval
	}

	pub fn reset(&mut self, now: Instant) {
		self.next_tick = now;
	}

	pub fn mark_due(&mut self, now: Instant) -> bool {
		if now < self.next_tick {
			return false;
		}
		if now.duration_since(self.next_tick) > self.interval {
			self.next_tick = now;
		}
		self.next_tick += self.interval;
		true
	}

	pub fn sleep_duration(&self, now: Instant, max_sleep: Duration) -> Duration {
		self.next_tick
			.checked_duration_since(now)
			.unwrap_or_else(|| Duration::from_millis(1))
			.min(max_sleep)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn assert_duration_near(actual: Duration, expected_ns: u128) {
		let actual_ns = actual.as_nanos();
		let delta = actual_ns.abs_diff(expected_ns);
		assert!(delta <= 1, "actual={actual_ns}ns expected={expected_ns}ns");
	}

	#[test]
	fn computes_60_and_90_fps_intervals() {
		let now = Instant::now();

		assert_duration_near(OutputCadence::for_fps(60, now).interval(), 16_666_667);
		assert_duration_near(OutputCadence::for_fps(90, now).interval(), 11_111_111);
	}

	#[test]
	fn marks_first_tick_due_and_advances_by_interval() {
		let now = Instant::now();
		let mut cadence = OutputCadence::for_fps(60, now);

		assert!(cadence.mark_due(now));
		assert!(!cadence.mark_due(now + cadence.interval() - Duration::from_nanos(1)));
		assert!(cadence.mark_due(now + cadence.interval()));
	}

	#[test]
	fn skips_backlog_when_late() {
		let now = Instant::now();
		let mut cadence = OutputCadence::for_fps(90, now);
		let late = now + cadence.interval() * 10;

		assert!(cadence.mark_due(late));
		assert_eq!(cadence.sleep_duration(late, Duration::from_millis(100)), cadence.interval());
	}
}
