use std::io::{ErrorKind, Read, Write};

use anyhow::Context;
use un_motion_frame::UNMotionFrame;
use un_motion_interfaces::OutputSink;

pub const MAX_FRAME_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

pub struct MessagePackStreamRecorder<W: Write> {
	writer: W,
}

impl<W: Write> MessagePackStreamRecorder<W> {
	pub fn new(writer: W) -> Self {
		Self { writer }
	}

	pub fn write_frame(&mut self, frame: &UNMotionFrame) -> anyhow::Result<()> {
		let payload = rmp_serde::to_vec_named(frame).context("messagepack encode failed")?;
		if payload.len() > MAX_FRAME_PAYLOAD_BYTES {
			anyhow::bail!("frame payload exceeds {MAX_FRAME_PAYLOAD_BYTES} byte limit");
		}
		let payload_len: u32 = payload.len().try_into().context("frame payload too large for u32 length prefix")?;

		self.writer
			.write_all(&payload_len.to_le_bytes())
			.context("failed to write frame length prefix")?;
		self.writer.write_all(&payload).context("failed to write frame payload")?;
		Ok(())
	}

	pub fn flush(&mut self) -> anyhow::Result<()> {
		self.writer.flush().context("failed to flush recorder writer")?;
		Ok(())
	}

	pub fn into_inner(self) -> W {
		self.writer
	}
}

pub fn decode_framed_stream<R: Read>(mut reader: R) -> anyhow::Result<Vec<UNMotionFrame>> {
	let mut out = Vec::new();

	while let Some(frame) = decode_next_frame(&mut reader)? {
		out.push(frame);
	}

	Ok(out)
}

fn decode_next_frame<R: Read>(reader: &mut R) -> anyhow::Result<Option<UNMotionFrame>> {
	let Some(len_buf) = read_frame_length_prefix(reader)? else {
		return Ok(None);
	};
	let payload_len = u32::from_le_bytes(len_buf) as usize;
	if payload_len > MAX_FRAME_PAYLOAD_BYTES {
		anyhow::bail!("frame payload exceeds {MAX_FRAME_PAYLOAD_BYTES} byte limit: {payload_len}");
	}
	let mut payload = vec![0_u8; payload_len];
	reader.read_exact(&mut payload).context("failed to read frame payload")?;
	let frame = rmp_serde::from_slice(&payload).context("messagepack decode failed")?;
	Ok(Some(frame))
}

fn read_frame_length_prefix<R: Read>(reader: &mut R) -> anyhow::Result<Option<[u8; 4]>> {
	let mut prefix = [0u8; 4];
	loop {
		match reader.read(&mut prefix[..1]) {
			Ok(0) => return Ok(None),
			Ok(_) => break,
			Err(error) if error.kind() == ErrorKind::Interrupted => continue,
			Err(error) => return Err(error).context("failed to read frame length prefix"),
		}
	}
	reader.read_exact(&mut prefix[1..]).context("truncated frame length prefix")?;
	Ok(Some(prefix))
}

pub fn replay_framed_stream_to_sink<R: Read, S: OutputSink<UNMotionFrame>>(mut reader: R, sink: &mut S) -> anyhow::Result<usize> {
	let mut sent_count = 0_usize;
	while let Some(frame) = decode_next_frame(&mut reader)? {
		sink.send(&frame)
			.with_context(|| format!("failed to send replay frame sequence={}", frame.header.sequence))?;
		sent_count += 1;
	}
	Ok(sent_count)
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_interfaces::OutputSink;

	#[derive(Default)]
	struct CaptureSink {
		sequences: Vec<u64>,
	}

	impl OutputSink<UNMotionFrame> for CaptureSink {
		fn send(&mut self, frame: &UNMotionFrame) -> anyhow::Result<()> {
			self.sequences.push(frame.header.sequence);
			Ok(())
		}
	}

	#[test]
	fn record_and_decode_keeps_order_and_count() {
		let mut recorder = MessagePackStreamRecorder::new(Vec::<u8>::new());
		recorder.write_frame(&UNMotionFrame::new(10)).expect("write frame 10");
		recorder.write_frame(&UNMotionFrame::new(11)).expect("write frame 11");
		recorder.flush().expect("flush");

		let bytes = recorder.into_inner();
		let frames = decode_framed_stream(bytes.as_slice()).expect("decode stream");

		assert_eq!(frames.len(), 2);
		assert_eq!(frames[0].header.sequence, 10);
		assert_eq!(frames[1].header.sequence, 11);
	}

	#[test]
	fn decode_fails_on_truncated_payload() {
		let frame = UNMotionFrame::new(1);
		let payload = rmp_serde::to_vec_named(&frame).expect("encode");
		let payload_len: u32 = payload.len().try_into().expect("u32 length");
		let mut bytes = Vec::new();
		bytes.extend_from_slice(&payload_len.to_le_bytes());
		bytes.extend_from_slice(&payload[..payload.len() / 2]);

		let err = decode_framed_stream(bytes.as_slice()).expect_err("should fail on truncated payload");
		assert!(err.to_string().contains("failed to read frame payload"));
	}

	#[test]
	fn decode_rejects_oversized_payload_before_allocation() {
		let oversized = (MAX_FRAME_PAYLOAD_BYTES as u32 + 1).to_le_bytes();
		let error = decode_framed_stream(oversized.as_slice()).expect_err("oversized payload must fail");
		assert!(error.to_string().contains("frame payload exceeds"));
	}

	#[test]
	fn decode_rejects_partial_length_prefixes() {
		for length in 1..4 {
			let error = decode_framed_stream(&[0u8; 3][..length]).expect_err("partial prefix must fail");
			assert!(error.to_string().contains("truncated frame length prefix"));
		}
	}

	#[test]
	fn replay_to_sink_keeps_order_and_count() {
		let mut recorder = MessagePackStreamRecorder::new(Vec::<u8>::new());
		recorder.write_frame(&UNMotionFrame::new(101)).expect("write frame 101");
		recorder.write_frame(&UNMotionFrame::new(102)).expect("write frame 102");
		recorder.flush().expect("flush");

		let bytes = recorder.into_inner();
		let mut sink = CaptureSink::default();
		let sent = replay_framed_stream_to_sink(bytes.as_slice(), &mut sink).expect("replay to sink");

		assert_eq!(sent, 2);
		assert_eq!(sink.sequences, vec![101, 102]);
	}

	#[test]
	fn replay_stops_reading_after_sink_failure() {
		struct RejectingSink;
		impl OutputSink<UNMotionFrame> for RejectingSink {
			fn send(&mut self, _frame: &UNMotionFrame) -> anyhow::Result<()> {
				anyhow::bail!("sink rejected frame")
			}
		}

		let mut recorder = MessagePackStreamRecorder::new(Vec::<u8>::new());
		recorder.write_frame(&UNMotionFrame::new(1)).expect("write frame");
		let mut bytes = recorder.into_inner();
		bytes.extend_from_slice(&(MAX_FRAME_PAYLOAD_BYTES as u32 + 1).to_le_bytes());

		let error = replay_framed_stream_to_sink(bytes.as_slice(), &mut RejectingSink).expect_err("sink must fail first");

		assert!(error.to_string().contains("failed to send replay frame sequence=1"));
	}
}
