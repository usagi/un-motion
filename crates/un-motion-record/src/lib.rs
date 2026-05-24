use std::io::{ErrorKind, Read, Write};

use anyhow::Context;
use un_motion_frame::UNMotionFrame;
use un_motion_interfaces::OutputSink;

pub struct MessagePackStreamRecorder<W: Write> {
	writer: W,
}

impl<W: Write> MessagePackStreamRecorder<W> {
	pub fn new(writer: W) -> Self {
		Self { writer }
	}

	pub fn write_frame(&mut self, frame: &UNMotionFrame) -> anyhow::Result<()> {
		let payload = rmp_serde::to_vec_named(frame).context("messagepack encode failed")?;
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

	loop {
		let mut len_buf = [0_u8; 4];
		match reader.read_exact(&mut len_buf) {
			Ok(()) => {}
			Err(err) if err.kind() == ErrorKind::UnexpectedEof => break,
			Err(err) => return Err(err).context("failed to read frame length prefix"),
		}

		let payload_len = u32::from_le_bytes(len_buf) as usize;
		let mut payload = vec![0_u8; payload_len];
		reader.read_exact(&mut payload).context("failed to read frame payload")?;

		let frame: UNMotionFrame = rmp_serde::from_slice(&payload).context("messagepack decode failed")?;
		out.push(frame);
	}

	Ok(out)
}

pub fn replay_framed_stream_to_sink<R: Read, S: OutputSink<UNMotionFrame>>(reader: R, sink: &mut S) -> anyhow::Result<usize> {
	let frames = decode_framed_stream(reader)?;
	let mut sent_count = 0_usize;
	for frame in &frames {
		sink.send(frame)
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
}
