use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use un_motion_input_vmc::{OscMotionInputConfig, OscMotionInputSource, VmcInputFrame};

fn main() -> anyhow::Result<()> {
	let args = ProbeArgs::parse()?;
	let mut source = OscMotionInputSource::bind(OscMotionInputConfig::new(args.source_id, args.listen_addr))?;
	eprintln!("listening for OSC motion on {} as {}", source.local_addr()?, source.source_id());
	let mut jsonl = if let Some(path) = &args.output_jsonl {
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)?;
		}
		Some(BufWriter::new(File::create(path)?))
	} else {
		None
	};

	let started = Instant::now();
	let mut total_frames = 0_u64;
	let mut total_vmc_frames = 0_u64;
	let mut total_bones = 0_u64;
	let mut total_blendshapes = 0_u64;
	let mut total_decode_errors = 0_u64;
	loop {
		let batch = source.poll_batch()?;
		total_decode_errors += batch.decode_errors;
		for example in batch.decode_error_examples {
			eprintln!("decode_error {example}");
		}
		for frame in batch.frames {
			total_frames += 1;
			if frame.has_vmc_payload() {
				total_vmc_frames += 1;
			}
			total_bones += frame.bones.len() as u64;
			total_blendshapes += frame.blendshapes.len() as u64;
			println!("{}", frame.summary());
			if let Some(writer) = jsonl.as_mut() {
				write_frame_jsonl(writer, total_frames, &frame)?;
			}
			if args.once {
				return Ok(());
			}
		}
		if let Some(duration) = args.duration {
			if started.elapsed() >= duration {
				eprintln!(
					"summary frames={total_frames} vmc_frames={total_vmc_frames} bones={total_bones} blendshapes={total_blendshapes} decode_errors={total_decode_errors}"
				);
				return Ok(());
			}
		}
		thread::sleep(Duration::from_millis(5));
	}
}

struct ProbeArgs {
	listen_addr: SocketAddr,
	source_id: String,
	duration: Option<Duration>,
	once: bool,
	output_jsonl: Option<PathBuf>,
}

impl ProbeArgs {
	fn parse() -> anyhow::Result<Self> {
		let mut listen_addr: SocketAddr = "127.0.0.1:39540".parse()?;
		let mut source_id = "osc:probe".to_string();
		let mut duration = None;
		let mut once = false;
		let mut output_jsonl = None;
		let mut args = std::env::args().skip(1);
		while let Some(arg) = args.next() {
			match arg.as_str() {
				"--addr" | "--listen" => {
					let value = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
					listen_addr = value.parse()?;
				}
				"--source-id" => {
					source_id = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
				}
				"--duration-ms" => {
					let value = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
					duration = Some(Duration::from_millis(value.parse()?));
				}
				"--output-jsonl" => {
					output_jsonl = Some(PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?));
				}
				"--once" => once = true,
				"--help" | "-h" => {
					print_help();
					std::process::exit(0);
				}
				_ => anyhow::bail!("unknown argument: {arg}"),
			}
		}
		Ok(Self {
			listen_addr,
			source_id,
			duration,
			once,
			output_jsonl,
		})
	}
}

fn print_help() {
	let program = std::env::args()
		.next()
		.and_then(|path| {
			std::path::PathBuf::from(path)
				.file_name()
				.map(|name| name.to_string_lossy().to_string())
		})
		.unwrap_or_else(|| "un-motion-osc-listen-probe".to_string());
	println!(
		"usage: {program} [--addr 127.0.0.1:39539] [--source-id osc:probe] [--duration-ms 10000] [--output-jsonl target/vmc-captures/sample.jsonl] [--once]"
	);
}

fn write_frame_jsonl(writer: &mut BufWriter<File>, frame_index: u64, frame: &VmcInputFrame) -> anyhow::Result<()> {
	write!(
		writer,
		"{{\"frame\":{},\"timestampNs\":{},\"sourceId\":\"{}\",\"protocols\":\"{}\",\"ok\":{},\"root\":{},\"bones\":{},\"blendshapes\":{},\"blendApply\":{},\"messageCount\":{}",
		frame_index,
		frame.received_timestamp_ns,
		escape_json(&frame.source_id),
		frame.protocol_summary(),
		json_optional_i32(frame.ok),
		frame.root.as_ref().map(|_| 1).unwrap_or(0),
		frame.bones.len(),
		frame.blendshapes.len(),
		frame.blend_apply,
		frame.message_count
	)?;
	write!(writer, ",\"boneNames\":[")?;
	for (index, bone) in frame.bones.iter().enumerate() {
		if index > 0 {
			write!(writer, ",")?;
		}
		write!(writer, "\"{}\"", escape_json(&bone.name))?;
	}
	write!(writer, "],\"blendshapeNames\":[")?;
	for (index, blendshape) in frame.blendshapes.iter().enumerate() {
		if index > 0 {
			write!(writer, ",")?;
		}
		write!(writer, "\"{}\"", escape_json(&blendshape.name))?;
	}
	writeln!(writer, "]}}")?;
	Ok(())
}

fn json_optional_i32(value: Option<i32>) -> String {
	value.map(|value| value.to_string()).unwrap_or_else(|| "null".to_string())
}

fn escape_json(value: &str) -> String {
	value.replace('\\', "\\\\").replace('"', "\\\"")
}
