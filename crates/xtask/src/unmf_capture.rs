use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use un_motion_frame::UNMotionFrame;
use un_motion_frame_zenoh::{Subscriber, TopicMode, ZenohSubscriberBackend, ZenohTopicStrategy};

use super::absolutize;

#[derive(Debug)]
struct Args {
	base_key_expr: String,
	topic_mode: TopicMode,
	stream_id: Option<String>,
	require_body_bones: bool,
	timeout: Duration,
	output: PathBuf,
}

pub(super) fn run(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_args(raw_args)?;
	let strategy = ZenohTopicStrategy::new(args.base_key_expr.clone(), args.topic_mode);
	let subscribe_key_expr = strategy.subscribe_key_expr();
	let mut backend = ZenohSubscriberBackend::open_default().context("UNMF/Z capture Zenoh open failed")?;
	let subscriber =
		Subscriber::declare(&mut backend, strategy).with_context(|| format!("UNMF/Z capture subscribe failed on {subscribe_key_expr}"))?;
	let started = Instant::now();
	let frame = loop {
		if started.elapsed() >= args.timeout {
			bail!(
				"UNMF/Z capture timed out after {} ms on {subscribe_key_expr}",
				args.timeout.as_millis()
			);
		}
		match subscriber.recv_frame_timeout(Duration::from_millis(50)) {
			Ok(Some(frame)) if frame_matches(&frame, &args) => break frame,
			Ok(Some(_)) | Ok(None) => {}
			Err(error) => eprintln!("UNMF/Z capture decode error: {error}"),
		}
	};
	let output_path = absolutize(repo, &args.output);
	if let Some(parent) = output_path.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	let json = serde_json::to_string_pretty(&frame)?;
	fs::write(&output_path, format!("{json}\n")).with_context(|| format!("failed to write {}", output_path.display()))?;
	println!("captured UNMotionFrame from {} to {}", subscribe_key_expr, output_path.display());
	Ok(())
}

fn frame_matches(frame: &UNMotionFrame, args: &Args) -> bool {
	if let Some(stream_id) = args.stream_id.as_deref()
		&& frame.header.stream_id.as_deref() != Some(stream_id)
	{
		return false;
	}
	if args.require_body_bones {
		let body_bones = frame
			.body
			.as_ref()
			.and_then(|body| body.humanoid.as_ref())
			.map_or(0, |humanoid| humanoid.bones.len());
		if body_bones == 0 {
			return false;
		}
	}
	true
}

fn parse_args(raw_args: Vec<OsString>) -> Result<Args> {
	let mut args = Args {
		base_key_expr: "un-motion/frame".to_string(),
		topic_mode: TopicMode::Frame,
		stream_id: None,
		require_body_bones: false,
		timeout: Duration::from_secs(5),
		output: PathBuf::from("target/unmf-captures/frame.json"),
	};
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--key" | "--base-key" | "--base-key-expr" => {
				args.base_key_expr = next_value(&mut iter, "--key")?.to_string_lossy().into_owned()
			}
			"--topic-mode" | "--mode" => {
				let value = next_value(&mut iter, "--topic-mode")?.to_string_lossy().into_owned();
				args.topic_mode = parse_topic_mode(&value)?;
			}
			"--stream-id" => args.stream_id = Some(next_value(&mut iter, "--stream-id")?.to_string_lossy().into_owned()),
			"--require-body-bones" => args.require_body_bones = true,
			"--timeout-ms" => {
				args.timeout = Duration::from_millis(
					next_value(&mut iter, "--timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --timeout-ms")?,
				);
			}
			"--output" | "-o" => args.output = PathBuf::from(next_value(&mut iter, "--output")?),
			"--help" | "-h" => {
				print_usage();
				std::process::exit(0);
			}
			other if !other.starts_with('-') => args.base_key_expr = other.to_string(),
			other => bail!("unexpected unmf capture-frame argument: {other}"),
		}
	}
	if args.base_key_expr.trim().is_empty() {
		bail!("--key must not be empty");
	}
	if args.timeout.is_zero() {
		bail!("--timeout-ms must be greater than 0");
	}
	Ok(args)
}

fn next_value(iter: &mut impl Iterator<Item = OsString>, option: &str) -> Result<OsString> {
	iter.next().with_context(|| format!("missing value for {option}"))
}

fn parse_topic_mode(value: &str) -> Result<TopicMode> {
	match value {
		"frame" => Ok(TopicMode::Frame),
		"by-primary-source" | "primary-source" | "source" => Ok(TopicMode::ByPrimarySource),
		"by-stream-id" | "stream-id" | "stream" => Ok(TopicMode::ByStreamId),
		other => bail!("invalid --topic-mode: {other}"),
	}
}

fn print_usage() {
	eprintln!(
		"usage: cargo xtask unmf capture-frame [--key un-motion/frame] [--topic-mode frame|by-primary-source|by-stream-id] [--stream-id id] [--require-body-bones] [--output frame.json] [--timeout-ms 5000]"
	);
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn defaults_are_stable() {
		let args = parse_args(Vec::new()).expect("defaults");
		assert_eq!(args.base_key_expr, "un-motion/frame");
		assert_eq!(args.topic_mode, TopicMode::Frame);
		assert_eq!(args.timeout, Duration::from_secs(5));
		assert_eq!(args.output, PathBuf::from("target/unmf-captures/frame.json"));
		assert!(!args.require_body_bones);
	}

	#[test]
	fn parses_filters_and_output_options() {
		let args = parse_args(
			[
				"--key",
				"motion/**",
				"--topic-mode",
				"by-stream-id",
				"--stream-id",
				"main",
				"--require-body-bones",
				"--timeout-ms",
				"750",
				"-o",
				"frame.json",
			]
			.into_iter()
			.map(OsString::from)
			.collect(),
		)
		.expect("options");
		assert_eq!(args.base_key_expr, "motion/**");
		assert_eq!(args.topic_mode, TopicMode::ByStreamId);
		assert_eq!(args.stream_id.as_deref(), Some("main"));
		assert!(args.require_body_bones);
		assert_eq!(args.timeout, Duration::from_millis(750));
		assert_eq!(args.output, PathBuf::from("frame.json"));
	}

	#[test]
	fn frame_filters_reject_wrong_stream_and_missing_body() {
		let mut frame = UNMotionFrame::new(1);
		frame.header.stream_id = Some("main".to_string());
		let mut args = parse_args(vec!["--stream-id".into(), "other".into()]).expect("args");
		assert!(!frame_matches(&frame, &args));
		args.stream_id = Some("main".to_string());
		assert!(frame_matches(&frame, &args));
		args.require_body_bones = true;
		assert!(!frame_matches(&frame, &args));
	}

	#[test]
	fn rejects_zero_timeout_and_unknown_mode() {
		assert!(parse_args(vec!["--timeout-ms".into(), "0".into()]).is_err());
		assert!(parse_args(vec!["--topic-mode".into(), "unknown".into()]).is_err());
	}
}
