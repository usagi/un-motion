use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use un_motion_input_ifacialmocap::{IFACIALMOCAP_UDP_PORT, IfacialMocapInputConfig, IfacialMocapInputSource, IfacialMocapTransport};

fn main() -> anyhow::Result<()> {
	let args = ProbeArgs::parse()?;
	let mut source = IfacialMocapInputSource::bind(IfacialMocapInputConfig {
		source_id: args.source_id,
		bind_addr: args.bind_addr,
		remote_addr: args.remote_addr,
		transport: args.transport,
		start_command: args.start_command,
	})?;
	eprintln!("listening for iFacialMocap on {} as {}", source.local_addr()?, source.source_id());

	let started = Instant::now();
	let mut total_frames = 0_u64;
	let mut total_expressions = 0_u64;
	let mut total_decode_errors = 0_u64;
	let mut sequence = 0_u64;
	loop {
		let batch = source.poll_batch()?;
		total_decode_errors += batch.decode_errors;
		for example in batch.decode_error_examples {
			eprintln!("decode_error {example}");
		}
		for frame in batch.frames {
			total_frames += 1;
			total_expressions += frame.expressions.len() as u64;
			sequence += 1;
			let unmotion = frame.to_unmotion_frame(sequence);
			println!(
				"source={} head=({:.3},{:.3},{:.3}) leftEye=({:.3},{:.3},{:.3}) rightEye=({:.3},{:.3},{:.3}) confidence={:.3} expressions={} signals={}",
				frame.source_id,
				frame.head.yaw,
				frame.head.pitch,
				frame.head.roll,
				frame.left_eye.yaw,
				frame.left_eye.pitch,
				frame.left_eye.roll,
				frame.right_eye.yaw,
				frame.right_eye.pitch,
				frame.right_eye.roll,
				frame.confidence,
				frame.expressions.len(),
				unmotion.signals.len()
			);
			for (name, value) in frame.expressions.iter().take(args.expression_limit) {
				println!("  expr {name}={value:.3}");
			}
			if args.once {
				return Ok(());
			}
		}
		if started.elapsed() >= args.duration {
			eprintln!("summary frames={total_frames} expressions={total_expressions} decode_errors={total_decode_errors}");
			return Ok(());
		}
		thread::sleep(Duration::from_millis(5));
	}
}

struct ProbeArgs {
	bind_addr: SocketAddr,
	remote_addr: Option<SocketAddr>,
	transport: IfacialMocapTransport,
	source_id: String,
	start_command: Option<String>,
	duration: Duration,
	once: bool,
	expression_limit: usize,
}

impl ProbeArgs {
	fn parse() -> anyhow::Result<Self> {
		let mut bind_addr: SocketAddr = format!("0.0.0.0:{IFACIALMOCAP_UDP_PORT}").parse()?;
		let mut remote_addr = None;
		let mut transport = IfacialMocapTransport::Udp;
		let mut source_id = "ifacialmocap:probe".to_string();
		let mut start_command = Some("iFacialMocap_sahne".to_string());
		let mut duration = Duration::from_millis(10_000);
		let mut once = false;
		let mut expression_limit = 8_usize;

		let mut args = std::env::args().skip(1);
		while let Some(arg) = args.next() {
			match arg.as_str() {
				"--addr" | "--listen" => {
					let value = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
					bind_addr = value.parse()?;
				}
				"--remote" => {
					let value = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
					remote_addr = Some(value.parse()?);
				}
				"--tcp" => transport = IfacialMocapTransport::Tcp,
				"--udp" => transport = IfacialMocapTransport::Udp,
				"--source-id" => {
					source_id = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
				}
				"--start-command" => {
					start_command = Some(args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?);
				}
				"--no-start-command" => start_command = None,
				"--duration-ms" => {
					let value = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
					duration = Duration::from_millis(value.parse()?);
				}
				"--expression-limit" => {
					let value = args.next().ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
					expression_limit = value.parse()?;
				}
				"--once" => once = true,
				"--help" | "-h" => {
					print_help();
					std::process::exit(0);
				}
				_ => anyhow::bail!("unknown argument: {arg}"),
			}
		}

		if transport == IfacialMocapTransport::Tcp && remote_addr.is_none() {
			anyhow::bail!("--tcp requires --remote host:port");
		}

		Ok(Self {
			bind_addr,
			remote_addr,
			transport,
			source_id,
			start_command,
			duration,
			once,
			expression_limit,
		})
	}
}

fn print_help() {
	println!(
		"usage: un-motion-ifacialmocap-listen-probe [--addr 0.0.0.0:49983] [--remote host:port] [--udp|--tcp] [--no-start-command] [--duration-ms 10000] [--once]"
	);
}
