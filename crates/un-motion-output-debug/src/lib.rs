use anyhow::Context;
use un_motion_frame::UNMotionFrame;
use un_motion_interfaces::OutputSink;

#[derive(Clone, Debug)]
pub struct DebugOutputSink {
	pub pretty: bool,
	pub last_json: Option<String>,
}

impl Default for DebugOutputSink {
	fn default() -> Self {
		Self {
			pretty: true,
			last_json: None,
		}
	}
}

impl DebugOutputSink {
	pub fn take_last_json(&mut self) -> Option<String> {
		self.last_json.take()
	}
}

impl OutputSink<UNMotionFrame> for DebugOutputSink {
	fn send(&mut self, frame: &UNMotionFrame) -> anyhow::Result<()> {
		let json = if self.pretty {
			serde_json::to_string_pretty(frame)
		} else {
			serde_json::to_string(frame)
		}
		.context("debug frame json serialize failed")?;

		println!("{json}");
		self.last_json = Some(json);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sink_emits_json() {
		let mut sink = DebugOutputSink::default();
		let frame = UNMotionFrame::new(1);
		sink.send(&frame).expect("debug sink should serialize");
		let json = sink.take_last_json().expect("json should be captured");
		assert!(json.contains("\"sequence\": 1"));
	}
}
