use std::{
	env,
	ffi::OsString,
	fs::{self, File},
	io::{BufWriter, ErrorKind, Read, Write},
	net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
	path::{Path, PathBuf},
	process::{Child, Command, Stdio},
	time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use flate2::read::ZlibDecoder;
use image::{GenericImage, ImageBuffer, Rgba, imageops::FilterType};
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, decoder, encoder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use un_motion_frame::{
	ExpressionSample, Quatf, SampleState, TrackingState, TransformSample as UnmotionTransformSample, UNMotionFrame, Vec3f,
};
use un_motion_frame_zenoh::{Subscriber, TopicMode, ZenohSubscriberBackend, ZenohTopicStrategy};
use un_motion_input_vmc::{VmcInputFrame, VmcPacketDecoder, VmcTransform};

fn main() -> Result<()> {
	let repo = repo_root()?;
	let mut args = env::args_os().skip(1);
	let Some(command) = args.next() else {
		print_usage();
		bail!("missing xtask command");
	};
	match command.to_string_lossy().as_ref() {
		"fmt" => cargo_simple(&repo, "fmt", ["fmt", "--all"]),
		"check" => cargo_simple(&repo, "check", ["check", "--workspace"]),
		"test" => cargo_simple(&repo, "test", ["test", "--workspace"]),
		"frontend" => frontend(&repo, args.collect()),
		"core" => core(&repo, args.collect()),
		"make-release-package" => make_release_package(&repo, args.collect()),
		"license-report" => license_report(&repo, args.collect()),
		"verify" => verify(&repo, args.collect()),
		"image" => image_command(&repo, args.collect()),
		"run-capturer" => run_capturer(&repo, args.collect()),
		"mediapipe" => mediapipe(&repo, args.collect()),
		"vmc" => vmc(&repo, args.collect()),
		"unmf" | "zenoh" => unmf(&repo, args.collect()),
		"research" => research(&repo, args.collect()),
		"help" | "--help" | "-h" => {
			print_usage();
			Ok(())
		}
		other => bail!("unknown xtask command: {other}"),
	}
}

fn cargo_simple<const N: usize>(repo: &Path, label: &str, args: [&str; N]) -> Result<()> {
	run(repo, "cargo", args).with_context(|| format!("cargo xtask {label} failed"))
}

#[derive(Debug)]
struct RunCapturerArgs {
	profile: String,
	profile_root: Option<PathBuf>,
	release: bool,
	log: Option<String>,
	experimental_flicker_test: bool,
	passthrough: Vec<OsString>,
}

#[derive(Debug, Clone)]
struct CapturerProfileEntry {
	id: String,
	name: String,
	path: PathBuf,
}

fn run_capturer(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	if raw_args
		.first()
		.is_some_and(|arg| matches!(arg.to_string_lossy().as_ref(), "--help" | "-h" | "help"))
	{
		print_run_capturer_usage();
		return Ok(());
	}

	let args = parse_run_capturer_args(raw_args)?;
	let explicit_profile_root = args.profile_root.is_some();
	let profile_root = args.profile_root.unwrap_or_else(default_un_motion_config_dir);
	let profile = resolve_capturer_profile(&profile_root, &args.profile)?;

	eprintln!(
		"resolved capturer profile '{}' ({}) from {}",
		profile.name,
		profile.id,
		profile.path.display()
	);

	let mut cmd = Command::new(resolve_tool("cargo"));
	cmd.current_dir(repo).arg("run");
	if args.release {
		cmd.arg("--release");
	}
	cmd.args(["-p", "un-motion-capturer", "--", "--active-profile"]).arg(&profile.id);
	if explicit_profile_root {
		cmd.arg("--profile-root").arg(&profile_root);
	}
	if args.experimental_flicker_test {
		cmd.arg("--experimental-flicker-test");
	}
	cmd.args(args.passthrough);
	if let Some(log) = args.log {
		cmd.env("UN_MOTION_LOG", log);
	}
	run_command("cargo run -p un-motion-capturer", &mut cmd)
}

fn parse_run_capturer_args(raw_args: Vec<OsString>) -> Result<RunCapturerArgs> {
	let mut profile = None;
	let mut profile_root = None;
	let mut release = false;
	let mut log = None;
	let mut experimental_flicker_test = false;
	let mut passthrough = Vec::new();
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--profile" => profile = Some(os_string_to_string(next_value(&mut iter, "--profile")?, "--profile")?),
			"--profile-root" => profile_root = Some(PathBuf::from(next_value(&mut iter, "--profile-root")?)),
			"--release" => release = true,
			"--log" => log = Some(os_string_to_string(next_value(&mut iter, "--log")?, "--log")?),
			"--experimental-flicker-test" => experimental_flicker_test = true,
			"--" => {
				passthrough.extend(iter);
				break;
			}
			"--help" | "-h" | "help" => {
				print_run_capturer_usage();
				bail!("run-capturer help requested")
			}
			other => bail!("unknown run-capturer option: {other}"),
		}
	}
	let profile = profile.context("missing --profile value, for example: cargo xtask run-capturer --profile Dev1")?;
	Ok(RunCapturerArgs {
		profile,
		profile_root,
		release,
		log,
		experimental_flicker_test,
		passthrough,
	})
}

fn resolve_capturer_profile(profile_root: &Path, selector: &str) -> Result<CapturerProfileEntry> {
	let profiles_dir = profile_root.join("profiles");
	let profiles = load_capturer_profiles(&profiles_dir)?;
	let matches: Vec<_> = profiles
		.iter()
		.filter(|profile| profile.id == selector || profile.name.eq_ignore_ascii_case(selector))
		.cloned()
		.collect();
	match matches.as_slice() {
		[profile] => Ok(profile.clone()),
		[] => {
			let available = if profiles.is_empty() {
				"(none)".to_string()
			} else {
				profiles
					.iter()
					.map(|profile| format!("{} ({})", profile.name, profile.id))
					.collect::<Vec<_>>()
					.join(", ")
			};
			bail!(
				"profile '{selector}' was not found under {}; available: {available}",
				profiles_dir.display()
			)
		}
		multiple => {
			let paths = multiple
				.iter()
				.map(|profile| profile.path.display().to_string())
				.collect::<Vec<_>>()
				.join(", ");
			bail!("profile selector '{selector}' matched multiple profiles: {paths}")
		}
	}
}

fn load_capturer_profiles(profiles_dir: &Path) -> Result<Vec<CapturerProfileEntry>> {
	let mut profiles = Vec::new();
	let entries = fs::read_dir(profiles_dir).with_context(|| format!("failed to read {}", profiles_dir.display()))?;
	for entry in entries {
		let entry = entry?;
		let path = entry.path();
		if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
			continue;
		}
		let text = fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
		let value: toml::Value = toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
		let id = value
			.get("id")
			.and_then(toml::Value::as_str)
			.with_context(|| format!("profile file has no string id: {}", path.display()))?
			.to_string();
		let name = value.get("name").and_then(toml::Value::as_str).unwrap_or(&id).to_string();
		profiles.push(CapturerProfileEntry { id, name, path });
	}
	profiles.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
	Ok(profiles)
}

fn default_un_motion_config_dir() -> PathBuf {
	if let Some(path) = env::var_os("UN_MOTION_CONFIG_DIR") {
		return PathBuf::from(path);
	}
	if cfg!(windows) {
		if let Some(path) = env::var_os("APPDATA") {
			return PathBuf::from(path).join("UN Motion");
		}
	}
	if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
		return PathBuf::from(path).join("un-motion");
	}
	if let Some(path) = env::var_os("HOME") {
		return PathBuf::from(path).join(".config").join("un-motion");
	}
	PathBuf::from("un-motion-config")
}

fn os_string_to_string(value: OsString, name: &str) -> Result<String> {
	value.into_string().map_err(|_| anyhow::anyhow!("{name} must be valid UTF-8"))
}

fn print_run_capturer_usage() {
	eprintln!(
		"usage: cargo xtask run-capturer --profile Dev1 [--profile-root PATH] [--release] [--log FILTER] [--experimental-flicker-test] [-- capturer args]"
	);
}

fn frontend(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let subcommand = args.first().map(|arg| arg.to_string_lossy()).unwrap_or_else(|| "build".into());
	match subcommand.as_ref() {
		"build" => run(repo.join("apps/un-motion-supervisor"), "npm", ["run", "build"]),
		"install" | "ci" => run(repo.join("apps/un-motion-supervisor"), "npm", ["ci"]),
		"--help" | "-h" | "help" => {
			eprintln!("usage: cargo xtask frontend <build|ci>");
			Ok(())
		}
		other => bail!("unknown frontend subcommand: {other}"),
	}
}

#[derive(Debug)]
struct CoreSmokeArgs {
	core_exe: Option<PathBuf>,
	timeout: Duration,
	keep_temp: bool,
}

#[derive(Debug)]
struct CoreExternalVmcSmokeArgs {
	core: CoreSmokeArgs,
	listen_addr: SocketAddr,
	target_addr: SocketAddr,
	observe_addr: SocketAddr,
	label: String,
	mirror_correction_enabled: bool,
}

#[derive(Debug)]
struct CoreExternalIfacialMocapSmokeArgs {
	core: CoreSmokeArgs,
	listen_addr: SocketAddr,
	target_addr: SocketAddr,
	observe_addr: SocketAddr,
	label: String,
}

#[derive(Debug)]
struct CoreExternalVmcStabilityArgs {
	core: CoreSmokeArgs,
	listen_addr: SocketAddr,
	output_bind_addr: SocketAddr,
	label: String,
	duration: Duration,
	output: Option<PathBuf>,
	min_samples: usize,
	mirror_correction_enabled: bool,
}

#[derive(Debug)]
struct CoreExternalVmcCompareArgs {
	core: CoreSmokeArgs,
	listen_addr: SocketAddr,
	output_bind_addr: SocketAddr,
	label: String,
	duration: Duration,
	output_dir: PathBuf,
	min_samples: usize,
	mirror_correction_enabled: bool,
}

#[derive(Debug)]
struct CoreHttpResponse {
	status: u16,
	body: String,
}

fn core(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let subcommand = args.first().map(|arg| arg.to_string_lossy()).unwrap_or_else(|| "smoke".into());
	match subcommand.as_ref() {
		"smoke" => core_smoke(repo, args.get(1..).unwrap_or_default().to_vec()),
		"vmc-smoke" => core_vmc_smoke(repo, args.get(1..).unwrap_or_default().to_vec()),
		"vmc-mirror-smoke" => core_vmc_mirror_smoke(repo, args.get(1..).unwrap_or_default().to_vec()),
		"external-vmc-smoke" => core_external_vmc_smoke(repo, args.get(1..).unwrap_or_default().to_vec()),
		"external-vmc-stability" => core_external_vmc_stability(repo, args.get(1..).unwrap_or_default().to_vec()),
		"external-vmc-compare" => core_external_vmc_compare(repo, args.get(1..).unwrap_or_default().to_vec()),
		"external-ifacialmocap-smoke" => core_external_ifacialmocap_smoke(repo, args.get(1..).unwrap_or_default().to_vec()),
		"lifecycle-smoke" => core_lifecycle_smoke(repo, args.get(1..).unwrap_or_default().to_vec()),
		"--help" | "-h" | "help" => {
			eprintln!(
				"usage: cargo xtask core <smoke|vmc-smoke|vmc-mirror-smoke|external-vmc-smoke|external-vmc-stability|external-vmc-compare|external-ifacialmocap-smoke|lifecycle-smoke> [--core-exe target/release/un-motion-core.exe] [--timeout-ms 30000] [--keep-temp]"
			);
			Ok(())
		}
		other => bail!("unknown core subcommand: {other}"),
	}
}

fn core_smoke(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_smoke_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-smoke-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let addr = free_tcp_addr()?;
	let mut child = spawn_core_smoke_process(repo, &args, &temp_root, addr)?;
	let started = Instant::now();
	let result = (|| {
		wait_for_core_health(addr, args.timeout)?;
		let initial_status = core_http_json(addr, "GET", "/api/status", None)?;
		let document = smoke_profile_document_json();
		core_http_json(
			addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": document }).to_string()),
		)?;
		let started_status = core_http_json(addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("core smoke start did not report running: {started_status}");
		}
		let snapshot = core_http_json(addr, "GET", "/api/runtime/snapshot", None)?;
		if snapshot
			.pointer("/snapshot/status/activeProfileId")
			.and_then(serde_json::Value::as_str)
			!= Some("smoke")
		{
			bail!("core smoke snapshot did not use smoke profile: {snapshot}");
		}
		let stopped_status = core_http_json(addr, "POST", "/api/runtime/stop", None)?;
		if stopped_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(true)
		{
			bail!("core smoke stop still reports running: {stopped_status}");
		}
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::json!({
				"bindAddr": addr.to_string(),
				"workspace": temp_root.display().to_string(),
				"durationMs": started.elapsed().as_millis() as u64,
				"initialHealth": initial_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
				"startedHealth": started_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
				"stoppedHealth": stopped_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
			}))?
		);
		Ok(())
	})();
	stop_core_smoke_process(&mut child);
	if !args.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn core_lifecycle_smoke(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_smoke_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-lifecycle-smoke-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let addr = free_tcp_addr()?;
	let started = Instant::now();
	let mut child = Some(spawn_core_smoke_process(repo, &args, &temp_root, addr)?);
	let result = (|| {
		wait_for_core_health(addr, args.timeout)?;
		core_http_json(
			addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": smoke_profile_document_json() }).to_string()),
		)?;
		let started_status = core_http_json(addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("core lifecycle smoke start did not report running: {started_status}");
		}
		let stopped_status = core_http_json(addr, "POST", "/api/runtime/stop", None)?;
		if stopped_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(true)
		{
			bail!("core lifecycle smoke stop did not report stopped: {stopped_status}");
		}
		let mut first_child = child.take().context("core lifecycle smoke process missing before exit")?;
		stop_core_smoke_process(&mut first_child);
		wait_for_core_shutdown(addr, Duration::from_secs(3))?;

		child = Some(spawn_core_smoke_process(repo, &args, &temp_root, addr)?);
		wait_for_core_health(addr, args.timeout)?;
		let reopened_document = core_http_json(addr, "GET", "/api/profiles/document", None)?;
		if reopened_document
			.pointer("/selection/selectedProfileId")
			.and_then(serde_json::Value::as_str)
			!= Some("smoke")
		{
			bail!("core lifecycle smoke reopened with wrong active profile: {reopened_document}");
		}
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::json!({
				"bindAddr": addr.to_string(),
				"workspace": temp_root.display().to_string(),
				"durationMs": started.elapsed().as_millis() as u64,
				"activeProfileAfterReopen": reopened_document.pointer("/selection/selectedProfileId").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
			}))?
		);
		Ok(())
	})();
	if let Some(mut child) = child {
		stop_core_smoke_process(&mut child);
	}
	if !args.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn core_vmc_smoke(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_smoke_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-vmc-smoke-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let api_addr = free_tcp_addr()?;
	let vmc_listen_addr = free_udp_addr()?;
	let output_socket = UdpSocket::bind("127.0.0.1:0").context("failed to bind VMC smoke output receiver")?;
	output_socket
		.set_read_timeout(Some(Duration::from_millis(50)))
		.context("failed to configure VMC smoke output receiver")?;
	let vmc_output_addr = output_socket
		.local_addr()
		.context("failed to read VMC smoke output receiver address")?;
	let mut child = spawn_core_smoke_process(repo, &args, &temp_root, api_addr)?;
	let started = Instant::now();
	let result = (|| {
		wait_for_core_health(api_addr, args.timeout)?;
		let document = smoke_vmc_profile_document_json(vmc_listen_addr, vmc_output_addr);
		core_http_json(
			api_addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": document }).to_string()),
		)?;
		let started_status = core_http_json(api_addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("core VMC smoke start did not report running: {started_status}");
		}
		let sender = UdpSocket::bind("127.0.0.1:0").context("failed to bind VMC smoke sender")?;
		let datagram = smoke_vmc_datagram()?;
		for _ in 0..5 {
			sender
				.send_to(&datagram, vmc_listen_addr)
				.with_context(|| format!("failed to send VMC smoke datagram to {vmc_listen_addr}"))?;
			std::thread::sleep(Duration::from_millis(10));
		}
		let output = wait_for_vmc_smoke_output(&output_socket, Duration::from_secs(3))?;
		let stopped_status = core_http_json(api_addr, "POST", "/api/runtime/stop", None)?;
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::json!({
				"apiAddr": api_addr.to_string(),
				"vmcListenAddr": vmc_listen_addr.to_string(),
				"vmcOutputAddr": vmc_output_addr.to_string(),
				"workspace": temp_root.display().to_string(),
				"durationMs": started.elapsed().as_millis() as u64,
				"outputMessages": output.messages,
				"hasHead": output.has_head,
				"hasEyeBlinkLeft": output.has_eye_blink_left,
				"hasJawOpen": output.has_jaw_open,
				"startedHealth": started_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
				"stoppedHealth": stopped_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
			}))?
		);
		Ok(())
	})();
	stop_core_smoke_process(&mut child);
	if !args.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn core_vmc_mirror_smoke(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_smoke_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-vmc-mirror-smoke-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let api_addr = free_tcp_addr()?;
	let vmc_listen_addr = free_udp_addr()?;
	let output_socket = UdpSocket::bind("127.0.0.1:0").context("failed to bind VMC mirror smoke output receiver")?;
	output_socket
		.set_read_timeout(Some(Duration::from_millis(50)))
		.context("failed to configure VMC mirror smoke output receiver")?;
	let vmc_output_addr = output_socket
		.local_addr()
		.context("failed to read VMC mirror smoke output receiver address")?;
	let mut child = spawn_core_smoke_process(repo, &args, &temp_root, api_addr)?;
	let started = Instant::now();
	let result = (|| {
		wait_for_core_health(api_addr, args.timeout)?;
		let document = smoke_vmc_mirror_profile_document_json(vmc_listen_addr, vmc_output_addr);
		core_http_json(
			api_addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": document }).to_string()),
		)?;
		let started_status = core_http_json(api_addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("core VMC mirror smoke start did not report running: {started_status}");
		}
		let sender = UdpSocket::bind("127.0.0.1:0").context("failed to bind VMC mirror smoke sender")?;
		let datagram = smoke_vmc_mirror_datagram()?;
		for _ in 0..5 {
			sender
				.send_to(&datagram, vmc_listen_addr)
				.with_context(|| format!("failed to send VMC mirror smoke datagram to {vmc_listen_addr}"))?;
			std::thread::sleep(Duration::from_millis(10));
		}
		let output = wait_for_vmc_mirror_smoke_output(&output_socket, Duration::from_secs(3))?;
		let stopped_status = core_http_json(api_addr, "POST", "/api/runtime/stop", None)?;
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::json!({
				"apiAddr": api_addr.to_string(),
				"vmcListenAddr": vmc_listen_addr.to_string(),
				"vmcOutputAddr": vmc_output_addr.to_string(),
				"workspace": temp_root.display().to_string(),
				"durationMs": started.elapsed().as_millis() as u64,
				"outputMessages": output.messages,
				"rootX": output.root_x,
				"headX": output.head_x,
				"hasRightHand": output.has_right_hand,
				"hasEyeBlinkRight": output.has_eye_blink_right,
				"hasJawOpen": output.has_jaw_open,
				"startedHealth": started_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
				"stoppedHealth": stopped_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
			}))?
		);
		Ok(())
	})();
	stop_core_smoke_process(&mut child);
	if !args.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn core_external_vmc_smoke(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_external_vmc_smoke_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-external-vmc-smoke-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let api_addr = free_tcp_addr()?;
	let observe_socket = UdpSocket::bind(args.observe_addr)
		.with_context(|| format!("failed to bind external VMC observe receiver on {}", args.observe_addr))?;
	observe_socket
		.set_read_timeout(Some(Duration::from_millis(50)))
		.context("failed to configure external VMC observe receiver")?;
	let mut child = spawn_core_smoke_process(repo, &args.core, &temp_root, api_addr)?;
	let started = Instant::now();
	let result = (|| {
		wait_for_core_health(api_addr, args.core.timeout)?;
		let document = smoke_vmc_profile_document_json_with_options(
			args.listen_addr,
			args.target_addr,
			"external-vmc-smoke",
			&args.label,
			"xtask core external VMC smoke",
			args.mirror_correction_enabled,
		);
		core_http_json(
			api_addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": document }).to_string()),
		)?;
		let started_status = core_http_json(api_addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("external VMC smoke start did not report running: {started_status}");
		}
		let snapshot = wait_for_external_vmc_core_route(api_addr, args.core.timeout)?;
		let core_frames = snapshot
			.pointer("/snapshot/status/frameCount")
			.and_then(serde_json::Value::as_u64)
			.unwrap_or(0);
		let core_packets = snapshot
			.pointer("/snapshot/status/packetCount")
			.and_then(serde_json::Value::as_u64)
			.unwrap_or(0);
		let observed = wait_for_external_vmc_observed_output(&observe_socket, Duration::from_secs(5))?;
		let stopped_status = core_http_json(api_addr, "POST", "/api/runtime/stop", None)?;
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::json!({
				"label": args.label,
				"apiAddr": api_addr.to_string(),
				"listenAddr": args.listen_addr.to_string(),
				"targetAddr": args.target_addr.to_string(),
				"observeAddr": args.observe_addr.to_string(),
				"mirrorCorrectionEnabled": args.mirror_correction_enabled,
				"workspace": temp_root.display().to_string(),
				"durationMs": started.elapsed().as_millis() as u64,
				"coreFrames": core_frames,
				"corePackets": core_packets,
				"observedMessages": observed.messages,
				"observedBones": observed.bones,
				"observedBlendshapes": observed.blendshapes,
				"hasHead": observed.has_head,
				"hasBlendshape": observed.has_blendshape,
				"startedHealth": started_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
				"stoppedHealth": stopped_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
			}))?
		);
		Ok(())
	})();
	stop_core_smoke_process(&mut child);
	if !args.core.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn core_external_vmc_stability(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_external_vmc_stability_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-external-vmc-stability-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let api_addr = free_tcp_addr()?;
	let output_socket = UdpSocket::bind(args.output_bind_addr)
		.with_context(|| format!("failed to bind external VMC stability output receiver on {}", args.output_bind_addr))?;
	output_socket
		.set_read_timeout(Some(Duration::from_millis(50)))
		.context("failed to configure external VMC stability output receiver")?;
	let target_addr = output_socket
		.local_addr()
		.context("failed to resolve stability output receiver address")?;
	let mut child = spawn_core_smoke_process(repo, &args.core, &temp_root, api_addr)?;
	let result = (|| {
		wait_for_core_health(api_addr, args.core.timeout)?;
		let document = smoke_vmc_profile_document_json_with_options(
			args.listen_addr,
			target_addr,
			"external-vmc-stability",
			&args.label,
			"xtask core external VMC stability",
			args.mirror_correction_enabled,
		);
		core_http_json(
			api_addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": document }).to_string()),
		)?;
		let started_status = core_http_json(api_addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("external VMC stability start did not report running: {started_status}");
		}
		wait_for_external_vmc_core_route(api_addr, args.core.timeout)?;
		drain_udp_socket(&output_socket, "external VMC stability output receiver")?;
		output_socket
			.set_read_timeout(Some(Duration::from_millis(50)))
			.context("failed to restore external VMC stability output receiver timeout")?;
		let report = collect_vmc_stability_report(
			target_addr.to_string(),
			args.label.clone(),
			&output_socket,
			args.duration,
			args.min_samples,
		)?;
		let _ = core_http_json(api_addr, "POST", "/api/runtime/stop", None);
		write_and_print_vmc_stability_report(repo, args.output.as_deref(), &report)
	})();
	stop_core_smoke_process(&mut child);
	if !args.core.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn core_external_vmc_compare(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_external_vmc_compare_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-external-vmc-compare-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let api_addr = free_tcp_addr()?;
	let core_listen_addr = free_udp_addr()?;
	let input_socket = UdpSocket::bind(args.listen_addr)
		.with_context(|| format!("failed to bind external VMC compare input receiver on {}", args.listen_addr))?;
	input_socket
		.set_nonblocking(true)
		.context("failed to configure external VMC compare input receiver")?;
	let forward_socket = UdpSocket::bind("127.0.0.1:0").context("failed to bind external VMC compare forward socket")?;
	let output_socket = UdpSocket::bind(args.output_bind_addr)
		.with_context(|| format!("failed to bind external VMC compare output receiver on {}", args.output_bind_addr))?;
	output_socket
		.set_nonblocking(true)
		.context("failed to configure external VMC compare output receiver")?;
	let target_addr = output_socket
		.local_addr()
		.context("failed to resolve compare output receiver address")?;
	let mut child = spawn_core_smoke_process(repo, &args.core, &temp_root, api_addr)?;
	let result = (|| {
		wait_for_core_health(api_addr, args.core.timeout)?;
		let document = smoke_vmc_profile_document_json_with_options(
			core_listen_addr,
			target_addr,
			"external-vmc-compare",
			&args.label,
			"xtask core external VMC compare",
			args.mirror_correction_enabled,
		);
		core_http_json(
			api_addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": document }).to_string()),
		)?;
		let started_status = core_http_json(api_addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("external VMC compare start did not report running: {started_status}");
		}
		wait_for_vmc_compare_output_start(&input_socket, &forward_socket, core_listen_addr, &output_socket, args.core.timeout)?;

		let mut direct = VmcStabilityCollector::new(format!("{}-direct", args.label));
		let mut unmotion = VmcStabilityCollector::new(format!("{}-unmotion", args.label));
		let started = Instant::now();
		let mut buf = [0_u8; 65535];
		while started.elapsed() < args.duration {
			let mut activity = false;
			loop {
				match input_socket.recv_from(&mut buf) {
					Ok((len, _)) => {
						activity = true;
						forward_socket
							.send_to(&buf[..len], core_listen_addr)
							.with_context(|| format!("failed to forward VMC datagram to core input {core_listen_addr}"))?;
						direct.observe_datagram(&buf[..len], started.elapsed().as_secs_f64() * 1000.0, now_unix_ms() * 1_000_000);
					}
					Err(error) if error.kind() == ErrorKind::WouldBlock => break,
					Err(error) => return Err(error).context("external VMC compare input recv failed"),
				}
			}
			loop {
				match output_socket.recv_from(&mut buf) {
					Ok((len, _)) => {
						activity = true;
						unmotion.observe_datagram(&buf[..len], started.elapsed().as_secs_f64() * 1000.0, now_unix_ms() * 1_000_000);
					}
					Err(error) if error.kind() == ErrorKind::WouldBlock => break,
					Err(error) => return Err(error).context("external VMC compare output recv failed"),
				}
			}
			if !activity {
				std::thread::sleep(Duration::from_millis(1));
			}
		}
		let _ = core_http_json(api_addr, "POST", "/api/runtime/stop", None);
		let duration_ms = started.elapsed().as_millis() as u64;
		let direct_report = direct.into_report(args.listen_addr.to_string(), duration_ms, args.min_samples);
		let unmotion_report = unmotion.into_report(target_addr.to_string(), duration_ms, args.min_samples);
		let safe_label = sanitize_capture_label(&args.label);
		let output_dir = absolutize(repo, &args.output_dir);
		fs::create_dir_all(&output_dir).with_context(|| format!("failed to create {}", output_dir.display()))?;
		let direct_path = output_dir.join(format!("{safe_label}-direct.json"));
		let unmotion_path = output_dir.join(format!("{safe_label}-unmotion.json"));
		let summary_path = output_dir.join(format!("{safe_label}-compare.md"));
		write_vmc_stability_report(repo, &direct_path, &direct_report)?;
		write_vmc_stability_report(repo, &unmotion_path, &unmotion_report)?;
		let summary = render_vmc_stability_summary(&[direct_report, unmotion_report], 8);
		fs::write(&summary_path, summary.as_bytes()).with_context(|| format!("failed to write {}", summary_path.display()))?;
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::json!({
				"label": args.label,
				"listenAddr": args.listen_addr.to_string(),
				"coreListenAddr": core_listen_addr.to_string(),
				"targetAddr": target_addr.to_string(),
				"durationMs": duration_ms,
				"directReport": direct_path.display().to_string(),
				"unmotionReport": unmotion_path.display().to_string(),
				"summary": summary_path.display().to_string(),
			}))?
		);
		Ok(())
	})();
	stop_core_smoke_process(&mut child);
	if !args.core.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn core_external_ifacialmocap_smoke(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_core_external_ifacialmocap_smoke_args(raw_args)?;
	let temp_root = std::env::temp_dir().join(format!("unmotion-core-external-ifacialmocap-smoke-{}", now_unix_ms()));
	fs::create_dir_all(&temp_root).with_context(|| format!("failed to create {}", temp_root.display()))?;
	let api_addr = free_tcp_addr()?;
	let observe_socket = UdpSocket::bind(args.observe_addr)
		.with_context(|| format!("failed to bind external iFacialMocap observe receiver on {}", args.observe_addr))?;
	observe_socket
		.set_read_timeout(Some(Duration::from_millis(50)))
		.context("failed to configure external iFacialMocap observe receiver")?;
	let mut child = spawn_core_smoke_process(repo, &args.core, &temp_root, api_addr)?;
	let started = Instant::now();
	let result = (|| {
		wait_for_core_health(api_addr, args.core.timeout)?;
		let document = smoke_ifacialmocap_profile_document_json(args.listen_addr, args.target_addr, &args.label);
		core_http_json(
			api_addr,
			"PUT",
			"/api/profiles/document",
			Some(&serde_json::json!({ "selection": document }).to_string()),
		)?;
		let started_status = core_http_json(api_addr, "POST", "/api/runtime/start", None)?;
		if !started_status
			.pointer("/status/running")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false)
		{
			bail!("external iFacialMocap smoke start did not report running: {started_status}");
		}
		let snapshot = wait_for_external_vmc_core_route(api_addr, args.core.timeout)?;
		let core_frames = snapshot
			.pointer("/snapshot/status/frameCount")
			.and_then(serde_json::Value::as_u64)
			.unwrap_or(0);
		let core_packets = snapshot
			.pointer("/snapshot/status/packetCount")
			.and_then(serde_json::Value::as_u64)
			.unwrap_or(0);
		let observed = wait_for_external_vmc_observed_output(&observe_socket, Duration::from_secs(5))?;
		let stopped_status = core_http_json(api_addr, "POST", "/api/runtime/stop", None)?;
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::json!({
				"label": args.label,
				"apiAddr": api_addr.to_string(),
				"listenAddr": args.listen_addr.to_string(),
				"targetAddr": args.target_addr.to_string(),
				"observeAddr": args.observe_addr.to_string(),
				"workspace": temp_root.display().to_string(),
				"durationMs": started.elapsed().as_millis() as u64,
				"coreFrames": core_frames,
				"corePackets": core_packets,
				"observedMessages": observed.messages,
				"observedBones": observed.bones,
				"observedBlendshapes": observed.blendshapes,
				"hasHead": observed.has_head,
				"hasBlendshape": observed.has_blendshape,
				"startedHealth": started_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
				"stoppedHealth": stopped_status.pointer("/status/health").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
			}))?
		);
		Ok(())
	})();
	stop_core_smoke_process(&mut child);
	if !args.core.keep_temp {
		let _ = fs::remove_dir_all(&temp_root);
	}
	result
}

fn parse_core_smoke_args(raw_args: Vec<OsString>) -> Result<CoreSmokeArgs> {
	let mut core_exe = None;
	let mut timeout = Duration::from_secs(30);
	let mut keep_temp = false;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--core-exe" => core_exe = Some(PathBuf::from(next_value(&mut iter, "--core-exe")?)),
			"--timeout-ms" => {
				timeout = Duration::from_millis(
					next_value(&mut iter, "--timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --timeout-ms")?,
				);
			}
			"--keep-temp" => keep_temp = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask core <smoke|vmc-smoke|vmc-mirror-smoke|external-vmc-smoke|external-vmc-stability|external-vmc-compare|external-ifacialmocap-smoke|lifecycle-smoke> [--core-exe target/release/un-motion-core.exe] [--timeout-ms 30000] [--keep-temp]"
				);
				std::process::exit(0);
			}
			other => bail!("unexpected core smoke argument: {other}"),
		}
	}
	if timeout.is_zero() {
		bail!("--timeout-ms must be greater than 0");
	}
	Ok(CoreSmokeArgs {
		core_exe,
		timeout,
		keep_temp,
	})
}

fn parse_core_external_vmc_smoke_args(raw_args: Vec<OsString>) -> Result<CoreExternalVmcSmokeArgs> {
	let mut core_exe = None;
	let mut timeout = Duration::from_secs(30);
	let mut keep_temp = false;
	let mut listen_addr: Option<SocketAddr> = None;
	let mut target_addr: SocketAddr = "127.0.0.1:39551".parse()?;
	let mut observe_addr: SocketAddr = "127.0.0.1:39571".parse()?;
	let mut label = "external-vmc-smoke".to_string();
	let mut mirror_correction_enabled = false;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--core-exe" => core_exe = Some(PathBuf::from(next_value(&mut iter, "--core-exe")?)),
			"--timeout-ms" => {
				timeout = Duration::from_millis(
					next_value(&mut iter, "--timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --timeout-ms")?,
				);
			}
			"--keep-temp" => keep_temp = true,
			"--listen" => listen_addr = Some(parse_socket_addr_arg(next_value(&mut iter, "--listen")?, "--listen")?),
			"--target" => target_addr = parse_socket_addr_arg(next_value(&mut iter, "--target")?, "--target")?,
			"--observe" => observe_addr = parse_socket_addr_arg(next_value(&mut iter, "--observe")?, "--observe")?,
			"--label" => label = next_value(&mut iter, "--label")?.to_string_lossy().to_string(),
			"--mirror" => mirror_correction_enabled = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask core external-vmc-smoke --listen 127.0.0.1:39550 [--target 127.0.0.1:39551] [--observe 127.0.0.1:39571] [--mirror] [--label warudo-to-vseeface] [--timeout-ms 30000] [--keep-temp]"
				);
				std::process::exit(0);
			}
			other => bail!("unexpected external VMC smoke argument: {other}"),
		}
	}
	if timeout.is_zero() {
		bail!("--timeout-ms must be greater than 0");
	}
	let listen_addr = listen_addr.context("--listen is required, for example --listen 127.0.0.1:39550")?;
	if label.trim().is_empty() {
		bail!("--label must not be empty");
	}
	Ok(CoreExternalVmcSmokeArgs {
		core: CoreSmokeArgs {
			core_exe,
			timeout,
			keep_temp,
		},
		listen_addr,
		target_addr,
		observe_addr,
		label,
		mirror_correction_enabled,
	})
}

fn parse_core_external_vmc_stability_args(raw_args: Vec<OsString>) -> Result<CoreExternalVmcStabilityArgs> {
	let mut core_exe = None;
	let mut timeout = Duration::from_secs(30);
	let mut keep_temp = false;
	let mut listen_addr: Option<SocketAddr> = None;
	let mut output_bind_addr: SocketAddr = "127.0.0.1:0".parse()?;
	let mut label = "external-vmc-stability".to_string();
	let mut duration = Duration::from_secs(5);
	let mut output = None;
	let mut min_samples = 2_usize;
	let mut mirror_correction_enabled = false;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--core-exe" => core_exe = Some(PathBuf::from(next_value(&mut iter, "--core-exe")?)),
			"--timeout-ms" => {
				timeout = Duration::from_millis(
					next_value(&mut iter, "--timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --timeout-ms")?,
				);
			}
			"--keep-temp" => keep_temp = true,
			"--listen" => listen_addr = Some(parse_socket_addr_arg(next_value(&mut iter, "--listen")?, "--listen")?),
			"--output-bind" => output_bind_addr = parse_socket_addr_arg(next_value(&mut iter, "--output-bind")?, "--output-bind")?,
			"--label" => label = next_value(&mut iter, "--label")?.to_string_lossy().to_string(),
			"--duration-ms" => {
				duration = Duration::from_millis(
					next_value(&mut iter, "--duration-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --duration-ms")?,
				);
			}
			"--output" | "-o" => output = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
			"--min-samples" => {
				min_samples = next_value(&mut iter, "--min-samples")?
					.to_string_lossy()
					.parse::<usize>()
					.context("invalid --min-samples")?;
			}
			"--mirror" => mirror_correction_enabled = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask core external-vmc-stability --listen 127.0.0.1:39560 [--output-bind 127.0.0.1:0] [--duration-ms 5000] [--label unmotion-wmc] [--output report.json] [--min-samples 2] [--mirror] [--timeout-ms 30000] [--keep-temp]"
				);
				std::process::exit(0);
			}
			other => bail!("unexpected external VMC stability argument: {other}"),
		}
	}
	if timeout.is_zero() {
		bail!("--timeout-ms must be greater than 0");
	}
	if duration.is_zero() {
		bail!("--duration-ms must be greater than 0");
	}
	if min_samples == 0 {
		bail!("--min-samples must be greater than 0");
	}
	let listen_addr = listen_addr.context("--listen is required, for example --listen 127.0.0.1:39560")?;
	if label.trim().is_empty() {
		bail!("--label must not be empty");
	}
	Ok(CoreExternalVmcStabilityArgs {
		core: CoreSmokeArgs {
			core_exe,
			timeout,
			keep_temp,
		},
		listen_addr,
		output_bind_addr,
		label,
		duration,
		output,
		min_samples,
		mirror_correction_enabled,
	})
}

fn parse_core_external_vmc_compare_args(raw_args: Vec<OsString>) -> Result<CoreExternalVmcCompareArgs> {
	let mut core_exe = None;
	let mut timeout = Duration::from_secs(30);
	let mut keep_temp = false;
	let mut listen_addr: Option<SocketAddr> = None;
	let mut output_bind_addr: SocketAddr = "127.0.0.1:0".parse()?;
	let mut label = "external-vmc-compare".to_string();
	let mut duration = Duration::from_secs(5);
	let mut output_dir = PathBuf::from("target/vmc-captures/runs/stability");
	let mut min_samples = 2_usize;
	let mut mirror_correction_enabled = false;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--core-exe" => core_exe = Some(PathBuf::from(next_value(&mut iter, "--core-exe")?)),
			"--timeout-ms" => {
				timeout = Duration::from_millis(
					next_value(&mut iter, "--timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --timeout-ms")?,
				);
			}
			"--keep-temp" => keep_temp = true,
			"--listen" => listen_addr = Some(parse_socket_addr_arg(next_value(&mut iter, "--listen")?, "--listen")?),
			"--output-bind" => output_bind_addr = parse_socket_addr_arg(next_value(&mut iter, "--output-bind")?, "--output-bind")?,
			"--label" => label = next_value(&mut iter, "--label")?.to_string_lossy().to_string(),
			"--duration-ms" => {
				duration = Duration::from_millis(
					next_value(&mut iter, "--duration-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --duration-ms")?,
				);
			}
			"--output-dir" => output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?),
			"--min-samples" => {
				min_samples = next_value(&mut iter, "--min-samples")?
					.to_string_lossy()
					.parse::<usize>()
					.context("invalid --min-samples")?;
			}
			"--mirror" => mirror_correction_enabled = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask core external-vmc-compare --listen 127.0.0.1:39560 [--output-bind 127.0.0.1:0] [--duration-ms 5000] [--label wmc] [--output-dir target/vmc-captures/runs/stability] [--min-samples 2] [--mirror] [--timeout-ms 30000] [--keep-temp]"
				);
				std::process::exit(0);
			}
			other => bail!("unexpected external VMC compare argument: {other}"),
		}
	}
	if timeout.is_zero() {
		bail!("--timeout-ms must be greater than 0");
	}
	if duration.is_zero() {
		bail!("--duration-ms must be greater than 0");
	}
	if min_samples == 0 {
		bail!("--min-samples must be greater than 0");
	}
	let listen_addr = listen_addr.context("--listen is required, for example --listen 127.0.0.1:39560")?;
	if label.trim().is_empty() {
		bail!("--label must not be empty");
	}
	Ok(CoreExternalVmcCompareArgs {
		core: CoreSmokeArgs {
			core_exe,
			timeout,
			keep_temp,
		},
		listen_addr,
		output_bind_addr,
		label,
		duration,
		output_dir,
		min_samples,
		mirror_correction_enabled,
	})
}

fn parse_core_external_ifacialmocap_smoke_args(raw_args: Vec<OsString>) -> Result<CoreExternalIfacialMocapSmokeArgs> {
	let mut core_exe = None;
	let mut timeout = Duration::from_secs(30);
	let mut keep_temp = false;
	let mut listen_addr: Option<SocketAddr> = None;
	let mut target_addr: SocketAddr = "127.0.0.1:39551".parse()?;
	let mut observe_addr: SocketAddr = "127.0.0.1:39571".parse()?;
	let mut label = "external-ifacialmocap-smoke".to_string();
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--core-exe" => core_exe = Some(PathBuf::from(next_value(&mut iter, "--core-exe")?)),
			"--timeout-ms" => {
				timeout = Duration::from_millis(
					next_value(&mut iter, "--timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --timeout-ms")?,
				);
			}
			"--keep-temp" => keep_temp = true,
			"--listen" => listen_addr = Some(parse_socket_addr_arg(next_value(&mut iter, "--listen")?, "--listen")?),
			"--target" => target_addr = parse_socket_addr_arg(next_value(&mut iter, "--target")?, "--target")?,
			"--observe" => observe_addr = parse_socket_addr_arg(next_value(&mut iter, "--observe")?, "--observe")?,
			"--label" => label = next_value(&mut iter, "--label")?.to_string_lossy().to_string(),
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask core external-ifacialmocap-smoke --listen 192.168.13.13:49983 [--target 127.0.0.1:39551] [--observe 127.0.0.1:39571] [--label ifacialmocap-to-vseeface] [--timeout-ms 30000] [--keep-temp]"
				);
				std::process::exit(0);
			}
			other => bail!("unexpected external iFacialMocap smoke argument: {other}"),
		}
	}
	if timeout.is_zero() {
		bail!("--timeout-ms must be greater than 0");
	}
	let listen_addr = listen_addr.context("--listen is required, for example --listen 192.168.13.13:49983")?;
	if label.trim().is_empty() {
		bail!("--label must not be empty");
	}
	Ok(CoreExternalIfacialMocapSmokeArgs {
		core: CoreSmokeArgs {
			core_exe,
			timeout,
			keep_temp,
		},
		listen_addr,
		target_addr,
		observe_addr,
		label,
	})
}

fn parse_socket_addr_arg(value: OsString, name: &str) -> Result<SocketAddr> {
	value
		.to_string_lossy()
		.parse()
		.with_context(|| format!("invalid {name} socket address"))
}

fn spawn_core_smoke_process(repo: &Path, args: &CoreSmokeArgs, temp_root: &Path, addr: SocketAddr) -> Result<Child> {
	let mut command = if let Some(core_exe) = &args.core_exe {
		let mut command = Command::new(absolutize(repo, core_exe));
		command.arg("--bind").arg(addr.to_string());
		command
	} else {
		let mut command = Command::new(resolve_tool("cargo"));
		command
			.arg("run")
			.arg("--quiet")
			.arg("--manifest-path")
			.arg(repo.join("Cargo.toml"))
			.arg("-p")
			.arg("un-motion-core")
			.arg("--bin")
			.arg("un-motion-core")
			.arg("--")
			.arg("--bind")
			.arg(addr.to_string());
		command
	};
	command
		.current_dir(temp_root)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	command.spawn().context("failed to start core smoke process")
}

fn stop_core_smoke_process(child: &mut Child) {
	if matches!(child.try_wait(), Ok(Some(_))) {
		return;
	}
	let _ = child.kill();
	let _ = child.wait();
}

fn free_tcp_addr() -> Result<SocketAddr> {
	let listener = TcpListener::bind("127.0.0.1:0").context("failed to reserve free TCP port")?;
	Ok(listener.local_addr()?)
}

fn free_udp_addr() -> Result<SocketAddr> {
	let socket = UdpSocket::bind("127.0.0.1:0").context("failed to reserve free UDP port")?;
	Ok(socket.local_addr()?)
}

fn wait_for_core_health(addr: SocketAddr, timeout: Duration) -> Result<()> {
	let deadline = Instant::now() + timeout;
	let mut last_error = None;
	while Instant::now() < deadline {
		match core_http_request(addr, "GET", "/healthz", None, Duration::from_millis(500)) {
			Ok(response) if response.status == 200 => return Ok(()),
			Ok(response) => last_error = Some(format!("HTTP {}", response.status)),
			Err(error) => last_error = Some(error.to_string()),
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	bail!(
		"core smoke API did not become healthy at http://{addr}: {}",
		last_error.unwrap_or_else(|| "timeout".to_string())
	)
}

fn wait_for_core_shutdown(addr: SocketAddr, timeout: Duration) -> Result<()> {
	let deadline = Instant::now() + timeout;
	while Instant::now() < deadline {
		match core_http_request(addr, "GET", "/healthz", None, Duration::from_millis(200)) {
			Ok(response) if response.status == 200 => {
				std::thread::sleep(Duration::from_millis(50));
			}
			_ => return Ok(()),
		}
	}
	bail!("core smoke API was still healthy after shutdown at http://{addr}")
}

fn wait_for_external_vmc_core_route(addr: SocketAddr, timeout: Duration) -> Result<serde_json::Value> {
	let deadline = Instant::now() + timeout;
	let mut last_snapshot = None;
	while Instant::now() < deadline {
		let snapshot = core_http_json(addr, "GET", "/api/runtime/snapshot", None)?;
		let frames = snapshot
			.pointer("/snapshot/status/frameCount")
			.and_then(serde_json::Value::as_u64)
			.unwrap_or(0);
		let packets = snapshot
			.pointer("/snapshot/status/packetCount")
			.and_then(serde_json::Value::as_u64)
			.unwrap_or(0);
		let live_stream = snapshot
			.pointer("/snapshot/runtime/streams")
			.and_then(serde_json::Value::as_array)
			.is_some_and(|streams| {
				streams
					.iter()
					.any(|stream| stream.pointer("/health").and_then(serde_json::Value::as_str) == Some("Live"))
			});
		if frames > 0 && packets > 0 && live_stream {
			return Ok(snapshot);
		}
		last_snapshot = Some(snapshot);
		std::thread::sleep(Duration::from_millis(50));
	}
	bail!(
		"external VMC smoke did not route through core before timeout: {}",
		last_snapshot
			.map(|snapshot| snapshot.to_string())
			.unwrap_or_else(|| "no snapshot".to_string())
	)
}

fn core_http_json(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> Result<serde_json::Value> {
	let response = core_http_request(addr, method, path, body, Duration::from_secs(3))?;
	if !(200..300).contains(&response.status) {
		bail!("{method} {path} returned HTTP {}: {}", response.status, response.body);
	}
	serde_json::from_str(&response.body).with_context(|| format!("{method} {path} returned invalid JSON"))
}

fn core_http_request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>, timeout: Duration) -> Result<CoreHttpResponse> {
	let mut stream = TcpStream::connect_timeout(&addr, timeout).with_context(|| format!("connect http://{addr}{path}"))?;
	stream.set_read_timeout(Some(timeout)).ok();
	stream.set_write_timeout(Some(timeout)).ok();
	let body = body.unwrap_or("");
	let request = format!(
		"{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
		body.len()
	);
	stream.write_all(request.as_bytes()).context("HTTP request write failed")?;
	let mut response = String::new();
	stream.read_to_string(&mut response).context("HTTP response read failed")?;
	let (head, body) = response.split_once("\r\n\r\n").context("malformed HTTP response")?;
	let status = head
		.lines()
		.next()
		.and_then(|line| line.split_whitespace().nth(1))
		.context("missing HTTP status")?
		.parse::<u16>()
		.context("invalid HTTP status")?;
	Ok(CoreHttpResponse {
		status,
		body: body.to_string(),
	})
}

fn smoke_profile_document_json() -> serde_json::Value {
	serde_json::json!({
		"profiles": [{
			"id": "smoke",
			"name": "Smoke",
			"note": "xtask core smoke",
			"defaultSourceEnabled": false,
			"defaultSourceLabel": "UNMotion Default",
			"runtimeSelection": {
				"fps": 30,
				"vmcEnabled": false
			},
			"pipelineComponents": null
		}],
		"profileSources": [],
		"selectedProfileId": "smoke",
		"nextProfileIndex": 2,
		"nextSourceIndex": 2
	})
}

fn smoke_vmc_profile_document_json(vmc_listen_addr: SocketAddr, vmc_output_addr: SocketAddr) -> serde_json::Value {
	smoke_vmc_profile_document_json_with_options(
		vmc_listen_addr,
		vmc_output_addr,
		"smoke-vmc",
		"Smoke VMC",
		"xtask core VMC smoke",
		false,
	)
}

fn smoke_vmc_mirror_profile_document_json(vmc_listen_addr: SocketAddr, vmc_output_addr: SocketAddr) -> serde_json::Value {
	smoke_vmc_profile_document_json_with_options(
		vmc_listen_addr,
		vmc_output_addr,
		"smoke-vmc-mirror",
		"Smoke VMC Mirror",
		"xtask core VMC mirror smoke",
		true,
	)
}

fn smoke_vmc_profile_document_json_with_options(
	vmc_listen_addr: SocketAddr,
	vmc_output_addr: SocketAddr,
	profile_id: &str,
	profile_name: &str,
	note: &str,
	_mirror_correction_enabled: bool,
) -> serde_json::Value {
	serde_json::json!({
		"profiles": [{
			"id": profile_id,
			"name": profile_name,
			"note": note,
			"defaultSourceEnabled": false,
			"defaultSourceLabel": "UNMotion Default",
			"runtimeSelection": {
				"fps": 90,
				"engine": "vmc",
				"vmcReceiveListenAddr": vmc_listen_addr.to_string(),
				"vmcEnabled": true,
				"vmcTargetAddr": vmc_output_addr.to_string()
			},
			"pipelineComponents": null
		}],
		"profileSources": [],
		"selectedProfileId": profile_id,
		"nextProfileIndex": 2,
		"nextSourceIndex": 2
	})
}

fn smoke_ifacialmocap_profile_document_json(
	ifacialmocap_listen_addr: SocketAddr,
	vmc_output_addr: SocketAddr,
	profile_name: &str,
) -> serde_json::Value {
	serde_json::json!({
		"profiles": [{
			"id": "external-ifacialmocap-smoke",
			"name": profile_name,
			"note": "xtask core external iFacialMocap smoke",
			"defaultSourceEnabled": false,
			"defaultSourceLabel": "UNMotion Default",
			"runtimeSelection": {
				"fps": 90,
				"engine": "ifacialmocap",
				"ifacialmocapReceiveListenAddr": ifacialmocap_listen_addr.to_string(),
				"vmcEnabled": true,
				"vmcTargetAddr": vmc_output_addr.to_string()
			},
			"pipelineComponents": null
		}],
		"profileSources": [],
		"selectedProfileId": "external-ifacialmocap-smoke",
		"nextProfileIndex": 2,
		"nextSourceIndex": 2
	})
}

#[derive(Debug)]
struct VmcSmokeOutput {
	messages: usize,
	has_head: bool,
	has_eye_blink_left: bool,
	has_jaw_open: bool,
}

#[derive(Debug)]
struct VmcMirrorSmokeOutput {
	messages: usize,
	root_x: f32,
	head_x: f32,
	has_right_hand: bool,
	has_eye_blink_right: bool,
	has_jaw_open: bool,
}

#[derive(Debug)]
struct ExternalVmcObservedOutput {
	messages: usize,
	bones: usize,
	blendshapes: usize,
	has_head: bool,
	has_blendshape: bool,
}

fn wait_for_vmc_smoke_output(socket: &UdpSocket, timeout: Duration) -> Result<VmcSmokeOutput> {
	let deadline = Instant::now() + timeout;
	let mut buf = [0_u8; 65535];
	let mut messages = 0_usize;
	let mut has_head = false;
	let mut has_eye_blink_left = false;
	let mut has_jaw_open = false;
	while Instant::now() < deadline {
		let (len, _) = match socket.recv_from(&mut buf) {
			Ok(value) => value,
			Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
			Err(error) => return Err(error).context("VMC smoke output recv failed"),
		};
		let Ok((_, packet)) = decoder::decode_udp(&buf[..len]) else {
			continue;
		};
		for message in flatten_osc_messages(packet) {
			messages = messages.saturating_add(1);
			if message.addr == "/VMC/Ext/Bone/Pos" && message.args.first() == Some(&OscType::String("Head".to_string())) {
				has_head = true;
			}
			if message.addr == "/VMC/Ext/Blend/Val" && message.args.first() == Some(&OscType::String("eyeBlinkLeft".to_string())) {
				has_eye_blink_left = true;
			}
			if message.addr == "/VMC/Ext/Blend/Val" && message.args.first() == Some(&OscType::String("jawOpen".to_string())) {
				has_jaw_open = true;
			}
		}
		if has_head && has_eye_blink_left && has_jaw_open {
			return Ok(VmcSmokeOutput {
				messages,
				has_head,
				has_eye_blink_left,
				has_jaw_open,
			});
		}
	}
	bail!(
		"timed out waiting for VMC smoke output; messages={messages} head={has_head} eyeBlinkLeft={has_eye_blink_left} jawOpen={has_jaw_open}"
	)
}

fn wait_for_external_vmc_observed_output(socket: &UdpSocket, timeout: Duration) -> Result<ExternalVmcObservedOutput> {
	let deadline = Instant::now() + timeout;
	let mut buf = [0_u8; 65535];
	let mut messages = 0_usize;
	let mut bones = 0_usize;
	let mut blendshapes = 0_usize;
	let mut has_head = false;
	let mut has_blendshape = false;
	while Instant::now() < deadline {
		let (len, _) = match socket.recv_from(&mut buf) {
			Ok(value) => value,
			Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
			Err(error) => return Err(error).context("external VMC observe recv failed"),
		};
		let Ok((_, packet)) = decoder::decode_udp(&buf[..len]) else {
			continue;
		};
		for message in flatten_osc_messages(packet) {
			messages = messages.saturating_add(1);
			if message.addr == "/VMC/Ext/Bone/Pos" {
				bones = bones.saturating_add(1);
				if message.args.first() == Some(&OscType::String("Head".to_string())) {
					has_head = true;
				}
			}
			if message.addr == "/VMC/Ext/Blend/Val" {
				blendshapes = blendshapes.saturating_add(1);
				has_blendshape = true;
			}
		}
		if has_head && has_blendshape {
			return Ok(ExternalVmcObservedOutput {
				messages,
				bones,
				blendshapes,
				has_head,
				has_blendshape,
			});
		}
	}
	bail!(
		"timed out waiting for external VMC observed output; messages={messages} bones={bones} blendshapes={blendshapes} head={has_head} blendshape={has_blendshape}"
	)
}

fn drain_udp_socket(socket: &UdpSocket, label: &str) -> Result<()> {
	socket
		.set_nonblocking(true)
		.with_context(|| format!("failed to set {label} nonblocking"))?;
	let mut buf = [0_u8; 65535];
	loop {
		match socket.recv_from(&mut buf) {
			Ok(_) => {}
			Err(error) if error.kind() == ErrorKind::WouldBlock => break,
			Err(error) if error.kind() == ErrorKind::Interrupted => {}
			Err(error) => return Err(error).with_context(|| format!("failed to drain {label}")),
		}
	}
	socket
		.set_nonblocking(false)
		.with_context(|| format!("failed to restore {label} blocking mode"))?;
	Ok(())
}

fn wait_for_vmc_compare_output_start(
	input_socket: &UdpSocket,
	forward_socket: &UdpSocket,
	core_listen_addr: SocketAddr,
	output_socket: &UdpSocket,
	timeout: Duration,
) -> Result<()> {
	let deadline = Instant::now() + timeout;
	let mut buf = [0_u8; 65535];
	let mut forwarded = 0_u64;
	while Instant::now() < deadline {
		let mut activity = false;
		loop {
			match input_socket.recv_from(&mut buf) {
				Ok((len, _)) => {
					activity = true;
					forwarded = forwarded.saturating_add(1);
					forward_socket
						.send_to(&buf[..len], core_listen_addr)
						.with_context(|| format!("failed to warm up VMC compare core input {core_listen_addr}"))?;
				}
				Err(error) if error.kind() == ErrorKind::WouldBlock => break,
				Err(error) => return Err(error).context("VMC compare warmup input recv failed"),
			}
		}
		match output_socket.recv_from(&mut buf) {
			Ok(_) => return Ok(()),
			Err(error) if error.kind() == ErrorKind::WouldBlock => {}
			Err(error) => return Err(error).context("VMC compare warmup output recv failed"),
		}
		if !activity {
			std::thread::sleep(Duration::from_millis(1));
		}
	}
	bail!("external VMC compare produced no core output before timeout; forwarded datagrams={forwarded}")
}

fn wait_for_vmc_mirror_smoke_output(socket: &UdpSocket, timeout: Duration) -> Result<VmcMirrorSmokeOutput> {
	let deadline = Instant::now() + timeout;
	let mut buf = [0_u8; 65535];
	let mut messages = 0_usize;
	let mut root_x = None;
	let mut head_x = None;
	let mut has_left_hand = false;
	let mut has_right_hand = false;
	let mut has_eye_blink_left = false;
	let mut has_eye_blink_right = false;
	let mut has_jaw_open = false;
	while Instant::now() < deadline {
		let (len, _) = match socket.recv_from(&mut buf) {
			Ok(value) => value,
			Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
			Err(error) => return Err(error).context("VMC mirror smoke output recv failed"),
		};
		let Ok((_, packet)) = decoder::decode_udp(&buf[..len]) else {
			continue;
		};
		for message in flatten_osc_messages(packet) {
			messages = messages.saturating_add(1);
			if message.addr == "/VMC/Ext/Root/Pos" {
				root_x = message.args.get(1).and_then(osc_float_arg);
			}
			if message.addr == "/VMC/Ext/Bone/Pos" && message.args.first() == Some(&OscType::String("Head".to_string())) {
				head_x = message.args.get(1).and_then(osc_float_arg);
			}
			if message.addr == "/VMC/Ext/Bone/Pos" && message.args.first() == Some(&OscType::String("LeftHand".to_string())) {
				has_left_hand = true;
			}
			if message.addr == "/VMC/Ext/Bone/Pos" && message.args.first() == Some(&OscType::String("RightHand".to_string())) {
				has_right_hand = true;
			}
			if message.addr == "/VMC/Ext/Blend/Val" && message.args.first() == Some(&OscType::String("eyeBlinkLeft".to_string())) {
				has_eye_blink_left = true;
			}
			if message.addr == "/VMC/Ext/Blend/Val" && message.args.first() == Some(&OscType::String("eyeBlinkRight".to_string())) {
				has_eye_blink_right = true;
			}
			if message.addr == "/VMC/Ext/Blend/Val" && message.args.first() == Some(&OscType::String("jawOpen".to_string())) {
				has_jaw_open = true;
			}
		}
		if root_x.is_some()
			&& head_x.is_some()
			&& has_right_hand
			&& has_eye_blink_right
			&& has_jaw_open
			&& !has_left_hand
			&& !has_eye_blink_left
		{
			let root_x = root_x.expect("checked root x");
			let head_x = head_x.expect("checked head x");
			if (root_x + 0.25).abs() > 0.001 || (head_x + 0.8).abs() > 0.001 {
				bail!("VMC mirror smoke output had wrong mirrored x values: root_x={root_x} head_x={head_x}");
			}
			return Ok(VmcMirrorSmokeOutput {
				messages,
				root_x,
				head_x,
				has_right_hand,
				has_eye_blink_right,
				has_jaw_open,
			});
		}
	}
	bail!(
		"timed out waiting for VMC mirror smoke output; messages={messages} root_x={root_x:?} head_x={head_x:?} rightHand={has_right_hand} leftHand={has_left_hand} eyeBlinkRight={has_eye_blink_right} eyeBlinkLeft={has_eye_blink_left} jawOpen={has_jaw_open}"
	)
}

fn osc_float_arg(arg: &OscType) -> Option<f32> {
	match arg {
		OscType::Float(value) => Some(*value),
		OscType::Double(value) => Some(*value as f32),
		_ => None,
	}
}

fn smoke_vmc_datagram() -> Result<Vec<u8>> {
	encoder::encode(&OscPacket::Bundle(OscBundle {
		timetag: OscTime { seconds: 0, fractional: 1 },
		content: vec![
			OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Bone/Pos".to_string(),
				args: vec![
					OscType::String("Head".to_string()),
					OscType::Float(0.0),
					OscType::Float(0.5),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(0.0),
					OscType::Float(1.0),
				],
			}),
			OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Val".to_string(),
				args: vec![OscType::String("eyeBlinkLeft".to_string()), OscType::Float(0.75)],
			}),
			OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Val".to_string(),
				args: vec![OscType::String("jawOpen".to_string()), OscType::Float(0.5)],
			}),
			OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Apply".to_string(),
				args: Vec::new(),
			}),
		],
	}))
	.context("failed to encode VMC smoke datagram")
}

fn smoke_vmc_mirror_datagram() -> Result<Vec<u8>> {
	encoder::encode(&OscPacket::Bundle(OscBundle {
		timetag: OscTime { seconds: 0, fractional: 1 },
		content: vec![
			vmc_transform_packet("/VMC/Ext/Root/Pos", "root", 0.25, 1.2, -0.4, 0.1, 0.2, -0.3, 0.9),
			vmc_transform_packet("/VMC/Ext/Bone/Pos", "Head", 0.8, 0.1, 0.2, 0.1, 0.2, -0.3, 0.9),
			vmc_transform_packet("/VMC/Ext/Bone/Pos", "LeftHand", 0.4, 0.2, 0.3, 0.0, 0.0, 0.0, 1.0),
			OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Val".to_string(),
				args: vec![OscType::String("eyeBlinkLeft".to_string()), OscType::Float(0.75)],
			}),
			OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Val".to_string(),
				args: vec![OscType::String("jawOpen".to_string()), OscType::Float(0.5)],
			}),
			OscPacket::Message(OscMessage {
				addr: "/VMC/Ext/Blend/Apply".to_string(),
				args: Vec::new(),
			}),
		],
	}))
	.context("failed to encode VMC mirror smoke datagram")
}

fn vmc_transform_packet(addr: &str, name: &str, px: f32, py: f32, pz: f32, rx: f32, ry: f32, rz: f32, rw: f32) -> OscPacket {
	OscPacket::Message(OscMessage {
		addr: addr.to_string(),
		args: vec![
			OscType::String(name.to_string()),
			OscType::Float(px),
			OscType::Float(py),
			OscType::Float(pz),
			OscType::Float(rx),
			OscType::Float(ry),
			OscType::Float(rz),
			OscType::Float(rw),
		],
	})
}

fn make_release_package(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_make_release_package_args(raw_args)?;
	let version = match args.version {
		Some(version) => version,
		None => default_release_version(repo)?,
	};
	let file_version = package_file_version(&version)?;
	let package_name = format!("un-motion-{file_version}");
	let output_dir = absolutize(repo, &args.output_dir);
	let output_zip = output_dir.join(format!("{package_name}.zip"));
	let staging_root = repo.join("target/release/package");
	let staging_dir = staging_root.join(&package_name);

	if !args.skip_build {
		run(repo.join("apps/un-motion-supervisor"), "npm", ["run", "build"])?;
		run(
			repo,
			"cargo",
			["build", "--release", "-p", "un-motion-core", "--bin", "un-motion-core"],
		)?;
		run(
			repo,
			"cargo",
			["build", "--release", "-p", "un-motion-capturer", "--bin", "un-motion-capturer"],
		)?;
		run(
			repo,
			"cargo",
			["build", "--release", "-p", "un-motion-supervisor", "--bin", "un-motion-supervisor"],
		)?;
	}

	fs::create_dir_all(&output_dir).with_context(|| format!("failed to create {}", output_dir.display()))?;
	if output_zip.exists() {
		fs::remove_file(&output_zip).with_context(|| format!("failed to replace {}", output_zip.display()))?;
	}
	if staging_dir.exists() {
		fs::remove_dir_all(&staging_dir).with_context(|| format!("failed to clean {}", staging_dir.display()))?;
	}
	fs::create_dir_all(&staging_dir).with_context(|| format!("failed to create {}", staging_dir.display()))?;

	let core_exe = repo.join(format!("target/release/un-motion-core{}", env::consts::EXE_SUFFIX));
	if !core_exe.exists() {
		bail!("release core executable was not produced: {}", core_exe.display());
	}
	let supervisor_exe = repo.join(format!("target/release/un-motion-supervisor{}", env::consts::EXE_SUFFIX));
	if !supervisor_exe.exists() {
		bail!("release supervisor executable was not produced: {}", supervisor_exe.display());
	}
	let capturer_exe = repo.join(format!("target/release/un-motion-capturer{}", env::consts::EXE_SUFFIX));
	if !capturer_exe.exists() {
		bail!("release capturer executable was not produced: {}", capturer_exe.display());
	}
	let native_artifacts = release_native_artifacts(repo)?;
	let model_artifacts = release_model_artifacts(repo)?;

	copy_release_file(&core_exe, &staging_dir.join(format!("un-motion-core{}", env::consts::EXE_SUFFIX)))?;
	copy_release_file(
		&supervisor_exe,
		&staging_dir.join(format!("un-motion-supervisor{}", env::consts::EXE_SUFFIX)),
	)?;
	copy_release_file(
		&capturer_exe,
		&staging_dir.join(format!("un-motion-capturer{}", env::consts::EXE_SUFFIX)),
	)?;
	for artifact in &native_artifacts {
		copy_release_file(&artifact.source, &staging_dir.join(&artifact.package_path))?;
	}
	for artifact in &model_artifacts {
		copy_release_file(&artifact.source, &staging_dir.join(&artifact.package_path))?;
	}
	copy_release_file(&repo.join("README.md"), &staging_dir.join("README.md"))?;
	copy_release_file(&repo.join("LICENSE"), &staging_dir.join("LICENSE"))?;
	copy_release_file(&repo.join("THIRD_PARTY_NOTICES.md"), &staging_dir.join("THIRD_PARTY_NOTICES.md"))?;
	copy_release_dir(&repo.join("LICENSES"), &staging_dir.join("LICENSES"))?;
	write_dependency_license_report(repo, &staging_dir.join("THIRD_PARTY_DEPENDENCIES.md"))?;
	copy_release_file(
		&repo.join("configs/desktop.example.toml"),
		&staging_dir.join("configs/desktop.example.toml"),
	)?;
	copy_release_file(
		&repo.join("configs/pipeline-policy.example.toml"),
		&staging_dir.join("configs/pipeline-policy.example.toml"),
	)?;
	let supervisor_launcher_name = supervisor_launcher_name();
	fs::write(staging_dir.join(supervisor_launcher_name), supervisor_launcher_script())
		.with_context(|| format!("failed to write {}", staging_dir.join(supervisor_launcher_name).display()))?;
	let manifest = release_package_manifest(&package_name, &version, &native_artifacts, &model_artifacts);
	fs::write(staging_dir.join("PACKAGE.txt"), manifest)
		.with_context(|| format!("failed to write {}", staging_dir.join("PACKAGE.txt").display()))?;

	let file = File::create(&output_zip).with_context(|| format!("failed to create {}", output_zip.display()))?;
	let mut zip = zip::ZipWriter::new(file);
	let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
	add_dir_to_zip(&mut zip, options, &staging_root, &staging_dir)?;
	zip.finish().context("failed to finish release package zip")?;

	if !args.keep_staging {
		fs::remove_dir_all(&staging_dir).with_context(|| format!("failed to remove {}", staging_dir.display()))?;
		if staging_root.exists() && fs::read_dir(&staging_root)?.next().is_none() {
			fs::remove_dir(&staging_root).with_context(|| format!("failed to remove {}", staging_root.display()))?;
		}
	}

	let size = fs::metadata(&output_zip)
		.with_context(|| format!("failed to stat {}", output_zip.display()))?
		.len();
	println!("Release package created:");
	println!("- Path: {}", output_zip.display());
	println!("- Size: {size} bytes");
	println!("PACKAGE_PATH={}", output_zip.display());
	Ok(())
}

fn license_report(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let mut output = PathBuf::from("target/license-report/THIRD_PARTY_DEPENDENCIES.md");
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--output" | "-o" => output = PathBuf::from(next_value(&mut iter, "--output")?),
			"--help" | "-h" | "help" => {
				eprintln!("usage: cargo xtask license-report [--output target/license-report/THIRD_PARTY_DEPENDENCIES.md]");
				return Ok(());
			}
			other => bail!("unexpected license-report argument: {other}"),
		}
	}
	let output = absolutize(repo, &output);
	write_dependency_license_report(repo, &output)?;
	println!("Dependency license report written: {}", output.display());
	Ok(())
}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
struct DependencyLicenseRow {
	ecosystem: String,
	name: String,
	version: String,
	license: String,
	source: String,
}

fn write_dependency_license_report(repo: &Path, output: &Path) -> Result<()> {
	let mut rows = Vec::new();
	rows.extend(rust_dependency_license_rows(repo)?);
	rows.extend(npm_dependency_license_rows(repo)?);
	rows.sort();
	rows.dedup();

	let mut markdown = String::from(
		"# Third-party dependency license report\n\n\
This report is generated from `Cargo.lock` / `cargo metadata` and `apps/un-motion-supervisor/package-lock.json`. It is a release audit aid, not legal advice.\n\n\
## Dependencies\n\n\
| Ecosystem | Package | Version | License | Source |\n\
| --- | --- | --- | --- | --- |\n",
	);
	for row in rows {
		markdown.push_str(&format!(
			"| {} | {} | {} | {} | {} |\n",
			markdown_escape_cell(&row.ecosystem),
			markdown_escape_cell(&row.name),
			markdown_escape_cell(&row.version),
			markdown_escape_cell(&row.license),
			markdown_escape_cell(&row.source)
		));
	}
	if let Some(parent) = output.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	fs::write(output, markdown).with_context(|| format!("failed to write {}", output.display()))
}

fn rust_dependency_license_rows(repo: &Path) -> Result<Vec<DependencyLicenseRow>> {
	let output = Command::new(resolve_tool("cargo"))
		.current_dir(repo)
		.args(["metadata", "--format-version", "1", "--locked"])
		.output()
		.context("failed to run cargo metadata")?;
	if !output.status.success() {
		bail!("cargo metadata failed:\n{}", String::from_utf8_lossy(&output.stderr));
	}
	let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).context("failed to parse cargo metadata output")?;
	let mut rows = Vec::new();
	for package in metadata
		.get("packages")
		.and_then(serde_json::Value::as_array)
		.context("cargo metadata output has no packages array")?
	{
		if package.get("source").and_then(serde_json::Value::as_str).is_none() {
			continue;
		}
		let name = json_string(package, "name");
		let version = json_string(package, "version");
		rows.push(DependencyLicenseRow {
			ecosystem: "Rust".to_string(),
			name,
			version,
			license: json_string(package, "license"),
			source: json_string(package, "source"),
		});
	}
	Ok(rows)
}

fn npm_dependency_license_rows(repo: &Path) -> Result<Vec<DependencyLicenseRow>> {
	let lockfile = repo.join("apps/un-motion-supervisor/package-lock.json");
	let lock: serde_json::Value =
		serde_json::from_str(&fs::read_to_string(&lockfile).with_context(|| format!("failed to read {}", lockfile.display()))?)
			.with_context(|| format!("failed to parse {}", lockfile.display()))?;
	let mut rows = Vec::new();
	let packages = lock
		.get("packages")
		.and_then(serde_json::Value::as_object)
		.context("package-lock.json has no packages object")?;
	for (path, package) in packages {
		let Some(name) = npm_lock_package_name(path) else {
			continue;
		};
		rows.push(DependencyLicenseRow {
			ecosystem: "npm".to_string(),
			name,
			version: json_string(package, "version"),
			license: json_string(package, "license"),
			source: json_string(package, "resolved"),
		});
	}
	Ok(rows)
}

fn npm_lock_package_name(path: &str) -> Option<String> {
	let parts: Vec<&str> = path.split('/').collect();
	let index = parts.iter().rposition(|part| *part == "node_modules")?;
	let name = *parts.get(index + 1)?;
	if name.starts_with('@') {
		Some(format!("{}/{}", name, parts.get(index + 2)?))
	} else {
		Some(name.to_string())
	}
}

fn json_string(value: &serde_json::Value, key: &str) -> String {
	value
		.get(key)
		.and_then(serde_json::Value::as_str)
		.filter(|value| !value.is_empty())
		.unwrap_or("UNKNOWN")
		.to_string()
}

fn markdown_escape_cell(value: &str) -> String {
	value.replace('|', "\\|").replace('\n', " ")
}

#[derive(Debug)]
struct ReleaseArtifact {
	source: PathBuf,
	package_path: String,
}

fn release_native_artifacts(repo: &Path) -> Result<Vec<ReleaseArtifact>> {
	let mut artifacts = Vec::new();
	for name in ["un-motion-mediapipe.dll", "opencv_world3410.dll"] {
		let source = repo.join("native/mediapipe").join(name);
		if !source.exists() {
			bail!("missing native release artifact: {}", source.display());
		}
		artifacts.push(ReleaseArtifact {
			source,
			package_path: name.to_string(),
		});
	}
	Ok(artifacts)
}

fn release_model_artifacts(repo: &Path) -> Result<Vec<ReleaseArtifact>> {
	let mut artifacts = Vec::new();
	for name in [
		"pose_landmarker_lite.task",
		"hand_landmarker.task",
		"face_landmarker.task",
		"holistic_landmarker.task",
	] {
		let source = repo.join("apps/un-motion-supervisor/public/mediapipe/models").join(name);
		if !source.exists() {
			bail!("missing model release artifact: {}", source.display());
		}
		artifacts.push(ReleaseArtifact {
			source,
			package_path: format!("models/{name}"),
		});
	}
	Ok(artifacts)
}

const PENN_ACTION_URL: &str = "https://www.cis.upenn.edu/~kostas/Penn_Action.tar.gz";
const FFMPEG_RELEASE_ESSENTIALS_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";

fn research(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let Some(subcommand) = args.first().map(|arg| arg.to_string_lossy()) else {
		eprintln!("usage: cargo xtask research <penn-action|ffmpeg> <command> [options]");
		bail!("missing research subcommand");
	};
	match subcommand.as_ref() {
		"penn-action" => penn_action(repo, args[1..].to_vec()),
		"ffmpeg" => research_ffmpeg(repo, args[1..].to_vec()),
		"--help" | "-h" | "help" => {
			eprintln!("usage: cargo xtask research <penn-action|ffmpeg> <command> [options]");
			Ok(())
		}
		other => bail!("unknown research subcommand: {other}"),
	}
}

fn penn_action(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let Some(subcommand) = args.first().map(|arg| arg.to_string_lossy()) else {
		eprintln!("usage: cargo xtask research penn-action <prepare|desktop-config|summary> [options]");
		bail!("missing penn-action subcommand");
	};
	match subcommand.as_ref() {
		"prepare" => prepare_penn_action(repo, args[1..].to_vec()),
		"desktop-config" => write_penn_action_desktop_config(repo, args[1..].to_vec()),
		"summary" => summarize_penn_action(repo, args[1..].to_vec()),
		"--help" | "-h" | "help" => {
			eprintln!("usage: cargo xtask research penn-action <prepare|desktop-config|summary> [options]");
			Ok(())
		}
		other => bail!("unknown penn-action subcommand: {other}"),
	}
}

#[derive(Debug)]
struct PennActionPrepareArgs {
	root: PathBuf,
	archive_url: String,
	video_limit: usize,
	video_actions: Vec<String>,
	fps: u32,
	ffmpeg_path: Option<PathBuf>,
	skip_download: bool,
	skip_extract: bool,
	skip_videos: bool,
	force_extract: bool,
}

impl Default for PennActionPrepareArgs {
	fn default() -> Self {
		Self {
			root: PathBuf::from("target/research/penn-action"),
			archive_url: PENN_ACTION_URL.to_string(),
			video_limit: 16,
			video_actions: Vec::new(),
			fps: 30,
			ffmpeg_path: None,
			skip_download: false,
			skip_extract: false,
			skip_videos: false,
			force_extract: false,
		}
	}
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PennActionManifest {
	name: String,
	source_url: String,
	root: String,
	archive: String,
	raw_dir: String,
	frames_dir: String,
	labels_dir: Option<String>,
	videos_dir: String,
	video_fps: u32,
	video_width: u32,
	sequences: Vec<PennActionSequence>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PennActionSequence {
	id: String,
	action: Option<String>,
	pose: Option<String>,
	frame_count: usize,
	first_frame: String,
	last_frame: String,
	label_file: Option<String>,
	video_320w: Option<String>,
}

fn prepare_penn_action(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_penn_action_prepare_args(raw_args)?;
	let root = absolutize(repo, &args.root);
	let archive = root.join("archive/Penn_Action.tar.gz");
	let raw_dir = root.join("raw");
	let videos_dir = root.join("videos/320w");
	let manifest_path = root.join("manifest.json");

	if !args.skip_download {
		ensure_download(repo, "Penn Action", &args.archive_url, &archive)?;
	}
	if !args.skip_extract {
		extract_penn_action_archive(&archive, &raw_dir, args.force_extract)?;
	}

	let frames_dir = find_penn_action_frames_dir(&raw_dir)?;
	let labels_dir = find_penn_action_labels_dir(&raw_dir).ok();
	let mut sequences = collect_penn_action_sequences(&frames_dir, labels_dir.as_deref())?;
	if !args.skip_videos && args.video_limit > 0 {
		create_penn_action_videos(
			repo,
			&frames_dir,
			&videos_dir,
			args.fps,
			args.video_limit,
			&args.video_actions,
			args.ffmpeg_path.as_deref(),
			&mut sequences,
		)?;
	}
	let manifest = PennActionManifest {
		name: "Penn Action".to_string(),
		source_url: args.archive_url,
		root: path_slash(&root),
		archive: path_slash(&archive),
		raw_dir: path_slash(&raw_dir),
		frames_dir: path_slash(&frames_dir),
		labels_dir: labels_dir.as_deref().map(path_slash),
		videos_dir: path_slash(&videos_dir),
		video_fps: args.fps,
		video_width: 320,
		sequences,
	};
	if let Some(parent) = manifest_path.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	let json = serde_json::to_string_pretty(&manifest).context("failed to serialize Penn Action manifest")?;
	fs::write(&manifest_path, format!("{json}\n")).with_context(|| format!("failed to write {}", manifest_path.display()))?;
	eprintln!("wrote {}", manifest_path.display());
	Ok(())
}

fn parse_penn_action_prepare_args(raw_args: Vec<OsString>) -> Result<PennActionPrepareArgs> {
	let mut args = PennActionPrepareArgs::default();
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--root" => args.root = PathBuf::from(next_value(&mut iter, "--root")?),
			"--archive-url" => args.archive_url = next_value(&mut iter, "--archive-url")?.to_string_lossy().into_owned(),
			"--video-limit" => args.video_limit = parse_usize(next_value(&mut iter, "--video-limit")?, "--video-limit")?,
			"--video-actions" => args.video_actions = parse_csv(next_value(&mut iter, "--video-actions")?),
			"--fps" => args.fps = parse_u32(next_value(&mut iter, "--fps")?, "--fps")?.max(1),
			"--ffmpeg" => args.ffmpeg_path = Some(PathBuf::from(next_value(&mut iter, "--ffmpeg")?)),
			"--skip-download" => args.skip_download = true,
			"--skip-extract" => args.skip_extract = true,
			"--skip-videos" => args.skip_videos = true,
			"--force-extract" => args.force_extract = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask research penn-action prepare [--root target/research/penn-action] [--archive-url URL] [--video-limit 16] [--video-actions jumping_jacks,squat] [--fps 30] [--ffmpeg path/to/ffmpeg] [--skip-download] [--skip-extract] [--skip-videos] [--force-extract]"
				);
				std::process::exit(0);
			}
			other => bail!("unknown Penn Action prepare option: {other}"),
		}
	}
	Ok(args)
}

#[derive(Debug)]
struct PennActionDesktopConfigArgs {
	root: PathBuf,
	sequence: String,
	action: Option<String>,
	output: Option<PathBuf>,
	ffmpeg_path: PathBuf,
	width: u32,
	height: u32,
	fps: u32,
	repeat: bool,
	install: bool,
	backup_dir: PathBuf,
}

impl Default for PennActionDesktopConfigArgs {
	fn default() -> Self {
		Self {
			root: PathBuf::from("target/research/penn-action"),
			sequence: "0001".to_string(),
			action: None,
			output: None,
			ffmpeg_path: PathBuf::from("target/tools/ffmpeg/ffmpeg.exe"),
			width: 640,
			height: 480,
			fps: 30,
			repeat: true,
			install: false,
			backup_dir: PathBuf::from("target/research/penn-action/conf-backups"),
		}
	}
}

fn write_penn_action_desktop_config(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_penn_action_desktop_config_args(raw_args)?;
	let root = absolutize(repo, &args.root);
	let manifest_path = root.join("manifest.json");
	let output = args
		.output
		.as_ref()
		.map(|path| absolutize(repo, path))
		.unwrap_or_else(|| root.join("desktop-file-video.toml"));
	let manifest = read_penn_action_manifest(&manifest_path)?;
	let sequence = select_penn_action_sequence_for_config(&manifest, &args)
		.with_context(|| format!("failed to select Penn Action sequence from {}", manifest_path.display()))?;
	let video = sequence
		.video_320w
		.as_ref()
		.map(PathBuf::from)
		.unwrap_or_else(|| root.join("videos/320w").join(format!("{}.mp4", sequence.id)));
	if !video.exists() {
		bail!(
			"Penn Action mp4 is missing for sequence {}: {}. Run `cargo xtask research penn-action prepare --skip-download --skip-extract --video-limit N` first.",
			sequence.id,
			video.display()
		);
	}
	let raw = penn_action_desktop_config_toml(
		&sequence.id,
		&video,
		&absolutize(repo, &args.ffmpeg_path),
		args.width,
		args.height,
		args.fps,
		args.repeat,
	);
	if let Some(parent) = output.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	fs::write(&output, raw).with_context(|| format!("failed to write {}", output.display()))?;
	eprintln!("wrote {}", output.display());
	if args.install {
		install_desktop_config(repo, &output, &args.backup_dir)?;
	}
	Ok(())
}

fn parse_penn_action_desktop_config_args(raw_args: Vec<OsString>) -> Result<PennActionDesktopConfigArgs> {
	let mut args = PennActionDesktopConfigArgs::default();
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--root" => args.root = PathBuf::from(next_value(&mut iter, "--root")?),
			"--sequence" => args.sequence = next_value(&mut iter, "--sequence")?.to_string_lossy().into_owned(),
			"--action" => args.action = Some(next_value(&mut iter, "--action")?.to_string_lossy().into_owned()),
			"--output" => args.output = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
			"--ffmpeg" => args.ffmpeg_path = PathBuf::from(next_value(&mut iter, "--ffmpeg")?),
			"--width" => args.width = parse_u32(next_value(&mut iter, "--width")?, "--width")?.max(1),
			"--height" => args.height = parse_u32(next_value(&mut iter, "--height")?, "--height")?.max(1),
			"--fps" => args.fps = parse_u32(next_value(&mut iter, "--fps")?, "--fps")?.max(1),
			"--once" => args.repeat = false,
			"--repeat" => args.repeat = true,
			"--install" => args.install = true,
			"--backup-dir" => args.backup_dir = PathBuf::from(next_value(&mut iter, "--backup-dir")?),
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask research penn-action desktop-config [--sequence 0001] [--action jumping_jacks] [--output target/research/penn-action/desktop-file-video.toml] [--ffmpeg target/tools/ffmpeg/ffmpeg.exe] [--width 640] [--height 480] [--fps 30] [--once] [--install] [--backup-dir target/research/penn-action/conf-backups]"
				);
				std::process::exit(0);
			}
			other => bail!("unknown Penn Action desktop-config option: {other}"),
		}
	}
	Ok(args)
}

fn select_penn_action_sequence_for_config<'a>(
	manifest: &'a PennActionManifest,
	args: &PennActionDesktopConfigArgs,
) -> Result<&'a PennActionSequence> {
	if let Some(action) = args.action.as_deref() {
		return manifest
			.sequences
			.iter()
			.find(|sequence| sequence.action.as_deref() == Some(action) && sequence.video_320w.is_some())
			.or_else(|| {
				manifest
					.sequences
					.iter()
					.find(|sequence| sequence.action.as_deref() == Some(action))
			})
			.with_context(|| format!("Penn Action action {action} is not in manifest"));
	}
	manifest
		.sequences
		.iter()
		.find(|sequence| sequence.id == args.sequence)
		.with_context(|| format!("Penn Action sequence {} is not in manifest", args.sequence))
}

fn install_desktop_config(repo: &Path, source: &Path, backup_dir: &Path) -> Result<()> {
	let conf = repo.join("conf.toml");
	let backup_dir = absolutize(repo, backup_dir);
	if conf.exists() {
		fs::create_dir_all(&backup_dir).with_context(|| format!("failed to create {}", backup_dir.display()))?;
		let backup = backup_dir.join(format!("conf.{}.toml", now_unix_ms()));
		fs::copy(&conf, &backup).with_context(|| format!("failed to back up {} to {}", conf.display(), backup.display()))?;
		eprintln!("backed up {} to {}", conf.display(), backup.display());
	}
	fs::copy(source, &conf).with_context(|| format!("failed to install {} to {}", source.display(), conf.display()))?;
	eprintln!("installed {}", conf.display());
	Ok(())
}

fn read_penn_action_manifest(path: &Path) -> Result<PennActionManifest> {
	let raw = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
	serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

#[derive(Debug)]
struct PennActionSummaryArgs {
	root: PathBuf,
	action: Option<String>,
}

impl Default for PennActionSummaryArgs {
	fn default() -> Self {
		Self {
			root: PathBuf::from("target/research/penn-action"),
			action: None,
		}
	}
}

#[derive(Debug, Default)]
struct PennActionActionSummary {
	sequences: usize,
	frames: usize,
	videos: Vec<String>,
	poses: BTreeMap<String, usize>,
}

fn summarize_penn_action(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_penn_action_summary_args(raw_args)?;
	let root = absolutize(repo, &args.root);
	let manifest_path = root.join("manifest.json");
	let manifest = read_penn_action_manifest(&manifest_path)?;
	if let Some(action) = args.action.as_deref() {
		print_penn_action_detail(&manifest, action)?;
	} else {
		print_penn_action_summary(&manifest);
	}
	Ok(())
}

fn parse_penn_action_summary_args(raw_args: Vec<OsString>) -> Result<PennActionSummaryArgs> {
	let mut args = PennActionSummaryArgs::default();
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--root" => args.root = PathBuf::from(next_value(&mut iter, "--root")?),
			"--action" => args.action = Some(next_value(&mut iter, "--action")?.to_string_lossy().into_owned()),
			"--help" | "-h" => {
				eprintln!("usage: cargo xtask research penn-action summary [--root target/research/penn-action] [--action jumping_jacks]");
				std::process::exit(0);
			}
			other => bail!("unknown Penn Action summary option: {other}"),
		}
	}
	Ok(args)
}

fn print_penn_action_summary(manifest: &PennActionManifest) {
	let mut actions = BTreeMap::<String, PennActionActionSummary>::new();
	for sequence in &manifest.sequences {
		let action = sequence.action.clone().unwrap_or_else(|| "unknown".to_string());
		let summary = actions.entry(action).or_default();
		summary.sequences += 1;
		summary.frames += sequence.frame_count;
		if let Some(pose) = &sequence.pose {
			*summary.poses.entry(pose.clone()).or_default() += 1;
		}
		if sequence.video_320w.is_some() {
			summary.videos.push(sequence.id.clone());
		}
	}
	println!("Penn Action sequences: {}", manifest.sequences.len());
	println!(
		"Prepared videos: {}",
		manifest.sequences.iter().filter(|sequence| sequence.video_320w.is_some()).count()
	);
	println!();
	println!("| action | sequences | frames | videos | prepared ids |");
	println!("|---|---:|---:|---:|---|");
	for (action, summary) in actions {
		let prepared_ids = if summary.videos.is_empty() {
			"-".to_string()
		} else {
			summary.videos.iter().take(8).cloned().collect::<Vec<_>>().join(",")
		};
		println!(
			"| {} | {} | {} | {} | {} |",
			action,
			summary.sequences,
			summary.frames,
			summary.videos.len(),
			prepared_ids
		);
	}
}

fn print_penn_action_detail(manifest: &PennActionManifest, action: &str) -> Result<()> {
	let sequences = manifest
		.sequences
		.iter()
		.filter(|sequence| sequence.action.as_deref() == Some(action))
		.collect::<Vec<_>>();
	if sequences.is_empty() {
		bail!("Penn Action action {action} is not in manifest");
	}
	println!("Penn Action action: {action}");
	println!("Sequences: {}", sequences.len());
	println!();
	println!("| id | pose | frames | video320w |");
	println!("|---|---|---:|---|");
	for sequence in sequences {
		println!(
			"| {} | {} | {} | {} |",
			sequence.id,
			sequence.pose.as_deref().unwrap_or("-"),
			sequence.frame_count,
			sequence.video_320w.as_deref().unwrap_or("-")
		);
	}
	Ok(())
}

fn penn_action_desktop_config_toml(sequence: &str, video: &Path, ffmpeg: &Path, width: u32, height: u32, fps: u32, repeat: bool) -> String {
	let id = format!("penn-action-{sequence}");
	let label = format!("Penn Action {sequence}");
	let source_id = format!("penn-action:{sequence}");
	desktop_file_video_config_toml(&id, &label, &source_id, video, ffmpeg, width, height, fps, repeat)
}

fn desktop_file_video_config_toml(
	input_id: &str,
	source_label: &str,
	source_id: &str,
	video: &Path,
	ffmpeg: &Path,
	width: u32,
	height: u32,
	fps: u32,
	repeat: bool,
) -> String {
	format!(
		r#"[desktop.runtime_selection]
device = ""
engine = "mediapipe-native"
mediaPipeRunningMode = "video"
mediaPipeHolisticEnabled = true
fps = {fps}
resolution = "{width}x{height}"
zenohEnabled = false
vmcEnabled = true
vmcTargetAddr = "127.0.0.1:39539"
vmcChestStabilizationEnabled = false
vmcChestStabilizationStrength = 0.6
zenohKeyExpr = "un-motion/frame"
minimizeToTray = false

[desktop.runtime_selection.modifier]
headEnabled = true
faceEnabled = true
handsEnabled = true
armsIkEnabled = true
torsoEnabled = false
legsEnabled = false
feetEnabled = false
cameraDiagonalViewAngleDeg = 70.0
minLandmarkConfidence = 0.55
mirrorMode = "normal"
smoothingPreset = "adaptive"

[desktop.runtime_selection.modifier.postProcessRules]
headFromPose = true
headFromFaceMatrix = true
headReconcile = true
neutralEyeFallback = true
handCameraTarget = true
handOrientation = true
fingerDerived = true
armFromPose = true
armIkFromHands = true
crossedHandHeuristic = true
coordinateCorrection = true
finalClamp = true

[components.input]
id = "{input_id}"
kind = "file-video"

[components.input.settings]
path = "{video}"
source_id = "{source_id}"
source_label = "{source_label}"
fps = {fps}
output_width = {width}
output_height = {height}
repeat = {repeat}
ffmpeg_path = "{ffmpeg}"

[components.input_buffer]
id = "input-ring"
kind = "ring"
capacity = 8
overflow = "block-producer"

[components.engine]
id = "mp-native"
kind = "media-pipe-native"

[components.post_process]
id = "mp-default"
kind = "media-pipe-default"

[components.output_buffer]
id = "output-ring"
kind = "ring"
capacity = 8
overflow = "block-producer"

[[components.outputs]]
id = "vmc"
kind = "vmc"
enabled = true

[[components.outputs]]
id = "zenoh"
kind = "zenoh"
enabled = false
"#,
		input_id = toml_string_value(input_id),
		source_id = toml_string_value(source_id),
		source_label = toml_string_value(source_label),
		video = toml_string_path(video),
		ffmpeg = toml_string_path(ffmpeg)
	)
}

fn toml_string_value(value: &str) -> String {
	value.replace('"', "\\\"")
}

fn toml_string_path(path: &Path) -> String {
	path_slash(path).replace('"', "\\\"")
}

fn extract_penn_action_archive(archive: &Path, raw_dir: &Path, force: bool) -> Result<()> {
	if !archive.exists() {
		bail!("Penn Action archive is missing: {}", archive.display());
	}
	if force && raw_dir.exists() {
		fs::remove_dir_all(raw_dir).with_context(|| format!("failed to remove {}", raw_dir.display()))?;
	}
	if find_penn_action_frames_dir(raw_dir).is_ok() {
		return Ok(());
	}
	fs::create_dir_all(raw_dir).with_context(|| format!("failed to create {}", raw_dir.display()))?;
	let mut cmd = Command::new(resolve_tool("tar"));
	cmd.arg("-xzf").arg(archive).arg("-C").arg(raw_dir);
	run_command("extract Penn Action", &mut cmd)
}

fn find_penn_action_frames_dir(raw_dir: &Path) -> Result<PathBuf> {
	for candidate in [
		raw_dir.join("Penn_Action/frames"),
		raw_dir.join("PennAction/frames"),
		raw_dir.join("frames"),
	] {
		if candidate.is_dir() {
			return Ok(candidate);
		}
	}
	bail!("failed to find Penn Action frames directory under {}", raw_dir.display())
}

fn find_penn_action_labels_dir(raw_dir: &Path) -> Result<PathBuf> {
	for candidate in [
		raw_dir.join("Penn_Action/labels"),
		raw_dir.join("PennAction/labels"),
		raw_dir.join("labels"),
	] {
		if candidate.is_dir() {
			return Ok(candidate);
		}
	}
	bail!("failed to find Penn Action labels directory under {}", raw_dir.display())
}

#[derive(Clone, Debug, Default)]
struct PennActionLabelMeta {
	action: Option<String>,
	pose: Option<String>,
}

fn collect_penn_action_sequences(frames_dir: &Path, labels_dir: Option<&Path>) -> Result<Vec<PennActionSequence>> {
	let mut sequences = Vec::new();
	for entry in fs::read_dir(frames_dir).with_context(|| format!("failed to read {}", frames_dir.display()))? {
		let entry = entry?;
		let sequence_dir = entry.path();
		if !sequence_dir.is_dir() {
			continue;
		}
		let Some(id) = sequence_dir.file_name().and_then(|name| name.to_str()).map(ToString::to_string) else {
			continue;
		};
		let mut frames = fs::read_dir(&sequence_dir)
			.with_context(|| format!("failed to read {}", sequence_dir.display()))?
			.filter_map(|entry| entry.ok().map(|entry| entry.path()))
			.filter(|path| {
				path.extension()
					.and_then(|ext| ext.to_str())
					.is_some_and(|ext| ext.eq_ignore_ascii_case("jpg"))
			})
			.collect::<Vec<_>>();
		frames.sort();
		if frames.is_empty() {
			continue;
		}
		let label_file = labels_dir.map(|dir| dir.join(format!("{id}.mat"))).filter(|path| path.exists());
		let label_meta = label_file
			.as_deref()
			.and_then(|path| read_penn_action_label_meta(path).ok())
			.unwrap_or_default();
		sequences.push(PennActionSequence {
			id,
			action: label_meta.action,
			pose: label_meta.pose,
			frame_count: frames.len(),
			first_frame: path_slash(frames.first().expect("non-empty frames")),
			last_frame: path_slash(frames.last().expect("non-empty frames")),
			label_file: label_file.as_deref().map(path_slash),
			video_320w: None,
		});
	}
	sequences.sort_by(|a, b| a.id.cmp(&b.id));
	Ok(sequences)
}

fn read_penn_action_label_meta(path: &Path) -> Result<PennActionLabelMeta> {
	let data = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
	let mut meta = PennActionLabelMeta::default();
	let mut offset = 128;
	while offset + 8 <= data.len() {
		let tag_type = u32::from_le_bytes(data[offset..offset + 4].try_into().expect("tag type"));
		let tag_len = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().expect("tag len")) as usize;
		offset += 8;
		if offset + tag_len > data.len() {
			break;
		}
		if tag_type == 15 {
			let mut decoder = ZlibDecoder::new(&data[offset..offset + tag_len]);
			let mut decoded = Vec::new();
			if decoder.read_to_end(&mut decoded).is_ok() {
				if meta.action.is_none() {
					meta.action = find_ascii_candidate(&decoded, PENN_ACTION_LABEL_ACTIONS).map(ToString::to_string);
				}
				if meta.pose.is_none() {
					meta.pose = find_ascii_candidate(&decoded, PENN_ACTION_LABEL_POSES).map(ToString::to_string);
				}
			}
		}
		offset += tag_len;
	}
	Ok(meta)
}

const PENN_ACTION_LABEL_ACTIONS: &[&str] = &[
	"baseball_pitch",
	"baseball_swing",
	"bench_press",
	"bowl",
	"clean_and_jerk",
	"golf_swing",
	"jump_rope",
	"jumping_jacks",
	"pullup",
	"pushup",
	"situp",
	"squat",
	"strum_guitar",
	"tennis_forehand",
	"tennis_serve",
];

const PENN_ACTION_LABEL_POSES: &[&str] = &["front", "back", "left", "right"];

fn find_ascii_candidate<'a>(data: &[u8], candidates: &'a [&'a str]) -> Option<&'a str> {
	candidates
		.iter()
		.copied()
		.find(|candidate| contains_ascii(data, candidate.as_bytes()))
}

fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
	!needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}

fn create_penn_action_videos(
	repo: &Path,
	frames_dir: &Path,
	videos_dir: &Path,
	fps: u32,
	video_limit: usize,
	video_actions: &[String],
	ffmpeg_path: Option<&Path>,
	sequences: &mut [PennActionSequence],
) -> Result<()> {
	let ffmpeg = find_ffmpeg(repo, ffmpeg_path)
		.context("ffmpeg is required to create Penn Action mp4 files; run `cargo xtask research ffmpeg prepare`, set --ffmpeg, or rerun with --skip-videos")?;
	fs::create_dir_all(videos_dir).with_context(|| format!("failed to create {}", videos_dir.display()))?;
	let selected = select_penn_action_video_indices(sequences, video_limit, video_actions);
	for index in selected {
		let sequence = &mut sequences[index];
		let input_pattern = frames_dir.join(&sequence.id).join("%06d.jpg");
		let output = videos_dir.join(format!("{}.mp4", sequence.id));
		if !output.exists() {
			let mut cmd = Command::new(&ffmpeg);
			cmd.current_dir(repo)
				.args(["-y", "-hide_banner", "-loglevel", "error", "-framerate"])
				.arg(fps.to_string())
				.arg("-i")
				.arg(&input_pattern)
				.args(["-vf", "scale=320:-2", "-pix_fmt", "yuv420p"])
				.arg(&output);
			run_command(&format!("ffmpeg Penn Action {}", sequence.id), &mut cmd)?;
		}
		sequence.video_320w = Some(path_slash(&output));
	}
	Ok(())
}

fn select_penn_action_video_indices(sequences: &[PennActionSequence], video_limit: usize, video_actions: &[String]) -> Vec<usize> {
	if video_actions.is_empty() {
		return (0..sequences.len().min(video_limit)).collect();
	}
	let mut selected = Vec::new();
	for action in video_actions {
		if let Some((index, _)) = sequences
			.iter()
			.enumerate()
			.find(|(index, sequence)| !selected.contains(index) && sequence.action.as_deref() == Some(action.as_str()))
		{
			selected.push(index);
			if selected.len() >= video_limit {
				break;
			}
		}
	}
	selected
}

fn find_ffmpeg(repo: &Path, configured: Option<&Path>) -> Result<OsString> {
	let exe_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
	let mut candidates = Vec::new();
	if let Some(path) = configured {
		candidates.push(absolutize(repo, path));
	}
	if let Some(path) = std::env::var_os("FFMPEG").map(PathBuf::from) {
		candidates.push(path);
	}
	candidates.push(repo.join("target/tools/ffmpeg").join(exe_name));
	candidates.push(repo.join("tools/ffmpeg").join(exe_name));
	for candidate in candidates {
		if candidate.exists() {
			return Ok(candidate.into_os_string());
		}
	}
	find_tool("ffmpeg")
}

#[derive(Debug)]
struct FfmpegPrepareArgs {
	root: PathBuf,
	archive_url: String,
	force_extract: bool,
}

impl Default for FfmpegPrepareArgs {
	fn default() -> Self {
		Self {
			root: PathBuf::from("target/tools/ffmpeg"),
			archive_url: FFMPEG_RELEASE_ESSENTIALS_URL.to_string(),
			force_extract: false,
		}
	}
}

fn research_ffmpeg(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let Some(subcommand) = args.first().map(|arg| arg.to_string_lossy()) else {
		eprintln!("usage: cargo xtask research ffmpeg prepare [--root target/tools/ffmpeg]");
		bail!("missing ffmpeg subcommand");
	};
	match subcommand.as_ref() {
		"prepare" => prepare_portable_ffmpeg(repo, args[1..].to_vec()),
		"--help" | "-h" | "help" => {
			eprintln!("usage: cargo xtask research ffmpeg prepare [--root target/tools/ffmpeg]");
			Ok(())
		}
		other => bail!("unknown ffmpeg subcommand: {other}"),
	}
}

fn prepare_portable_ffmpeg(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_ffmpeg_prepare_args(raw_args)?;
	let root = absolutize(repo, &args.root);
	let archive = root.join("archive/ffmpeg-release-essentials.zip");
	let dist_dir = root.join("dist");
	let ffmpeg = root.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
	ensure_download(repo, "FFmpeg release essentials", &args.archive_url, &archive)?;
	if args.force_extract && dist_dir.exists() {
		fs::remove_dir_all(&dist_dir).with_context(|| format!("failed to remove {}", dist_dir.display()))?;
	}
	if !ffmpeg.exists() {
		extract_zip_archive(&archive, &dist_dir)?;
		let extracted = find_file_named(&dist_dir, if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })?;
		fs::copy(&extracted, &ffmpeg).with_context(|| format!("failed to copy {} to {}", extracted.display(), ffmpeg.display()))?;
	}
	let mut cmd = Command::new(&ffmpeg);
	cmd.arg("-version");
	run_command("probe portable ffmpeg", &mut cmd)?;
	eprintln!("wrote {}", ffmpeg.display());
	Ok(())
}

fn parse_ffmpeg_prepare_args(raw_args: Vec<OsString>) -> Result<FfmpegPrepareArgs> {
	let mut args = FfmpegPrepareArgs::default();
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--root" => args.root = PathBuf::from(next_value(&mut iter, "--root")?),
			"--archive-url" => args.archive_url = next_value(&mut iter, "--archive-url")?.to_string_lossy().into_owned(),
			"--force-extract" => args.force_extract = true,
			"--help" | "-h" => {
				eprintln!("usage: cargo xtask research ffmpeg prepare [--root target/tools/ffmpeg] [--archive-url URL] [--force-extract]");
				std::process::exit(0);
			}
			other => bail!("unknown FFmpeg prepare option: {other}"),
		}
	}
	Ok(args)
}

fn extract_zip_archive(archive: &Path, out_dir: &Path) -> Result<()> {
	fs::create_dir_all(out_dir).with_context(|| format!("failed to create {}", out_dir.display()))?;
	let file = File::open(archive).with_context(|| format!("failed to open {}", archive.display()))?;
	let mut zip = zip::ZipArchive::new(file).with_context(|| format!("failed to read {}", archive.display()))?;
	for index in 0..zip.len() {
		let mut entry = zip.by_index(index).with_context(|| format!("failed to read zip entry {index}"))?;
		let Some(enclosed_name) = entry.enclosed_name() else {
			continue;
		};
		let target = out_dir.join(enclosed_name);
		if entry.is_dir() {
			fs::create_dir_all(&target).with_context(|| format!("failed to create {}", target.display()))?;
			continue;
		}
		if let Some(parent) = target.parent() {
			fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
		}
		let mut out = File::create(&target).with_context(|| format!("failed to create {}", target.display()))?;
		std::io::copy(&mut entry, &mut out).with_context(|| format!("failed to extract {}", target.display()))?;
	}
	Ok(())
}

fn find_file_named(root: &Path, name: &str) -> Result<PathBuf> {
	let mut stack = vec![root.to_path_buf()];
	while let Some(dir) = stack.pop() {
		for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
			let entry = entry?;
			let path = entry.path();
			if path.is_dir() {
				stack.push(path);
			} else if path.file_name().and_then(|file_name| file_name.to_str()) == Some(name) {
				return Ok(path);
			}
		}
	}
	bail!("failed to find {name} under {}", root.display())
}

fn find_tool(program: &str) -> Result<OsString> {
	let tool = resolve_tool(program);
	let mut cmd = Command::new(if cfg!(windows) { "where" } else { "which" });
	cmd.arg(&tool);
	let output = cmd.output().with_context(|| format!("failed to locate {program}"))?;
	if output.status.success() {
		if let Some(first) = String::from_utf8_lossy(&output.stdout).lines().next() {
			let value = first.trim();
			if !value.is_empty() {
				return Ok(OsString::from(value));
			}
		}
	}
	bail!("{program} was not found on PATH")
}

fn path_slash(path: &Path) -> String {
	path.to_string_lossy().replace('\\', "/")
}

#[derive(Debug)]
struct MakeReleasePackageArgs {
	version: Option<String>,
	output_dir: PathBuf,
	skip_build: bool,
	keep_staging: bool,
}

fn parse_make_release_package_args(raw_args: Vec<OsString>) -> Result<MakeReleasePackageArgs> {
	let mut version = None;
	let mut output_dir = PathBuf::from("release-packages");
	let mut skip_build = false;
	let mut keep_staging = false;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--version" => version = Some(next_value(&mut iter, "--version")?.to_string_lossy().into_owned()),
			"--output-dir" => output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?),
			"--skip-build" => skip_build = true,
			"--keep-staging" => keep_staging = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask make-release-package [--version 1.2.3.beta-1] [--output-dir release-packages] [--skip-build] [--keep-staging]"
				);
				std::process::exit(0);
			}
			other => bail!("unexpected make-release-package argument: {other}"),
		}
	}
	Ok(MakeReleasePackageArgs {
		version,
		output_dir,
		skip_build,
		keep_staging,
	})
}

fn default_release_version(repo: &Path) -> Result<String> {
	let package_json = repo.join("apps/un-motion-supervisor/package.json");
	let raw = fs::read_to_string(&package_json).with_context(|| format!("failed to read {}", package_json.display()))?;
	let value: serde_json::Value = serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", package_json.display()))?;
	let version = value
		.get("version")
		.and_then(|value| value.as_str())
		.map(str::trim)
		.filter(|version| !version.is_empty())
		.with_context(|| format!("version is missing in {}", package_json.display()))?;
	Ok(version.to_string())
}

fn package_file_version(version: &str) -> Result<String> {
	let trimmed = version.trim();
	if trimmed.is_empty() {
		bail!("--version must not be empty");
	}
	let normalized = trimmed.replace(".alpha", "-alpha").replace(".beta", "-beta").replace(".rc", "-rc");
	if !normalized.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-') {
		bail!("--version may only contain ASCII letters, digits, '.', and '-' after prerelease normalization");
	}
	Ok(normalized)
}

fn release_package_manifest(
	package_name: &str,
	version: &str,
	native_artifacts: &[ReleaseArtifact],
	model_artifacts: &[ReleaseArtifact],
) -> String {
	format!(
		"package: {package_name}\nversion: {version}\nlauncher: {}\ncore_executable: un-motion-core{}\nsupervisor_executable: un-motion-supervisor{}\ncapturer_executable: un-motion-capturer{}\nnative_artifacts:\n{}\nmodel_artifacts:\n{}\n",
		supervisor_launcher_name(),
		env::consts::EXE_SUFFIX,
		env::consts::EXE_SUFFIX,
		env::consts::EXE_SUFFIX,
		native_artifacts
			.iter()
			.map(|artifact| format!("- {}", artifact.package_path))
			.collect::<Vec<_>>()
			.join("\n"),
		model_artifacts
			.iter()
			.map(|artifact| format!("- {}", artifact.package_path))
			.collect::<Vec<_>>()
			.join("\n")
	)
}

fn supervisor_launcher_name() -> &'static str {
	if cfg!(windows) {
		"Start UN Motion Supervisor.bat"
	} else {
		"start-un-motion-supervisor.sh"
	}
}

/// Supervisor を起動する launcher script。Supervisor は内部で
/// `un-motion-capturer.exe` を loopback に spawn するため、ここでは
/// supervisor.exe を直接起動するだけでよい。capturer / supervisor が
/// 同じディレクトリに存在することだけ確認する。
fn supervisor_launcher_script() -> String {
	let supervisor = format!("un-motion-supervisor{}", env::consts::EXE_SUFFIX);
	let capturer = format!("un-motion-capturer{}", env::consts::EXE_SUFFIX);
	if cfg!(windows) {
		format!(
			"@echo off\r\nsetlocal\r\ncd /d \"%~dp0\"\r\nif not exist \"%~dp0{supervisor}\" (\r\n  echo Missing {supervisor}\r\n  exit /b 1\r\n)\r\nif not exist \"%~dp0{capturer}\" (\r\n  echo Missing {capturer}\r\n  exit /b 1\r\n)\r\nstart \"\" \"%~dp0{supervisor}\"\r\n"
		)
	} else {
		format!(
			"#!/bin/sh\nset -eu\nDIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nif [ ! -x \"$DIR/{supervisor}\" ]; then\n  echo \"Missing $DIR/{supervisor}\" >&2\n  exit 1\nfi\nif [ ! -x \"$DIR/{capturer}\" ]; then\n  echo \"Missing $DIR/{capturer}\" >&2\n  exit 1\nfi\nexec \"$DIR/{supervisor}\"\n"
		)
	}
}

fn copy_release_file(source: &Path, destination: &Path) -> Result<()> {
	if !source.exists() {
		bail!("missing release package file: {}", source.display());
	}
	if let Some(parent) = destination.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	fs::copy(source, destination).with_context(|| format!("failed to copy {} to {}", source.display(), destination.display()))?;
	Ok(())
}

fn copy_release_dir(source: &Path, destination: &Path) -> Result<()> {
	if !source.exists() {
		bail!("missing release package directory: {}", source.display());
	}
	for entry in fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))? {
		let entry = entry.with_context(|| format!("failed to read entry under {}", source.display()))?;
		let source_path = entry.path();
		let destination_path = destination.join(entry.file_name());
		let file_type = entry
			.file_type()
			.with_context(|| format!("failed to read file type for {}", source_path.display()))?;
		if file_type.is_dir() {
			copy_release_dir(&source_path, &destination_path)?;
		} else if file_type.is_file() {
			copy_release_file(&source_path, &destination_path)?;
		}
	}
	Ok(())
}

fn add_dir_to_zip(zip: &mut zip::ZipWriter<File>, options: zip::write::SimpleFileOptions, staging_root: &Path, dir: &Path) -> Result<()> {
	let mut entries = fs::read_dir(dir)
		.with_context(|| format!("failed to read {}", dir.display()))?
		.collect::<std::result::Result<Vec<_>, _>>()
		.with_context(|| format!("failed to read entries under {}", dir.display()))?;
	entries.sort_by_key(|entry| entry.path());

	for entry in entries {
		let path = entry.path();
		let file_type = entry
			.file_type()
			.with_context(|| format!("failed to read file type for {}", path.display()))?;
		if file_type.is_dir() {
			add_dir_to_zip(zip, options, staging_root, &path)?;
		} else if file_type.is_file() {
			let name = path
				.strip_prefix(staging_root)
				.with_context(|| format!("failed to compute package path for {}", path.display()))?;
			add_file_to_zip(zip, options, &path, &path_slash(name))?;
		}
	}
	Ok(())
}

fn add_file_to_zip(zip: &mut zip::ZipWriter<File>, options: zip::write::SimpleFileOptions, source: &Path, name: &str) -> Result<()> {
	let mut file = File::open(source).with_context(|| format!("failed to open {}", source.display()))?;
	zip.start_file(name.replace('\\', "/"), options)
		.with_context(|| format!("failed to add {name} to package"))?;
	let mut buffer = Vec::new();
	file.read_to_end(&mut buffer)
		.with_context(|| format!("failed to read {}", source.display()))?;
	zip.write_all(&buffer)
		.with_context(|| format!("failed to write {name} to package"))?;
	Ok(())
}

fn verify(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let mut skip_frontend = false;
	let mut skip_rust = false;
	for arg in args {
		match arg.to_string_lossy().as_ref() {
			"--skip-frontend" => skip_frontend = true,
			"--skip-rust" => skip_rust = true,
			"--help" | "-h" => {
				eprintln!("usage: cargo xtask verify [--skip-frontend] [--skip-rust]");
				return Ok(());
			}
			other => bail!("unexpected verify argument: {other}"),
		}
	}

	if !skip_frontend {
		run(repo.join("apps/un-motion-supervisor"), "npm", ["run", "build"])?;
	}
	if !skip_rust {
		run(repo, "cargo", ["fmt", "--all", "--", "--check"])?;
		run(repo, "cargo", ["test", "--workspace"])?;
	}
	Ok(())
}

fn mediapipe(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let Some(subcommand) = args.first() else {
		eprintln!(
			"usage: cargo xtask mediapipe <build-native|native-probe|native-image-pipeline-probe|native-camera-probe|pose-fixtures> [options]"
		);
		bail!("missing mediapipe subcommand");
	};
	let rest = args[1..].to_vec();
	match subcommand.to_string_lossy().as_ref() {
		"build-native" | "build" => build_native_mediapipe(repo, rest),
		"native-probe" | "probe" => {
			let pass = strip_double_dash(rest);
			let mut cmd = Command::new("cargo");
			cmd.current_dir(repo)
				.arg("run")
				.arg("-q")
				.arg("-p")
				.arg("un-motion-output-vmc")
				.arg("--bin")
				.arg("un-motion-native-mediapipe-probe")
				.arg("--")
				.args(pass);
			run_command("mediapipe native-probe", &mut cmd)
		}
		"native-image-pipeline-probe" | "image-pipeline-probe" | "pipeline-probe" => {
			let pass = strip_double_dash(rest);
			let mut cmd = Command::new("cargo");
			cmd.current_dir(repo)
				.arg("run")
				.arg("-q")
				.arg("-p")
				.arg("un-motion-output-vmc")
				.arg("--bin")
				.arg("un-motion-native-image-pipeline-probe")
				.arg("--")
				.args(pass);
			run_command("mediapipe native-image-pipeline-probe", &mut cmd)
		}
		"native-camera-probe" | "camera-probe" => {
			let pass = strip_double_dash(rest);
			let mut cmd = Command::new("cargo");
			cmd.current_dir(repo)
				.arg("run")
				.arg("-q")
				.arg("-p")
				.arg("un-motion-output-vmc")
				.arg("--bin")
				.arg("un-motion-native-camera-probe")
				.arg("--")
				.args(pass);
			run_command("mediapipe native-camera-probe", &mut cmd)
		}
		"pose-fixtures" | "pose-regression" => mediapipe_pose_fixtures(repo, rest),
		"--help" | "-h" | "help" => {
			eprintln!(
				"usage:
  cargo xtask mediapipe build-native [--media-pipe-root third_party/mediapipe] [--out native/mediapipe/un-motion-mediapipe.dll]
  cargo xtask mediapipe native-probe -- --image image.png [probe args]
  cargo xtask mediapipe native-image-pipeline-probe -- --image image.png [probe args]
  cargo xtask mediapipe native-camera-probe -- [--list] [--backend nokhwa|directshow] [--device camera] [probe args]
  cargo xtask mediapipe pose-fixtures [--dir tests/pose] [--max-abs-head-pitch 0.25] [probe args]"
			);
			Ok(())
		}
		other => bail!("unknown mediapipe subcommand: {other}"),
	}
}

fn mediapipe_pose_fixtures(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let mut fixture_dir = repo.join("tests").join("pose");
	let mut max_abs_head_pitch = 0.25_f64;
	let mut max_abs_head_pitch_override = false;
	let mut head_diagnostics = false;
	let mut pass = Vec::new();
	let mut args = raw_args.into_iter().peekable();
	while let Some(arg) = args.next() {
		match arg.to_string_lossy().as_ref() {
			"--dir" => fixture_dir = repo.join(next_value(&mut args, "--dir")?),
			"--max-abs-head-pitch" => {
				let value = next_value(&mut args, "--max-abs-head-pitch")?;
				max_abs_head_pitch = value
					.to_string_lossy()
					.parse::<f64>()
					.with_context(|| format!("invalid --max-abs-head-pitch {}", value.to_string_lossy()))?;
				max_abs_head_pitch_override = true;
			}
			"--head-diagnostics" => head_diagnostics = true,
			"--" => {
				pass.extend(args.by_ref());
				break;
			}
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask mediapipe pose-fixtures [--dir tests/pose] [--max-abs-head-pitch 0.25] [--head-diagnostics] [-- probe args]"
				);
				return Ok(());
			}
			_ => pass.push(arg),
		}
	}
	if !fixture_dir.is_dir() {
		bail!("pose fixture directory not found: {}", fixture_dir.display());
	}
	let fixtures = load_pose_fixture_specs(&fixture_dir, max_abs_head_pitch, max_abs_head_pitch_override)?;
	if fixtures.is_empty() {
		bail!("no .png pose fixtures in {}", fixture_dir.display());
	}

	run(
		repo,
		"cargo",
		[
			"build",
			"-q",
			"-p",
			"un-motion-output-vmc",
			"--bin",
			"un-motion-native-image-pipeline-probe",
		],
	)?;
	let probe = repo.join(format!(
		"target/debug/un-motion-native-image-pipeline-probe{}",
		env::consts::EXE_SUFFIX
	));
	for fixture in fixtures {
		let image = fixture_dir.join(&fixture.file);
		let report = run_pose_fixture_probe(repo, &probe, &image, &pass)?;
		let head_yaw = signal_value_from_probe_report(&report, "head.yaw");
		let head_pitch = signal_value_from_probe_report(&report, "head.pitch");
		let head_roll = signal_value_from_probe_report(&report, "head.roll");
		let left_palm_normal_z = signal_value_from_probe_report(&report, "hand.left.palm.normal.z");
		let right_palm_normal_z = signal_value_from_probe_report(&report, "hand.right.palm.normal.z");
		let left_hand_palm_z = hand_palm_front_from_probe_report(&report, "LeftHand").map(|front| front[2]);
		let right_hand_palm_z = hand_palm_front_from_probe_report(&report, "RightHand").map(|front| front[2]);
		let head_front = head_front_from_probe_report(&report);
		println!(
			"{} [{}]: yaw={} pitch={} roll={} front={} palmNormalZ=({}, {}) handPalmZ=({}, {})",
			fixture.file,
			pose_fixture_kind(&fixture.kind),
			format_optional_f64(head_yaw),
			format_optional_f64(head_pitch),
			format_optional_f64(head_roll),
			format_optional_vec3(head_front),
			format_optional_f64(left_palm_normal_z),
			format_optional_f64(right_palm_normal_z),
			format_optional_f64(left_hand_palm_z),
			format_optional_f64(right_hand_palm_z)
		);
		check_pose_fixture_signal(&fixture, "head.yaw", head_yaw)?;
		check_pose_fixture_signal(&fixture, "head.pitch", head_pitch)?;
		check_pose_fixture_signal(&fixture, "head.roll", head_roll)?;
		check_pose_fixture_signal(&fixture, "hand.left.palm.normal.z", left_palm_normal_z)?;
		check_pose_fixture_signal(&fixture, "hand.right.palm.normal.z", right_palm_normal_z)?;
		check_pose_fixture_signal(&fixture, "LeftHand.palm.z", left_hand_palm_z)?;
		check_pose_fixture_signal(&fixture, "RightHand.palm.z", right_hand_palm_z)?;
		if head_diagnostics {
			print_pose_fixture_head_diagnostics(repo, &probe, &image, &pass)?;
		}
	}
	Ok(())
}

fn load_pose_fixture_specs(fixture_dir: &Path, max_abs_head_pitch: f64, max_abs_head_pitch_override: bool) -> Result<Vec<PoseFixtureSpec>> {
	let manifest_path = fixture_dir.join("fixtures.toml");
	if manifest_path.is_file() {
		let raw = fs::read_to_string(&manifest_path).with_context(|| format!("failed to read {}", manifest_path.display()))?;
		let mut manifest: PoseFixtureManifest =
			toml::from_str(&raw).with_context(|| format!("failed to parse {}", manifest_path.display()))?;
		for fixture in &mut manifest.fixture {
			if max_abs_head_pitch_override && fixture.max_abs_head_pitch.is_some() {
				fixture.max_abs_head_pitch = Some(max_abs_head_pitch);
			}
			let path = fixture_dir.join(&fixture.file);
			if !path.is_file() {
				bail!("pose fixture listed in manifest was not found: {}", path.display());
			}
		}
		return Ok(manifest.fixture);
	}

	let mut fixtures = fs::read_dir(fixture_dir)
		.with_context(|| format!("read pose fixture dir {}", fixture_dir.display()))?
		.filter_map(|entry| entry.ok().map(|entry| entry.path()))
		.filter(|path| {
			path.extension()
				.and_then(|ext| ext.to_str())
				.is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
		})
		.collect::<Vec<_>>();
	fixtures.sort();
	Ok(fixtures
		.into_iter()
		.filter_map(|path| {
			path.file_name().and_then(|name| name.to_str()).map(|file| PoseFixtureSpec {
				file: file.to_string(),
				kind: "front".to_string(),
				max_abs_head_pitch: Some(max_abs_head_pitch),
				..PoseFixtureSpec::default()
			})
		})
		.collect())
}

fn check_pose_fixture_signal(fixture: &PoseFixtureSpec, signal: &str, value: Option<f64>) -> Result<()> {
	let Some(value) = value else {
		if fixture.has_check_for_signal(signal) {
			bail!("{}: probe report missing {signal}", fixture.file);
		}
		return Ok(());
	};
	if let Some(max_abs) = fixture.max_abs_for_signal(signal) {
		if value.abs() > max_abs {
			bail!("{}: {signal} {value:.3} exceeds ±{max_abs:.3}", fixture.file);
		}
	}
	if let Some(min) = fixture.min_for_signal(signal) {
		if value < min {
			bail!("{}: {signal} {value:.3} is below {min:.3}", fixture.file);
		}
	}
	if let Some(max) = fixture.max_for_signal(signal) {
		if value > max {
			bail!("{}: {signal} {value:.3} is above {max:.3}", fixture.file);
		}
	}
	Ok(())
}

fn format_optional_f64(value: Option<f64>) -> String {
	value.map(|value| format!("{value:.3}")).unwrap_or_else(|| "n/a".to_string())
}

fn format_optional_vec3(value: Option<[f64; 3]>) -> String {
	value
		.map(|value| format!("({:.3},{:.3},{:.3})", value[0], value[1], value[2]))
		.unwrap_or_else(|| "(n/a,n/a,n/a)".to_string())
}

fn pose_fixture_kind(kind: &str) -> &str {
	if kind.is_empty() { "pose" } else { kind }
}

#[derive(Debug, Deserialize)]
struct PoseFixtureManifest {
	fixture: Vec<PoseFixtureSpec>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct PoseFixtureSpec {
	file: String,
	#[serde(default)]
	kind: String,
	#[serde(default)]
	max_abs_head_yaw: Option<f64>,
	#[serde(default)]
	max_abs_head_pitch: Option<f64>,
	#[serde(default)]
	max_abs_head_roll: Option<f64>,
	#[serde(default)]
	min_head_yaw: Option<f64>,
	#[serde(default)]
	max_head_yaw: Option<f64>,
	#[serde(default)]
	min_head_pitch: Option<f64>,
	#[serde(default)]
	max_head_pitch: Option<f64>,
	#[serde(default)]
	min_head_roll: Option<f64>,
	#[serde(default)]
	max_head_roll: Option<f64>,
	#[serde(default)]
	min_left_palm_normal_z: Option<f64>,
	#[serde(default)]
	min_right_palm_normal_z: Option<f64>,
	#[serde(default)]
	max_left_palm_normal_z: Option<f64>,
	#[serde(default)]
	max_right_palm_normal_z: Option<f64>,
	#[serde(default)]
	max_abs_left_palm_normal_z: Option<f64>,
	#[serde(default)]
	max_abs_right_palm_normal_z: Option<f64>,
	#[serde(default)]
	min_left_hand_palm_z: Option<f64>,
	#[serde(default)]
	min_right_hand_palm_z: Option<f64>,
	#[serde(default)]
	max_left_hand_palm_z: Option<f64>,
	#[serde(default)]
	max_right_hand_palm_z: Option<f64>,
}

impl PoseFixtureSpec {
	fn has_check_for_signal(&self, signal: &str) -> bool {
		self.max_abs_for_signal(signal).is_some() || self.min_for_signal(signal).is_some() || self.max_for_signal(signal).is_some()
	}

	fn max_abs_for_signal(&self, signal: &str) -> Option<f64> {
		match signal {
			"head.yaw" => self.max_abs_head_yaw,
			"head.pitch" => self.max_abs_head_pitch,
			"head.roll" => self.max_abs_head_roll,
			"hand.left.palm.normal.z" => self.max_abs_left_palm_normal_z,
			"hand.right.palm.normal.z" => self.max_abs_right_palm_normal_z,
			_ => None,
		}
	}

	fn min_for_signal(&self, signal: &str) -> Option<f64> {
		match signal {
			"head.yaw" => self.min_head_yaw,
			"head.pitch" => self.min_head_pitch,
			"head.roll" => self.min_head_roll,
			"hand.left.palm.normal.z" => self.min_left_palm_normal_z,
			"hand.right.palm.normal.z" => self.min_right_palm_normal_z,
			"LeftHand.palm.z" => self.min_left_hand_palm_z,
			"RightHand.palm.z" => self.min_right_hand_palm_z,
			_ => None,
		}
	}

	fn max_for_signal(&self, signal: &str) -> Option<f64> {
		match signal {
			"head.yaw" => self.max_head_yaw,
			"head.pitch" => self.max_head_pitch,
			"head.roll" => self.max_head_roll,
			"hand.left.palm.normal.z" => self.max_left_palm_normal_z,
			"hand.right.palm.normal.z" => self.max_right_palm_normal_z,
			"LeftHand.palm.z" => self.max_left_hand_palm_z,
			"RightHand.palm.z" => self.max_right_hand_palm_z,
			_ => None,
		}
	}
}

fn run_pose_fixture_probe(repo: &Path, probe: &Path, image: &Path, pass: &[OsString]) -> Result<serde_json::Value> {
	let mut cmd = Command::new(probe);
	cmd.current_dir(repo)
		.arg("--image")
		.arg(image)
		.arg("--running-mode")
		.arg("video")
		.arg("--holistic")
		.args(pass)
		.stdout(Stdio::piped())
		.stderr(Stdio::inherit());
	let output = cmd
		.output()
		.with_context(|| format!("failed to run pose fixture probe for {}", image.display()))?;
	if !output.status.success() {
		bail!("pose fixture probe failed for {} with {}", image.display(), output.status);
	}
	serde_json::from_slice(&output.stdout).with_context(|| format!("decode pose fixture probe JSON for {}", image.display()))
}

fn run_pose_fixture_probe_with_head_source(
	repo: &Path,
	probe: &Path,
	image: &Path,
	head_source: &str,
	pass: &[OsString],
) -> Result<serde_json::Value> {
	let mut source_pass = Vec::with_capacity(pass.len() + 2);
	source_pass.push(OsString::from("--head-source"));
	source_pass.push(OsString::from(head_source));
	source_pass.extend(pass_without_head_source(pass));
	run_pose_fixture_probe(repo, probe, image, &source_pass)
}

fn pass_without_head_source(pass: &[OsString]) -> Vec<OsString> {
	let mut out = Vec::with_capacity(pass.len());
	let mut iter = pass.iter();
	while let Some(arg) = iter.next() {
		let text = arg.to_string_lossy();
		if text == "--head-source" {
			let _ = iter.next();
			continue;
		}
		if text.starts_with("--head-source=") {
			continue;
		}
		out.push(arg.clone());
	}
	out
}

fn print_pose_fixture_head_diagnostics(repo: &Path, probe: &Path, image: &Path, pass: &[OsString]) -> Result<()> {
	for source in ["all", "face", "pose"] {
		let report = run_pose_fixture_probe_with_head_source(repo, probe, image, source, pass)?;
		let head_yaw = signal_value_from_probe_report(&report, "head.yaw");
		let head_pitch = signal_value_from_probe_report(&report, "head.pitch");
		let head_roll = signal_value_from_probe_report(&report, "head.roll");
		let head_front = head_front_from_probe_report(&report);
		let quality = probe_report_quality_note(&report);
		println!(
			"  source={source:<4} yaw={} pitch={} roll={} front={} {}",
			format_optional_f64(head_yaw),
			format_optional_f64(head_pitch),
			format_optional_f64(head_roll),
			format_optional_vec3(head_front),
			quality.unwrap_or("")
		);
	}
	Ok(())
}

fn signal_value_from_probe_report(report: &serde_json::Value, name: &str) -> Option<f64> {
	report
		.get("signals")?
		.as_array()?
		.iter()
		.find(|signal| signal.get("name").and_then(|value| value.as_str()) == Some(name))?
		.get("value")?
		.as_f64()
}

fn head_front_from_probe_report(report: &serde_json::Value) -> Option<[f64; 3]> {
	let rotation = head_rotation_from_probe_report(report)?;
	Some(quat_rotate_vec3(rotation, [0.0, 0.0, 1.0]))
}

fn head_rotation_from_probe_report(report: &serde_json::Value) -> Option<[f64; 4]> {
	bone_rotation_from_probe_report(report, "Head")
}

fn hand_palm_front_from_probe_report(report: &serde_json::Value, bone_name: &str) -> Option<[f64; 3]> {
	let side = bone_name.strip_suffix("Hand")?;
	let upper = bone_rotation_from_probe_report(report, &format!("{side}UpperArm"))?;
	let lower = bone_rotation_from_probe_report(report, &format!("{side}LowerArm"))?;
	let hand = bone_rotation_from_probe_report(report, bone_name)?;
	let rotation = quat_mul_f64(quat_mul_f64(upper, lower), hand);
	Some(quat_rotate_vec3(rotation, [0.0, -1.0, 0.0]))
}

fn bone_rotation_from_probe_report(report: &serde_json::Value, bone_name: &str) -> Option<[f64; 4]> {
	let bones = report.get("body")?.get("bones")?.as_array()?;
	let bone = bones
		.iter()
		.find(|bone| bone.get("bone").and_then(|value| value.as_str()) == Some(bone_name))?;
	let rotation = bone.get("rotation")?;
	Some([
		rotation.get("x")?.as_f64()?,
		rotation.get("y")?.as_f64()?,
		rotation.get("z")?.as_f64()?,
		rotation.get("w")?.as_f64()?,
	])
}

fn quat_rotate_vec3(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
	let q = normalize_quat_f64(q);
	let u = [q[0], q[1], q[2]];
	let s = q[3];
	let uv = cross3(u, v);
	let uuv = cross3(u, uv);
	[
		v[0] + 2.0 * (s * uv[0] + uuv[0]),
		v[1] + 2.0 * (s * uv[1] + uuv[1]),
		v[2] + 2.0 * (s * uv[2] + uuv[2]),
	]
}

fn quat_mul_f64(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
	let [ax, ay, az, aw] = normalize_quat_f64(a);
	let [bx, by, bz, bw] = normalize_quat_f64(b);
	normalize_quat_f64([
		(aw * bx) + (ax * bw) + (ay * bz) - (az * by),
		(aw * by) - (ax * bz) + (ay * bw) + (az * bx),
		(aw * bz) + (ax * by) - (ay * bx) + (az * bw),
		(aw * bw) - (ax * bx) - (ay * by) - (az * bz),
	])
}

fn normalize_quat_f64(q: [f64; 4]) -> [f64; 4] {
	let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
	if len <= f64::EPSILON {
		return [0.0, 0.0, 0.0, 1.0];
	}
	[q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
	[a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn probe_report_quality_note(report: &serde_json::Value) -> Option<&str> {
	report
		.get("notes")?
		.as_array()?
		.iter()
		.filter_map(|note| note.as_str())
		.find(|note| note.starts_with("mediapipe.quality head="))
}

#[derive(Debug, Deserialize)]
struct MediaPipePinFile {
	mediapipe: MediaPipeRepoPin,
	model: BTreeMap<String, MediaPipeModelPin>,
	build: MediaPipeBuildPin,
}

#[derive(Debug, Deserialize)]
struct MediaPipeRepoPin {
	repository: String,
	tag: String,
	commit: String,
	#[allow(dead_code)]
	license: String,
}

#[derive(Debug, Deserialize)]
struct MediaPipeModelPin {
	url: String,
	path: PathBuf,
	#[allow(dead_code)]
	license: String,
}

#[derive(Debug, Deserialize)]
struct MediaPipeBuildPin {
	bazelisk: BazeliskPin,
	opencv: Option<OpenCvPin>,
}

#[derive(Debug, Deserialize)]
struct BazeliskPin {
	#[allow(dead_code)]
	version: String,
	windows_amd64_url: String,
	path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct OpenCvPin {
	#[allow(dead_code)]
	version: String,
	world_version: String,
	windows_amd64_url: String,
	installer_path: PathBuf,
	extract_dir: PathBuf,
	build_path: PathBuf,
}

#[derive(Debug)]
struct PreparedOpenCv {
	build_path: PathBuf,
	bin_path: PathBuf,
	runtime_dll: PathBuf,
}

#[derive(Debug)]
struct BuildNativeMediaPipeArgs {
	pin_file: PathBuf,
	media_pipe_root: PathBuf,
	out: PathBuf,
	jobs: Option<usize>,
	skip_fetch: bool,
}

fn build_native_mediapipe(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_build_native_mediapipe_args(raw_args)?;
	let pin_path = absolutize(repo, &args.pin_file);
	let pin: MediaPipePinFile =
		toml::from_str(&fs::read_to_string(&pin_path).with_context(|| format!("failed to read {}", pin_path.display()))?)
			.with_context(|| format!("failed to parse {}", pin_path.display()))?;

	let media_pipe_root = absolutize(repo, &args.media_pipe_root);
	let out = absolutize(repo, &args.out);
	let bazelisk = absolutize(repo, &pin.build.bazelisk.path);
	let jobs = args.jobs.unwrap_or_else(default_heavy_jobs);
	let opencv = if cfg!(windows) {
		Some(ensure_opencv(repo, pin.build.opencv.as_ref(), args.skip_fetch)?)
	} else {
		None
	};

	if !args.skip_fetch {
		ensure_git_checkout(&media_pipe_root, &pin.mediapipe)?;
		ensure_bazelisk(&bazelisk, &pin.build.bazelisk)?;
		for (name, model) in &pin.model {
			ensure_download(repo, name, &model.url, &absolutize(repo, &model.path))?;
		}
	}

	copy_native_bridge(repo, &media_pipe_root)?;
	patch_mediapipe_sources(repo, &media_pipe_root, opencv.as_ref().map(|opencv| opencv.build_path.as_path()))?;
	let python_env = prepare_bazel_python(repo)?;
	let halide_runtime = find_halide_runtime()?;
	let built = build_mediapipe_dll(
		repo,
		&media_pipe_root,
		&bazelisk,
		&python_env,
		halide_runtime.as_deref(),
		opencv.as_ref().map(|opencv| opencv.bin_path.as_path()),
		jobs,
	)?;
	if let Some(parent) = out.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	copy_replacing_file(&built, &out)?;
	if let (Some(parent), Some(opencv)) = (out.parent(), opencv.as_ref()) {
		copy_replacing_file(
			&opencv.runtime_dll,
			&parent.join(opencv.runtime_dll.file_name().context("OpenCV runtime dll has no file name")?),
		)?;
	}
	eprintln!("wrote {}", out.display());
	Ok(())
}

fn parse_build_native_mediapipe_args(raw_args: Vec<OsString>) -> Result<BuildNativeMediaPipeArgs> {
	let mut args = BuildNativeMediaPipeArgs {
		pin_file: PathBuf::from("native/mediapipe/mediapipe-pin.toml"),
		media_pipe_root: PathBuf::from("third_party/mediapipe"),
		out: PathBuf::from("native/mediapipe/un-motion-mediapipe.dll"),
		jobs: None,
		skip_fetch: false,
	};
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--pin-file" => args.pin_file = PathBuf::from(next_value(&mut iter, "--pin-file")?),
			"--media-pipe-root" => args.media_pipe_root = PathBuf::from(next_value(&mut iter, "--media-pipe-root")?),
			"--out" => args.out = PathBuf::from(next_value(&mut iter, "--out")?),
			"--jobs" => args.jobs = Some(parse_usize(next_value(&mut iter, "--jobs")?, "--jobs")?.max(1)),
			"--skip-fetch" => args.skip_fetch = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask mediapipe build-native [--pin-file native/mediapipe/mediapipe-pin.toml] [--media-pipe-root third_party/mediapipe] [--out native/mediapipe/un-motion-mediapipe.dll] [--jobs N] [--skip-fetch]"
				);
				std::process::exit(0);
			}
			other => bail!("unexpected build-native argument: {other}"),
		}
	}
	Ok(args)
}

fn ensure_git_checkout(destination: &Path, pin: &MediaPipeRepoPin) -> Result<()> {
	if destination.join(".git").exists() {
		let mut fetch = Command::new("git");
		fetch.current_dir(destination).args(["fetch", "--tags", "--prune"]);
		run_command("git fetch MediaPipe", &mut fetch)?;
	} else {
		if let Some(parent) = destination.parent() {
			fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
		}
		let mut clone = Command::new("git");
		clone.args(["clone", &pin.repository, &destination.display().to_string()]);
		run_command("git clone MediaPipe", &mut clone)?;
	}

	let rev = if pin.commit.trim().is_empty() {
		pin.tag.as_str()
	} else {
		pin.commit.as_str()
	};
	let mut checkout = Command::new("git");
	checkout.current_dir(destination).args(["checkout", rev]);
	run_command(&format!("git checkout {rev}"), &mut checkout)
}

fn ensure_bazelisk(path: &Path, pin: &BazeliskPin) -> Result<()> {
	if path.exists() {
		let mut version = Command::new(path);
		version.arg("--version");
		if run_command("bazelisk --version", &mut version).is_ok() {
			return Ok(());
		}
		fs::remove_file(path).with_context(|| format!("failed to remove corrupt {}", path.display()))?;
	}
	ensure_download(Path::new("."), "bazelisk", &pin.windows_amd64_url, path)?;
	let mut version = Command::new(path);
	version.arg("--version");
	run_command("bazelisk --version", &mut version)
}

fn default_heavy_jobs() -> usize {
	let cores = physical_core_count().unwrap_or_else(|| std::thread::available_parallelism().map(usize::from).unwrap_or(2));
	(cores / 2).max(1)
}

fn physical_core_count() -> Option<usize> {
	#[cfg(windows)]
	{
		let output = Command::new("powershell.exe")
			.args([
				"-NoProfile",
				"-Command",
				"(Get-CimInstance Win32_Processor | Measure-Object -Property NumberOfCores -Sum).Sum",
			])
			.output()
			.ok()?;
		if !output.status.success() {
			return None;
		}
		String::from_utf8_lossy(&output.stdout)
			.lines()
			.map(str::trim)
			.find(|line| !line.is_empty())
			.and_then(|line| line.parse::<usize>().ok())
			.filter(|cores| *cores > 0)
	}

	#[cfg(not(windows))]
	{
		None
	}
}

fn ensure_opencv(repo: &Path, pin: Option<&OpenCvPin>, skip_fetch: bool) -> Result<PreparedOpenCv> {
	let Some(pin) = pin else {
		bail!("Windows native MediaPipe build now requires [build.opencv] in native/mediapipe/mediapipe-pin.toml")
	};
	let installer = absolutize(repo, &pin.installer_path);
	let extract_dir = absolutize(repo, &pin.extract_dir);
	let build_path = absolutize(repo, &pin.build_path);
	let lib_path = build_path
		.join("x64")
		.join("vc15")
		.join("lib")
		.join(format!("opencv_world{}.lib", pin.world_version));
	let bin_path = build_path.join("x64").join("vc15").join("bin");
	let runtime_dll = bin_path.join(format!("opencv_world{}.dll", pin.world_version));

	if !lib_path.exists() || !runtime_dll.exists() {
		if skip_fetch {
			bail!("OpenCV is missing under {}; rerun without --skip-fetch", build_path.display());
		}
		ensure_download(repo, "opencv", &pin.windows_amd64_url, &installer)?;
		fs::create_dir_all(&extract_dir).with_context(|| format!("failed to create {}", extract_dir.display()))?;
		let mut extract = Command::new(&installer);
		extract.current_dir(repo).arg(format!("-o{}", extract_dir.display())).arg("-y");
		run_command("extract OpenCV", &mut extract)?;
	}

	if !lib_path.exists() {
		bail!("OpenCV import library was not found at {}", lib_path.display());
	}
	if !runtime_dll.exists() {
		bail!("OpenCV runtime DLL was not found at {}", runtime_dll.display());
	}
	Ok(PreparedOpenCv {
		build_path,
		bin_path,
		runtime_dll,
	})
}

fn ensure_download(repo: &Path, label: &str, url: &str, out: &Path) -> Result<()> {
	if out.exists() {
		return Ok(());
	}
	if let Some(parent) = out.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	let mut cmd = Command::new(resolve_tool("curl"));
	cmd.current_dir(repo).args(["-L", "--fail", "--retry", "3", "-o"]);
	cmd.arg(out);
	cmd.arg(url);
	run_command(&format!("download {label}"), &mut cmd)
}

fn copy_replacing_file(source: &Path, destination: &Path) -> Result<()> {
	if destination.exists() {
		let metadata = fs::metadata(destination).with_context(|| format!("failed to inspect {}", destination.display()))?;
		let mut permissions = metadata.permissions();
		if permissions.readonly() {
			permissions.set_readonly(false);
			fs::set_permissions(destination, permissions).with_context(|| format!("failed to make {} writable", destination.display()))?;
		}
	}
	fs::copy(source, destination).with_context(|| format!("failed to copy {} to {}", source.display(), destination.display()))?;
	Ok(())
}

fn copy_native_bridge(repo: &Path, media_pipe_root: &Path) -> Result<()> {
	let target = media_pipe_root.join("un-motion");
	fs::create_dir_all(&target).with_context(|| format!("failed to create {}", target.display()))?;
	for file in ["BUILD.bazel", "un-motion-mediapipe-backend.cc", "un-motion-mediapipe-ffi.h"] {
		let source = repo.join("native/mediapipe").join(file);
		let dest = target.join(file);
		fs::copy(&source, &dest).with_context(|| format!("failed to copy {} to {}", source.display(), dest.display()))?;
	}
	Ok(())
}

fn patch_mediapipe_sources(repo: &Path, media_pipe_root: &Path, opencv_build_path: Option<&Path>) -> Result<()> {
	let workspace = media_pipe_root.join("WORKSPACE");
	let task_runner = media_pipe_root.join("mediapipe/tasks/cc/core/task_runner.cc");
	let task_runner_build = media_pipe_root.join("mediapipe/tasks/cc/core/BUILD");
	let base_options_header = media_pipe_root.join("mediapipe/tasks/cc/core/base_options.h");
	let base_options_cc = media_pipe_root.join("mediapipe/tasks/cc/core/base_options.cc");
	let dummy_logger = media_pipe_root.join("mediapipe/tasks/cc/core/logging/tasks_dummy_logger.h");
	let logging_build = media_pipe_root.join("mediapipe/tasks/cc/core/logging/BUILD");
	let build_config = media_pipe_root.join("mediapipe/framework/port/build_config.bzl");
	let profiler_resource_util = media_pipe_root.join("mediapipe/framework/profiler/profiler_resource_util_common.cc");
	let tflite_signature_reader = media_pipe_root.join("mediapipe/util/tflite/tflite_signature_reader.cc");
	let rectangle_util = media_pipe_root.join("mediapipe/util/rectangle_util.cc");
	let model_asset_bundle_resources = media_pipe_root.join("mediapipe/tasks/cc/core/model_asset_bundle_resources.cc");
	let packet_generator_wrapper = media_pipe_root.join("mediapipe/framework/tool/packet_generator_wrapper_calculator.cc");
	let zip_utils = media_pipe_root.join("mediapipe/tasks/cc/metadata/utils/zip_utils.cc");
	let status_macros = media_pipe_root.join("mediapipe/framework/deps/status_macros.h");
	let gpu_service = media_pipe_root.join("mediapipe/gpu/gpu_service.cc");
	let legacy_calculator_support = media_pipe_root.join("mediapipe/framework/legacy_calculator_support.cc");
	let api3_calculator_context = media_pipe_root.join("mediapipe/framework/api3/calculator_context.h");
	let api3_graph = media_pipe_root.join("mediapipe/framework/api3/graph.h");
	let halide_bzl = media_pipe_root.join("third_party/halide/halide.bzl");
	let pose_detector_graph = media_pipe_root.join("mediapipe/tasks/cc/vision/pose_detector/pose_detector_graph.cc");
	let holistic_landmarker_header = media_pipe_root.join("mediapipe/tasks/cc/vision/holistic_landmarker/holistic_landmarker.h");
	let holistic_landmarker_cc = media_pipe_root.join("mediapipe/tasks/cc/vision/holistic_landmarker/holistic_landmarker.cc");
	let image_to_tensor_frame_buffer = media_pipe_root.join("mediapipe/calculators/tensor/image_to_tensor_converter_frame_buffer.cc");

	for path in [
		&workspace,
		&task_runner,
		&task_runner_build,
		&base_options_header,
		&base_options_cc,
		&dummy_logger,
		&logging_build,
		&build_config,
		&profiler_resource_util,
		&tflite_signature_reader,
		&rectangle_util,
		&model_asset_bundle_resources,
		&packet_generator_wrapper,
		&zip_utils,
		&status_macros,
		&gpu_service,
		&legacy_calculator_support,
		&api3_calculator_context,
		&api3_graph,
		&halide_bzl,
		&pose_detector_graph,
		&holistic_landmarker_header,
		&holistic_landmarker_cc,
		&image_to_tensor_frame_buffer,
	] {
		if !path.exists() {
			bail!("MediaPipe patch target not found: {}", path.display());
		}
	}

	if let Some(opencv_build_path) = opencv_build_path {
		let bazel_path = bazel_path_string(opencv_build_path);
		edit_text_file(&workspace, |text| patch_windows_opencv_repository_path(&text, &bazel_path))?;
	}

	edit_text_file(&task_runner, |text| {
		text.replace("#include \"mediapipe/util/analytics/mediapipe_logging_enums.pb.h\"\n", "")
	})?;
	edit_text_file(&task_runner_build, |text| {
		text.replace("        \"//mediapipe/util/analytics:mediapipe_logging_enums_cc_proto\",\n", "")
	})?;
	edit_text_file(&base_options_header, |text| {
		text.replace(
			"  struct CpuOptions {};",
			"  struct CpuOptions {\n    bool use_xnnpack = false;\n    int xnnpack_num_threads = -1;\n  };",
		)
	})?;
	edit_text_file(&base_options_cc, |text| {
		text.replace(
			"  acceleration_proto.mutable_tflite();\n  return acceleration_proto;",
			"  if (options.use_xnnpack) {\n    auto* xnnpack = acceleration_proto.mutable_xnnpack();\n    if (options.xnnpack_num_threads > 0) {\n      xnnpack->set_num_threads(options.xnnpack_num_threads);\n    }\n  } else {\n    acceleration_proto.mutable_tflite();\n  }\n  return acceleration_proto;",
		)
	})?;
	edit_text_file(&dummy_logger, |text| {
		text.replace("#include \"mediapipe/tasks/cc/core/logging/logging_client.h\"\n", "")
	})?;
	edit_text_file(&logging_build, |text| text.replace("        \":logging_client\",\n", ""))?;

	edit_text_file(&build_config, |mut text| {
		text = text.replace("load(\"@npm//@bazel/typescript:index.bzl\", \"ts_project\")\n", "");
		if !text.contains("UNMotion C++ build TS stub") {
			let stub = r#"
def ts_project(**kwargs):
    # UNMotion C++ build TS stub: avoid fetching npm for native-only builds.
    native.filegroup(
        name = kwargs["name"],
        srcs = kwargs.get("srcs", []),
        visibility = kwargs.get("visibility", None),
        testonly = kwargs.get("testonly", 0),
    )

"#;
			text = text.replace(
				"load(\n    \"//mediapipe/framework/tool:mediapipe_proto.bzl\",\n    _mediapipe_cc_proto_library = \"mediapipe_cc_proto_library\",\n    _mediapipe_proto_library = \"mediapipe_proto_library\",\n)\n",
				&format!(
					"load(\n    \"//mediapipe/framework/tool:mediapipe_proto.bzl\",\n    _mediapipe_cc_proto_library = \"mediapipe_cc_proto_library\",\n    _mediapipe_proto_library = \"mediapipe_proto_library\",\n)\n{stub}"
				),
			);
		}
		text
	})?;

	edit_text_file(&profiler_resource_util, |text| {
		text.replace(
			"  MP_ASSIGN_OR_RETURN(std::string log_dir, GetLogDirectory());",
			"  auto log_dir_or = GetLogDirectory();\n  MP_RETURN_IF_ERROR(log_dir_or.status());\n  std::string log_dir = *log_dir_or;",
		)
		.replace(
			"  MP_ASSIGN_OR_RETURN(auto log_dir, GetLogDirectory());",
			"  auto log_dir_or = GetLogDirectory();\n  MP_RETURN_IF_ERROR(log_dir_or.status());\n  std::string log_dir = *log_dir_or;",
		)
	})?;
	edit_text_file(&tflite_signature_reader, |text| {
		text.replace(
			"    MP_ASSIGN_OR_RETURN(\n        SignatureInputOutputTensorNames input_output_tensor_names,\n        GetInputOutputTensorNamesFromTfliteSignature(interpreter,\n                                                     signature_key));",
			"    auto input_output_tensor_names_or =\n        GetInputOutputTensorNamesFromTfliteSignature(interpreter, signature_key);\n    MP_RETURN_IF_ERROR(input_output_tensor_names_or.status());\n    SignatureInputOutputTensorNames input_output_tensor_names =\n        std::move(*input_output_tensor_names_or);",
		)
	})?;
	edit_text_file(&rectangle_util, |text| {
		text.replace(
			"  MP_ASSIGN_OR_RETURN(Rectangle_f new_rectangle, ToRectangle(new_rect));",
			"  auto new_rectangle_or = ToRectangle(new_rect);\n  MP_RETURN_IF_ERROR(new_rectangle_or.status());\n  Rectangle_f new_rectangle = *new_rectangle_or;",
		)
		.replace(
			"    MP_ASSIGN_OR_RETURN(Rectangle_f existing_rectangle,\n                        ToRectangle(existing_rect));",
			"    auto existing_rectangle_or = ToRectangle(existing_rect);\n    MP_RETURN_IF_ERROR(existing_rectangle_or.status());\n    Rectangle_f existing_rectangle = *existing_rectangle_or;",
		)
	})?;
	edit_text_file(&model_asset_bundle_resources, |text| {
		text.replace(
			"    MP_ASSIGN_OR_RETURN(\n        std::string path_to_resource,\n        mediapipe::PathToResourceAsFile(model_asset_bundle_file_->file_name()));",
			"    auto path_to_resource_or =\n        mediapipe::PathToResourceAsFile(model_asset_bundle_file_->file_name());\n    MP_RETURN_IF_ERROR(path_to_resource_or.status());\n    std::string path_to_resource = *path_to_resource_or;",
		)
		.replace(
			"  MP_ASSIGN_OR_RETURN(model_asset_bundle_file_handler_,\n                      ExternalFileHandler::CreateFromExternalFile(\n                          model_asset_bundle_file_.get()));",
			"  auto model_asset_bundle_file_handler_or =\n      ExternalFileHandler::CreateFromExternalFile(model_asset_bundle_file_.get());\n  MP_RETURN_IF_ERROR(model_asset_bundle_file_handler_or.status());\n  model_asset_bundle_file_handler_ = std::move(*model_asset_bundle_file_handler_or);",
		)
	})?;
	edit_text_file(&packet_generator_wrapper, |text| {
		text.replace(
			"  MP_ASSIGN_OR_RETURN(auto static_access,\n                      mediapipe::internal::StaticAccessToGeneratorRegistry::\n                          CreateByNameInNamespace(options.package(),\n                                                  options.packet_generator()));",
			"  auto static_access_or =\n      mediapipe::internal::StaticAccessToGeneratorRegistry::\n          CreateByNameInNamespace(options.package(), options.packet_generator());\n  MP_RETURN_IF_ERROR(static_access_or.status());\n  auto static_access = std::move(*static_access_or);",
		)
	})?;
	edit_text_file(&zip_utils, |text| {
		text.replace(
			"      MP_ASSIGN_OR_RETURN(auto zip_file_info, GetCurrentZipFileInfo(zf));",
			"      auto zip_file_info_or = GetCurrentZipFileInfo(zf);\n      MP_RETURN_IF_ERROR(zip_file_info_or.status());\n      auto zip_file_info = *zip_file_info_or;",
		)
	})?;
	edit_text_file(&status_macros, |text| {
		text.replace(
			"MP_STATUS_MACROS_IMPL_UNPARENTHESIZE_IF_PARENTHESIZED(lhs) =           \\\n      std::move(statusor).value()",
			"lhs = std::move(statusor).value()",
		)
		.replace(
			"MP_STATUS_MACROS_IMPL_UNPARENTHESIZE_IF_PARENTHESIZED(lhs) = \\\n      std::move(statusor).value()",
			"lhs = std::move(statusor).value()",
		)
	})?;
	edit_text_file(&gpu_service, |text| {
		text.replace(
			"const GraphService<GpuResources> kGpuService(",
			"ABSL_CONST_INIT const GraphService<GpuResources> kGpuService(",
		)
	})?;
	edit_text_file(&legacy_calculator_support, |text| {
		text.replace(
			"thread_local CalculatorContext*\n    LegacyCalculatorSupport::Scoped<CalculatorContext>::current_ = nullptr;",
			"ABSL_CONST_INIT thread_local CalculatorContext*\n    LegacyCalculatorSupport::Scoped<CalculatorContext>::current_ = nullptr;",
		)
		.replace(
			"thread_local CalculatorContract*\n    LegacyCalculatorSupport::Scoped<CalculatorContract>::current_ = nullptr;",
			"ABSL_CONST_INIT thread_local CalculatorContract*\n    LegacyCalculatorSupport::Scoped<CalculatorContract>::current_ = nullptr;",
		)
	})?;
	edit_text_file(&api3_calculator_context, |text| {
		text.replace(
			"template <typename T, int&... DoNotSpecify, typename F>",
			"template <typename T, typename F>",
		)
		.replace(
			"template <typename T, typename U, typename... Rest, int&... DoNotSpecify,\n          typename F>",
			"template <typename T, typename U, typename... Rest, typename F>",
		)
	})?;
	edit_text_file(&api3_graph, |mut text| {
		if !text.contains("template <typename NodeT>\nclass SubgraphContext;") {
			text = text.replace(
				"class GenericGraph;",
				"class GenericGraph;\ntemplate <typename NodeT>\nclass SubgraphContext;",
			);
		}
		text
	})?;
	edit_text_file(&halide_bzl, |mut text| {
		text = text.replace(
			"    return _common_opts + select({\n        \"//conditions:default\": _posix_opts,\n        \"@mediapipe//mediapipe:windows\": _msvc_opts,\n    })",
			"    return select({\n        \"//conditions:default\": _common_opts + _posix_opts,\n        \"@mediapipe//mediapipe:windows\": _msvc_opts,\n    })",
		);
		if !text.contains("HALIDE_RUNTIME_PATH") {
			text = text.replace(
				"        \"HL_LLVM_ARGS\": str(ctx.var.get(\"halide_llvm_args\", \"\")),",
				"        \"HL_LLVM_ARGS\": str(ctx.var.get(\"halide_llvm_args\", \"\")),\n        \"PATH\": str(ctx.var.get(\"HALIDE_RUNTIME_PATH\", \"\")),",
			);
		}
		text
	})?;
	edit_text_file(&pose_detector_graph, |text| {
		text.replace(
			"image_to_tensor_options.set_border_mode(\n        mediapipe::ImageToTensorCalculatorOptions::BORDER_REPLICATE);",
			"image_to_tensor_options.set_border_mode(\n        mediapipe::ImageToTensorCalculatorOptions::BORDER_ZERO);",
		)
	})?;
	edit_text_file(&holistic_landmarker_header, |mut text| {
		if !text.contains("output_face_landmarks") {
			text = text.replace(
				"  // Whether to output face blendshapes classification. Face blendshapes are\n  // used for rendering animations of the face.\n  bool output_face_blendshapes = false;\n\n  // Whether to output segmentation masks.",
				"  // Whether to output face blendshapes classification. Face blendshapes are\n  // used for rendering animations of the face.\n  bool output_face_blendshapes = false;\n\n  // Whether to output face landmarks.\n  bool output_face_landmarks = true;\n\n  // Whether to output hand landmarks.\n  bool output_hand_landmarks = true;\n\n  // Whether to output hand world landmarks.\n  bool output_hand_world_landmarks = true;\n\n  // Whether to output pose landmarks.\n  bool output_pose_landmarks = true;\n\n  // Whether to output pose world landmarks.\n  bool output_pose_world_landmarks = true;\n\n  // Whether to output segmentation masks.",
			);
		}
		if !text.contains("flow_limiter_enabled") {
			text = text.replace(
				"  // Whether to output segmentation masks.\n  bool output_pose_segmentation_masks = false;\n\n  // The user-defined result callback",
				"  // Whether to output segmentation masks.\n  bool output_pose_segmentation_masks = false;\n\n  // Whether to add FlowLimiterCalculator in live stream mode.\n  bool flow_limiter_enabled = true;\n\n  int flow_limiter_max_in_flight = 1;\n\n  int flow_limiter_max_in_queue = 1;\n\n  // The user-defined result callback",
			);
		}
		text
	})?;
	edit_text_file(&holistic_landmarker_cc, |mut text| {
		text = text
			.replace(
				"  if (!packets.at(kFaceLandmarksStreamName).IsEmpty()) {",
				"  if (packets.count(kFaceLandmarksStreamName) &&\n      !packets.at(kFaceLandmarksStreamName).IsEmpty()) {",
			)
			.replace(
				"  if (!packets.at(kPoseLandmarksStreamName).IsEmpty()) {",
				"  if (packets.count(kPoseLandmarksStreamName) &&\n      !packets.at(kPoseLandmarksStreamName).IsEmpty()) {",
			)
			.replace(
				"  if (!packets.at(kPoseWorldLandmarksStreamName).IsEmpty()) {",
				"  if (packets.count(kPoseWorldLandmarksStreamName) &&\n      !packets.at(kPoseWorldLandmarksStreamName).IsEmpty()) {",
			)
			.replace(
				"  if (!packets.at(kLeftHandLandmarksStreamName).IsEmpty()) {",
				"  if (packets.count(kLeftHandLandmarksStreamName) &&\n      !packets.at(kLeftHandLandmarksStreamName).IsEmpty()) {",
			)
			.replace(
				"  if (!packets.at(kRightHandLandmarksStreamName).IsEmpty()) {",
				"  if (packets.count(kRightHandLandmarksStreamName) &&\n      !packets.at(kRightHandLandmarksStreamName).IsEmpty()) {",
			)
			.replace(
				"  if (!packets.at(kLeftHandWorldLandmarksStreamName).IsEmpty()) {",
				"  if (packets.count(kLeftHandWorldLandmarksStreamName) &&\n      !packets.at(kLeftHandWorldLandmarksStreamName).IsEmpty()) {",
			)
			.replace(
				"  if (!packets.at(kRightHandWorldLandmarksStreamName).IsEmpty()) {",
				"  if (packets.count(kRightHandWorldLandmarksStreamName) &&\n      !packets.at(kRightHandWorldLandmarksStreamName).IsEmpty()) {",
			);
		text = text.replace(
			"CalculatorGraphConfig CreateGraphConfig(\n    std::unique_ptr<HolisticLandmarkerGraphOptionsProto> options,\n    bool enable_flow_limiting) {",
			"CalculatorGraphConfig CreateGraphConfig(\n    std::unique_ptr<HolisticLandmarkerGraphOptionsProto> options,\n    bool enable_flow_limiting, bool output_face_landmarks,\n    bool output_pose_landmarks, bool output_pose_world_landmarks,\n    bool output_hand_landmarks, bool output_hand_world_landmarks,\n    bool output_pose_segmentation_masks, bool output_face_blendshapes,\n    int flow_limiter_max_in_flight, int flow_limiter_max_in_queue) {",
		);
		text = text.replace(
			"    bool output_hand_landmarks, bool output_hand_world_landmarks,\n    bool output_pose_segmentation_masks, bool output_face_blendshapes) {",
			"    bool output_hand_landmarks, bool output_hand_world_landmarks,\n    bool output_pose_segmentation_masks, bool output_face_blendshapes,\n    int flow_limiter_max_in_flight, int flow_limiter_max_in_queue) {",
		);
		text = text.replace(
			"  graph.In(kImageTag).SetName(kImageInStreamName);\n  subgraph.Out(kFaceLandmarksTag).SetName(kFaceLandmarksStreamName) >>\n      graph.Out(kFaceLandmarksTag);\n  subgraph.Out(kPoseLandmarksTag).SetName(kPoseLandmarksStreamName) >>\n      graph.Out(kPoseLandmarksTag);\n  subgraph.Out(kPoseWorldLandmarksTag).SetName(kPoseWorldLandmarksStreamName) >>\n      graph.Out(kPoseWorldLandmarksTag);\n  subgraph.Out(kLeftHandLandmarksTag).SetName(kLeftHandLandmarksStreamName) >>\n      graph.Out(kLeftHandLandmarksTag);\n  subgraph.Out(kRightHandLandmarksTag).SetName(kRightHandLandmarksStreamName) >>\n      graph.Out(kRightHandLandmarksTag);\n  subgraph.Out(kLeftHandWorldLandmarksTag)\n          .SetName(kLeftHandWorldLandmarksStreamName) >>\n      graph.Out(kLeftHandWorldLandmarksTag);\n  subgraph.Out(kRightHandWorldLandmarksTag)\n          .SetName(kRightHandWorldLandmarksStreamName) >>\n      graph.Out(kRightHandWorldLandmarksTag);\n  subgraph.Out(kPoseSegmentationMaskTag)\n          .SetName(kPoseSegmentationMaskStreamName) >>\n      graph.Out(kPoseSegmentationMaskTag);\n  subgraph.Out(kFaceBlendshapesTag).SetName(kFaceBlendshapesStreamName) >>\n      graph.Out(kFaceBlendshapesTag);",
			"  graph.In(kImageTag).SetName(kImageInStreamName);\n  if (output_face_landmarks) {\n    subgraph.Out(kFaceLandmarksTag).SetName(kFaceLandmarksStreamName) >>\n        graph.Out(kFaceLandmarksTag);\n  }\n  if (output_pose_landmarks) {\n    subgraph.Out(kPoseLandmarksTag).SetName(kPoseLandmarksStreamName) >>\n        graph.Out(kPoseLandmarksTag);\n  }\n  if (output_pose_world_landmarks) {\n    subgraph.Out(kPoseWorldLandmarksTag)\n            .SetName(kPoseWorldLandmarksStreamName) >>\n        graph.Out(kPoseWorldLandmarksTag);\n  }\n  if (output_hand_landmarks) {\n    subgraph.Out(kLeftHandLandmarksTag).SetName(kLeftHandLandmarksStreamName) >>\n        graph.Out(kLeftHandLandmarksTag);\n    subgraph.Out(kRightHandLandmarksTag)\n            .SetName(kRightHandLandmarksStreamName) >>\n        graph.Out(kRightHandLandmarksTag);\n  }\n  if (output_hand_world_landmarks) {\n    subgraph.Out(kLeftHandWorldLandmarksTag)\n            .SetName(kLeftHandWorldLandmarksStreamName) >>\n        graph.Out(kLeftHandWorldLandmarksTag);\n    subgraph.Out(kRightHandWorldLandmarksTag)\n            .SetName(kRightHandWorldLandmarksStreamName) >>\n        graph.Out(kRightHandWorldLandmarksTag);\n  }\n  if (output_pose_segmentation_masks) {\n    subgraph.Out(kPoseSegmentationMaskTag)\n            .SetName(kPoseSegmentationMaskStreamName) >>\n        graph.Out(kPoseSegmentationMaskTag);\n  }\n  if (output_face_blendshapes) {\n    subgraph.Out(kFaceBlendshapesTag).SetName(kFaceBlendshapesStreamName) >>\n        graph.Out(kFaceBlendshapesTag);\n  }",
		);
		text = text.replace(
			"          {.config = CreateGraphConfig(\n               std::move(options_proto),\n               options->running_mode == core::RunningMode::LIVE_STREAM),",
			"          {.config = CreateGraphConfig(\n               std::move(options_proto),\n               options->running_mode == core::RunningMode::LIVE_STREAM &&\n                   options->flow_limiter_enabled,\n               options->output_face_landmarks, options->output_pose_landmarks,\n               options->output_pose_world_landmarks,\n               options->output_hand_landmarks,\n               options->output_hand_world_landmarks,\n               options->output_pose_segmentation_masks,\n               options->output_face_blendshapes,\n               options->flow_limiter_max_in_flight,\n               options->flow_limiter_max_in_queue),",
		);
		text = text.replace(
			"options->running_mode == core::RunningMode::LIVE_STREAM,\n               options->output_face_landmarks, options->output_pose_landmarks,",
			"options->running_mode == core::RunningMode::LIVE_STREAM &&\n                   options->flow_limiter_enabled,\n               options->output_face_landmarks, options->output_pose_landmarks,",
		);
		text = text.replace(
			"               options->output_pose_segmentation_masks,\n               options->output_face_blendshapes),",
			"               options->output_pose_segmentation_masks,\n               options->output_face_blendshapes,\n               options->flow_limiter_max_in_flight,\n               options->flow_limiter_max_in_queue),",
		);
		text = text.replace(
			"    return tasks::core::AddFlowLimiterCalculator(graph, subgraph, {kImageTag},\n                                                 kPoseLandmarksTag);",
			"    return tasks::core::AddFlowLimiterCalculator(graph, subgraph, {kImageTag},\n                                                 kPoseLandmarksTag,\n                                                 flow_limiter_max_in_flight,\n                                                 flow_limiter_max_in_queue);",
		);
		text
	})?;
	copy_replacing_file(
		&repo.join("native/mediapipe/patches/image_to_tensor_converter_frame_buffer.cc"),
		&image_to_tensor_frame_buffer,
	)?;
	eprintln!("applied UNMotion MediaPipe vendor patches in {}", media_pipe_root.display());
	Ok(())
}

fn edit_text_file(path: &Path, edit: impl FnOnce(String) -> String) -> Result<()> {
	let original = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
	let normalized = original.replace("\r\n", "\n");
	let edited = edit(normalized);
	if edited != original {
		fs::write(path, edited).with_context(|| format!("failed to write {}", path.display()))?;
	}
	Ok(())
}

#[derive(Debug)]
struct BazelPythonEnv {
	path: OsString,
	bazel_sh: Option<OsString>,
}

fn prepare_bazel_python(repo: &Path) -> Result<BazelPythonEnv> {
	let python = find_python()?;
	let shim_dir = repo.join("tools/python3-shim");
	fs::create_dir_all(&shim_dir).with_context(|| format!("failed to create {}", shim_dir.display()))?;
	let shim = format!("@echo off\r\n\"{}\" %*\r\n", python.display());
	fs::write(shim_dir.join("python.bat"), &shim)?;
	fs::write(shim_dir.join("python3.bat"), &shim)?;
	for stale in ["python.exe", "python3.exe"] {
		let stale = shim_dir.join(stale);
		if stale.exists() {
			let _ = fs::remove_file(stale);
		}
	}

	let old_path = std::env::var_os("PATH").unwrap_or_default();
	let python_parent = python.parent().map(Path::to_path_buf).unwrap_or_default();
	let mut path = OsString::new();
	path.push(&shim_dir);
	path.push(if cfg!(windows) { ";" } else { ":" });
	path.push(&python_parent);
	path.push(if cfg!(windows) { ";" } else { ":" });
	path.push(old_path);

	let bazel_sh = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
	Ok(BazelPythonEnv {
		path,
		bazel_sh: bazel_sh.exists().then(|| bazel_sh.into_os_string()),
	})
}

fn find_python() -> Result<PathBuf> {
	if let Some(path) = std::env::var_os("PYTHON").map(PathBuf::from).filter(|path| path.exists()) {
		return Ok(path);
	}
	let bundled = std::env::var_os("USERPROFILE")
		.map(PathBuf::from)
		.map(|home| home.join(".cache/codex-runtimes/codex-primary-runtime/dependencies/python/python.exe"));
	if let Some(path) = bundled.filter(|path| path.exists()) {
		return Ok(path);
	}
	let mut cmd = Command::new(if cfg!(windows) { "where" } else { "which" });
	cmd.arg("python");
	let output = cmd.output().context("failed to locate python")?;
	if output.status.success() {
		if let Some(first) = String::from_utf8_lossy(&output.stdout).lines().next() {
			let path = PathBuf::from(first.trim());
			if path.exists() {
				return Ok(path);
			}
		}
	}
	bail!("could not find a runnable Python executable for Bazel")
}

fn find_halide_runtime() -> Result<Option<PathBuf>> {
	let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
		return Ok(None);
	};
	let bazel_root = home.join("_bazel_the");
	if !bazel_root.exists() {
		return Ok(None);
	}
	Ok(find_file_recursive(&bazel_root, "Halide.dll")?
		.into_iter()
		.find(|path| path.to_string_lossy().contains("windows_halide"))
		.and_then(|path| path.parent().map(Path::to_path_buf)))
}

fn find_file_recursive(root: &Path, file_name: &str) -> Result<Vec<PathBuf>> {
	let mut out = Vec::new();
	let mut stack = vec![root.to_path_buf()];
	while let Some(dir) = stack.pop() {
		let Ok(entries) = fs::read_dir(&dir) else {
			continue;
		};
		for entry in entries {
			let entry = entry?;
			let path = entry.path();
			if path.is_dir() {
				stack.push(path);
			} else if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
				out.push(path);
			}
		}
	}
	Ok(out)
}

fn build_mediapipe_dll(
	repo: &Path,
	media_pipe_root: &Path,
	bazelisk: &Path,
	python_env: &BazelPythonEnv,
	halide_runtime: Option<&Path>,
	opencv_bin_path: Option<&Path>,
	jobs: usize,
) -> Result<PathBuf> {
	let action_path = opencv_bin_path
		.map(|opencv_bin_path| prepend_env_path(opencv_bin_path, &python_env.path))
		.unwrap_or_else(|| python_env.path.clone());
	prime_bazel_mediapipe_external_repositories(media_pipe_root, bazelisk, python_env, halide_runtime, &action_path, jobs)?;
	patch_bazel_windows_external_repositories()?;
	let mut cmd = Command::new(bazelisk);
	cmd.current_dir(media_pipe_root)
		.env("PATH", &action_path)
		.args(["build", "-c", "opt"])
		.arg(format!("--jobs={jobs}"))
		.arg(format!("--local_cpu_resources={jobs}"))
		.arg(format!("--action_env=PATH={}", action_path.to_string_lossy()))
		.args(["--conlyopt=/std:c11", "--conlyopt=/experimental:c11atomics"])
		.arg(format!(
			"--define=HALIDE_RUNTIME_PATH={}",
			halide_runtime.map(|path| path.display().to_string()).unwrap_or_default()
		))
		.args([
			"--define=MEDIAPIPE_DISABLE_GPU=1",
			"--define=MEDIAPIPE_ENABLE_HALIDE=1",
			"//un-motion:un-motion-mediapipe.dll",
		]);
	if let Some(bazel_sh) = &python_env.bazel_sh {
		cmd.env("BAZEL_SH", bazel_sh);
	}
	run_command(&format!("bazel build //un-motion:un-motion-mediapipe.dll --jobs={jobs}"), &mut cmd)?;

	let built = media_pipe_root.join("bazel-bin/un-motion/un-motion-mediapipe.dll");
	if !built.exists() {
		bail!("Bazel build completed but DLL was not found at {}", built.display());
	}
	let _ = repo;
	Ok(built)
}

fn patch_windows_opencv_repository_path(text: &str, bazel_path: &str) -> String {
	let mut output = String::with_capacity(text.len() + bazel_path.len());
	let mut in_windows_opencv = false;
	for line in text.lines() {
		let trimmed = line.trim();
		if trimmed == "name = \"windows_opencv\"," {
			in_windows_opencv = true;
			output.push_str(line);
		} else if in_windows_opencv && trimmed.starts_with("path = ") {
			let indent = line.chars().take_while(|ch| ch.is_whitespace()).collect::<String>();
			output.push_str(&format!("{indent}path = \"{bazel_path}\","));
			in_windows_opencv = false;
		} else {
			output.push_str(line);
		}
		output.push('\n');
	}
	output
}

fn prime_bazel_mediapipe_external_repositories(
	media_pipe_root: &Path,
	bazelisk: &Path,
	python_env: &BazelPythonEnv,
	halide_runtime: Option<&Path>,
	action_path: &OsString,
	jobs: usize,
) -> Result<()> {
	if !cfg!(windows) {
		return Ok(());
	}
	let mut cmd = Command::new(bazelisk);
	cmd.current_dir(media_pipe_root)
		.env("PATH", action_path)
		.args(["fetch", "-c", "opt"])
		.arg(format!("--jobs={jobs}"))
		.arg(format!("--local_cpu_resources={jobs}"))
		.arg(format!("--action_env=PATH={}", action_path.to_string_lossy()))
		.args(["--conlyopt=/std:c11", "--conlyopt=/experimental:c11atomics"])
		.arg(format!(
			"--define=HALIDE_RUNTIME_PATH={}",
			halide_runtime.map(|path| path.display().to_string()).unwrap_or_default()
		))
		.args([
			"--define=MEDIAPIPE_DISABLE_GPU=1",
			"--define=MEDIAPIPE_ENABLE_HALIDE=1",
			"//un-motion:un-motion-mediapipe.dll",
		]);
	if let Some(bazel_sh) = &python_env.bazel_sh {
		cmd.env("BAZEL_SH", bazel_sh);
	}
	let _ = run_command("bazel fetch //un-motion:un-motion-mediapipe.dll", &mut cmd);
	Ok(())
}

fn patch_bazel_windows_external_repositories() -> Result<()> {
	if !cfg!(windows) {
		return Ok(());
	}
	let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
		return Ok(());
	};
	let bazel_root = home.join("_bazel_the");
	if !bazel_root.exists() {
		return Ok(());
	}
	for script in find_file_recursive(&bazel_root, "overlay_directories.py")? {
		patch_llvm_overlay_script(&script)?;
	}
	for source in find_file_recursive(&bazel_root, "stablehlo_reduce_window.cc")? {
		patch_stablehlo_reduce_window_for_msvc(&source)?;
	}
	Ok(())
}

fn patch_llvm_overlay_script(path: &Path) -> Result<()> {
	let Ok(text) = fs::read_to_string(path) else {
		return Ok(());
	};
	if !text.contains("def _symlink_abs(from_path, to_path):")
		|| text.contains("def _flush_unmotion_junctions():")
		|| text.contains("def _flush_junctions():")
	{
		return Ok(());
	}
	if text.contains("_flush_unmotion_junctions()") && !text.contains("def _flush_unmotion_junctions():") {
		let patched = text.replace("    _flush_unmotion_junctions()\n", "");
		fs::write(path, patched).with_context(|| format!("failed to patch {}", path.display()))?;
		return Ok(());
	}
	let mut patched = text.clone();
	if !patched.contains("import subprocess\n") {
		patched = patched.replace("import shutil\n", "import shutil\nimport subprocess\n");
	}
	if !patched.contains("import tempfile\n") {
		patched = patched.replace("import sys\n", "import sys\nimport tempfile\n");
	}
	patched = patched.replace(
		"def _symlink_abs(from_path, to_path):\n    os.symlink(os.path.abspath(from_path), os.path.abspath(to_path))\n",
		r#" _UNMOTION_JUNCTIONS = []


def _symlink_abs(from_path, to_path):
    from_path = os.path.abspath(from_path)
    to_path = os.path.abspath(to_path)
    try:
        os.symlink(from_path, to_path)
        return
    except OSError as e:
        if getattr(e, "winerror", None) != 1314:
            raise
    if os.path.isdir(from_path):
        _UNMOTION_JUNCTIONS.append((from_path, to_path))
    else:
        try:
            os.link(from_path, to_path)
        except OSError:
            shutil.copy2(from_path, to_path)


def _flush_unmotion_junctions():
    if not _UNMOTION_JUNCTIONS:
        return
    fd, path = tempfile.mkstemp(suffix=".cmd", text=True)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write("@echo off\n")
        for from_path, to_path in _UNMOTION_JUNCTIONS:
            f.write('mklink /J "{}" "{}" >NUL\n'.format(to_path, from_path))
            f.write("if errorlevel 1 exit /b %errorlevel%\n")
    try:
        subprocess.check_call(["cmd", "/c", path])
    finally:
        try:
            os.remove(path)
        except OSError:
            pass
"#
		.trim_start(),
	);
	if !patched.contains("_flush_unmotion_junctions()\n") {
		patched = patched.replace(
			"                _symlink_abs(\n                    os.path.join(args.src, relpath), os.path.join(args.target, relpath)\n                )\n",
			"                _symlink_abs(\n                    os.path.join(args.src, relpath), os.path.join(args.target, relpath)\n                )\n    _flush_unmotion_junctions()\n",
		);
	}
	if patched != text {
		fs::write(path, patched).with_context(|| format!("failed to patch {}", path.display()))?;
	}
	Ok(())
}

fn patch_stablehlo_reduce_window_for_msvc(path: &Path) -> Result<()> {
	let Ok(text) = fs::read_to_string(path) else {
		return Ok(());
	};
	if !text.contains("For instance: the following window has a [2, 2] shape and [2, 3] dilations.") {
		return Ok(());
	}
	let start = text.find("// For instance:").unwrap();
	let Some(relative_end) = text[start..].find("template <class Op, class Type>") else {
		return Ok(());
	};
	let end = start + relative_end;
	let mut patched = String::with_capacity(text.len());
	patched.push_str(&text[..start]);
	patched.push_str(&text[end..]);
	fs::write(path, patched).with_context(|| format!("failed to patch {}", path.display()))?;
	Ok(())
}

fn image_command(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let Some(subcommand) = args.first() else {
		eprintln!("usage: cargo xtask image resize --input in.png --output out.png --width 320 --height 240");
		bail!("missing image subcommand");
	};
	match subcommand.to_string_lossy().as_ref() {
		"resize" => resize_image_command(repo, args[1..].to_vec()),
		"--help" | "-h" | "help" => {
			eprintln!("usage: cargo xtask image resize --input in.png --output out.png --width 320 --height 240");
			Ok(())
		}
		other => bail!("unknown image subcommand: {other}"),
	}
}

fn vmc(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let Some(subcommand) = args.first() else {
		eprintln!("usage: cargo xtask vmc <capture-frame|stability|stability-summary> [options]");
		bail!("missing vmc subcommand");
	};
	match subcommand.to_string_lossy().as_ref() {
		"capture-frame" | "record-frame" | "frame" => capture_vmc_frame(repo, args[1..].to_vec()),
		"stability" | "stability-report" => vmc_stability_report(repo, args[1..].to_vec()),
		"stability-summary" | "stability-compare" => vmc_stability_summary(repo, args[1..].to_vec()),
		"--help" | "-h" | "help" => {
			eprintln!("usage: cargo xtask vmc <capture-frame|stability|stability-summary> [options]");
			Ok(())
		}
		other => bail!("unknown vmc subcommand: {other}"),
	}
}

fn unmf(repo: &Path, args: Vec<OsString>) -> Result<()> {
	let Some(subcommand) = args.first() else {
		eprintln!("usage: cargo xtask unmf <stability> [options]");
		bail!("missing unmf subcommand");
	};
	match subcommand.to_string_lossy().as_ref() {
		"stability" | "stability-report" => unmf_stability_report(repo, args[1..].to_vec()),
		"--help" | "-h" | "help" => {
			eprintln!(
				"usage: cargo xtask unmf stability [--key un-motion/frame] [--topic-mode frame|by-primary-source|by-stream-id] [--duration-ms 5000] [--output report.json] [--min-samples 2]"
			);
			Ok(())
		}
		other => bail!("unknown unmf subcommand: {other}"),
	}
}

#[derive(Debug)]
struct VmcFrameCaptureArgs {
	listen_addr: String,
	output: Option<PathBuf>,
	output_dir: PathBuf,
	timeout: Duration,
	collect_after_boundary: Duration,
	label: Option<String>,
	any_packet: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VmcFrameCaptureResult {
	listen_addr: String,
	output_path: String,
	packets: u32,
	messages: u32,
	bone_messages: u32,
	blendshape_messages: u32,
	duration_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VmcFrameCaptureEntry {
	timestamp_ms: u64,
	source_addr: String,
	addr: String,
	args: Vec<VmcFrameCaptureArg>,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum VmcFrameCaptureArg {
	String(String),
	Float(f32),
	Double(f64),
	Int(i32),
	Long(i64),
	Bool(bool),
	Other(String),
}

#[derive(Debug)]
struct VmcStabilityArgs {
	listen_addr: String,
	source_id: String,
	duration: Duration,
	read_timeout: Duration,
	output: Option<PathBuf>,
	min_samples: usize,
}

#[derive(Debug)]
struct VmcStabilitySummaryArgs {
	reports_dir: PathBuf,
	reports: Vec<PathBuf>,
	output: Option<PathBuf>,
	top: usize,
}

#[derive(Debug)]
struct UnmfStabilityArgs {
	base_key_expr: String,
	topic_mode: TopicMode,
	source_id: String,
	duration: Duration,
	read_timeout: Duration,
	output: Option<PathBuf>,
	min_samples: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnmfStabilityReport {
	key_expr: String,
	subscribe_key_expr: String,
	source_id: String,
	duration_ms: u64,
	frames: u64,
	unique_frame_timestamps: u64,
	duplicate_frame_timestamps: u64,
	non_monotonic_frame_timestamps: u64,
	sequence_gaps: u64,
	expected_dt_ns_samples: u64,
	expected_dt_ns_mean: f64,
	expected_dt_ns_max: f64,
	decode_errors: u64,
	root_samples: u64,
	bone_samples: u64,
	face_head_samples: u64,
	hand_joint_samples: u64,
	blendshape_samples: u64,
	tracking_states: Vec<UnmfStateCountReport>,
	sample_states: Vec<UnmfStateCountReport>,
	mediapipe_quality: Vec<UnmfQualityReport>,
	mediapipe_notes: Vec<UnmfStateCountReport>,
	tracks: Vec<VmcStabilityTrackReport>,
	blendshape_tracks: Vec<VmcStabilityScalarTrackReport>,
	worst_position_step: Vec<VmcStabilityTrackRank>,
	worst_rotation_step: Vec<VmcStabilityTrackRank>,
	worst_blendshape_step: Vec<VmcStabilityTrackRank>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnmfStateCountReport {
	scope: String,
	state: String,
	count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnmfQualityReport {
	part: String,
	reason: String,
	samples: u64,
	score_mean: f64,
	score_min: f64,
	score_max: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VmcStabilityReport {
	listen_addr: String,
	source_id: String,
	duration_ms: u64,
	packets: u64,
	decoded_frames: u64,
	vmc_payload_frames: u64,
	decode_errors: u64,
	root_samples: u64,
	bone_samples: u64,
	blendshape_samples: u64,
	blend_apply_frames: u64,
	tracks: Vec<VmcStabilityTrackReport>,
	#[serde(default)]
	blendshape_tracks: Vec<VmcStabilityScalarTrackReport>,
	worst_position_step: Vec<VmcStabilityTrackRank>,
	worst_rotation_step: Vec<VmcStabilityTrackRank>,
	#[serde(default)]
	worst_blendshape_step: Vec<VmcStabilityTrackRank>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VmcStabilityTrackReport {
	name: String,
	sample_count: u64,
	sample_rate_hz: f64,
	interval_mean_ms: f64,
	interval_std_ms: f64,
	interval_max_ms: f64,
	position_step_mean: f64,
	position_step_max: f64,
	rotation_step_mean_deg: f64,
	rotation_step_max_deg: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VmcStabilityTrackRank {
	name: String,
	sample_count: u64,
	value: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VmcStabilityScalarTrackReport {
	name: String,
	sample_count: u64,
	sample_rate_hz: f64,
	interval_mean_ms: f64,
	interval_std_ms: f64,
	interval_max_ms: f64,
	value_step_mean: f64,
	value_step_max: f64,
}

#[derive(Clone, Debug)]
struct VmcStabilityTrack {
	name: String,
	samples: u64,
	first_ms: f64,
	last_ms: f64,
	last_position: [f32; 3],
	last_rotation: [f32; 4],
	interval_ms: RunningStats,
	position_step: RunningStats,
	rotation_step_deg: RunningStats,
}

#[derive(Clone, Debug, Default)]
struct RunningStats {
	count: u64,
	sum: f64,
	sum_sq: f64,
	max: f64,
}

impl RunningStats {
	fn push(&mut self, value: f64) {
		self.count = self.count.saturating_add(1);
		self.sum += value;
		self.sum_sq += value * value;
		self.max = self.max.max(value);
	}

	fn mean(&self) -> f64 {
		if self.count == 0 { 0.0 } else { self.sum / self.count as f64 }
	}

	fn stddev(&self) -> f64 {
		if self.count < 2 {
			return 0.0;
		}
		let mean = self.mean();
		((self.sum_sq / self.count as f64) - (mean * mean)).max(0.0).sqrt()
	}
}

#[derive(Clone, Debug)]
struct VmcStabilityScalarTrack {
	name: String,
	samples: u64,
	first_ms: f64,
	last_ms: f64,
	last_value: f32,
	interval_ms: RunningStats,
	value_step: RunningStats,
}

impl VmcStabilityScalarTrack {
	fn new(name: impl Into<String>, time_ms: f64, value: f32) -> Self {
		Self {
			name: name.into(),
			samples: 1,
			first_ms: time_ms,
			last_ms: time_ms,
			last_value: value,
			interval_ms: RunningStats::default(),
			value_step: RunningStats::default(),
		}
	}

	fn push(&mut self, time_ms: f64, value: f32) {
		self.interval_ms.push((time_ms - self.last_ms).max(0.0));
		self.value_step.push(f64::from((value - self.last_value).abs()));
		self.samples = self.samples.saturating_add(1);
		self.last_ms = time_ms;
		self.last_value = value;
	}

	fn report(&self) -> VmcStabilityScalarTrackReport {
		let elapsed_ms = (self.last_ms - self.first_ms).max(0.0);
		let sample_rate_hz = if elapsed_ms > 0.0 && self.samples > 1 {
			(self.samples - 1) as f64 * 1000.0 / elapsed_ms
		} else {
			0.0
		};
		VmcStabilityScalarTrackReport {
			name: self.name.clone(),
			sample_count: self.samples,
			sample_rate_hz: round3(sample_rate_hz),
			interval_mean_ms: round3(self.interval_ms.mean()),
			interval_std_ms: round3(self.interval_ms.stddev()),
			interval_max_ms: round3(self.interval_ms.max),
			value_step_mean: round6(self.value_step.mean()),
			value_step_max: round6(self.value_step.max),
		}
	}
}

impl VmcStabilityTrack {
	fn new(name: impl Into<String>, time_ms: f64, transform: &VmcTransform) -> Self {
		Self {
			name: name.into(),
			samples: 1,
			first_ms: time_ms,
			last_ms: time_ms,
			last_position: transform.position,
			last_rotation: normalize_quat(transform.rotation),
			interval_ms: RunningStats::default(),
			position_step: RunningStats::default(),
			rotation_step_deg: RunningStats::default(),
		}
	}

	fn push(&mut self, time_ms: f64, transform: &VmcTransform) {
		let position = transform.position;
		let rotation = normalize_quat(transform.rotation);
		self.interval_ms.push((time_ms - self.last_ms).max(0.0));
		self.position_step.push(position_distance(self.last_position, position));
		self.rotation_step_deg.push(quat_angle_deg(self.last_rotation, rotation));
		self.samples = self.samples.saturating_add(1);
		self.last_ms = time_ms;
		self.last_position = position;
		self.last_rotation = rotation;
	}

	fn report(&self) -> VmcStabilityTrackReport {
		let elapsed_ms = (self.last_ms - self.first_ms).max(0.0);
		let sample_rate_hz = if elapsed_ms > 0.0 && self.samples > 1 {
			(self.samples - 1) as f64 * 1000.0 / elapsed_ms
		} else {
			0.0
		};
		VmcStabilityTrackReport {
			name: self.name.clone(),
			sample_count: self.samples,
			sample_rate_hz: round3(sample_rate_hz),
			interval_mean_ms: round3(self.interval_ms.mean()),
			interval_std_ms: round3(self.interval_ms.stddev()),
			interval_max_ms: round3(self.interval_ms.max),
			position_step_mean: round6(self.position_step.mean()),
			position_step_max: round6(self.position_step.max),
			rotation_step_mean_deg: round3(self.rotation_step_deg.mean()),
			rotation_step_max_deg: round3(self.rotation_step_deg.max),
		}
	}
}

fn capture_vmc_frame(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_vmc_frame_capture_args(raw_args)?;
	let listen_addr = args.listen_addr;
	let socket = UdpSocket::bind(&listen_addr).with_context(|| format!("VMC frame recorder bind failed on {listen_addr}"))?;
	socket
		.set_read_timeout(Some(Duration::from_millis(50)))
		.context("VMC frame recorder timeout setup failed")?;

	let output_path = match args.output {
		Some(path) => absolutize(repo, &path),
		None => {
			let output_dir = absolutize(repo, &args.output_dir);
			let label = args
				.label
				.as_deref()
				.map(sanitize_capture_label)
				.filter(|value| !value.is_empty())
				.unwrap_or_else(|| now_unix_ms().to_string());
			output_dir.join(format!("vmc-frame-{label}-{}.jsonl", listen_port_label(&listen_addr)))
		}
	};
	if let Some(parent) = output_path.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}

	let file = File::create(&output_path).with_context(|| format!("failed to create {}", output_path.display()))?;
	let mut writer = BufWriter::new(file);
	let started = Instant::now();
	let mut buf = [0_u8; 65535];
	let mut packets = 0_u32;
	let mut messages = 0_u32;
	let mut bone_messages = 0_u32;
	let mut blendshape_messages = 0_u32;
	let mut capture_started = false;
	let mut collect_deadline = None;

	while started.elapsed() < args.timeout {
		let (len, source_addr) = match socket.recv_from(&mut buf) {
			Ok(value) => value,
			Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
				if collect_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
					break;
				}
				continue;
			}
			Err(error) => return Err(error).context("VMC frame recorder recv failed"),
		};
		let Ok((_, packet)) = decoder::decode_udp(&buf[..len]) else {
			continue;
		};
		let messages_in_packet = flatten_osc_messages(packet);
		if !capture_started {
			if !args.any_packet && !messages_in_packet.iter().any(is_vmc_frame_boundary_message) {
				continue;
			}
			capture_started = true;
			if !args.collect_after_boundary.is_zero() {
				collect_deadline = Some(Instant::now() + args.collect_after_boundary);
			}
		}

		packets = packets.saturating_add(1);
		for message in messages_in_packet {
			if message.addr == "/VMC/Ext/Bone/Pos" {
				bone_messages = bone_messages.saturating_add(1);
			}
			if message.addr == "/VMC/Ext/Blend/Val" {
				blendshape_messages = blendshape_messages.saturating_add(1);
			}
			let entry = VmcFrameCaptureEntry {
				timestamp_ms: now_unix_ms(),
				source_addr: source_addr.to_string(),
				addr: message.addr,
				args: message.args.into_iter().map(vmc_frame_capture_arg).collect(),
			};
			serde_json::to_writer(&mut writer, &entry).context("VMC frame write failed")?;
			writer.write_all(b"\n").context("VMC frame newline write failed")?;
			messages = messages.saturating_add(1);
		}
		if collect_deadline.is_none() || collect_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
			break;
		}
	}
	writer.flush().context("VMC frame flush failed")?;

	if messages == 0 {
		let _ = fs::remove_file(&output_path);
		bail!("VMC frame recorder timed out on {listen_addr}");
	}

	let result = VmcFrameCaptureResult {
		listen_addr,
		output_path: output_path.display().to_string(),
		packets,
		messages,
		bone_messages,
		blendshape_messages,
		duration_ms: started.elapsed().as_millis() as u64,
	};
	println!("{}", serde_json::to_string_pretty(&result)?);
	Ok(())
}

fn vmc_stability_report(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_vmc_stability_args(raw_args)?;
	let socket = UdpSocket::bind(&args.listen_addr).with_context(|| format!("VMC stability bind failed on {}", args.listen_addr))?;
	socket
		.set_read_timeout(Some(args.read_timeout))
		.context("VMC stability timeout setup failed")?;
	let report = collect_vmc_stability_report(args.listen_addr, args.source_id, &socket, args.duration, args.min_samples)?;
	write_and_print_vmc_stability_report(repo, args.output.as_deref(), &report)
}

fn collect_vmc_stability_report(
	listen_addr: String,
	source_id: String,
	socket: &UdpSocket,
	duration: Duration,
	min_samples: usize,
) -> Result<VmcStabilityReport> {
	let mut collector = VmcStabilityCollector::new(source_id);
	let started = Instant::now();
	let mut buf = [0_u8; 65535];

	while started.elapsed() < duration {
		let (len, _source_addr) = match socket.recv_from(&mut buf) {
			Ok(value) => value,
			Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
			Err(error) => return Err(error).context("VMC stability recv failed"),
		};
		collector.observe_datagram(&buf[..len], started.elapsed().as_secs_f64() * 1000.0, now_unix_ms() * 1_000_000);
	}

	Ok(collector.into_report(listen_addr, started.elapsed().as_millis() as u64, min_samples))
}

struct VmcStabilityCollector {
	source_id: String,
	decoder: VmcPacketDecoder,
	tracks: BTreeMap<String, VmcStabilityTrack>,
	blendshape_tracks: BTreeMap<String, VmcStabilityScalarTrack>,
	packets: u64,
	decoded_frames: u64,
	vmc_payload_frames: u64,
	decode_errors: u64,
	root_samples: u64,
	bone_samples: u64,
	blendshape_samples: u64,
	blend_apply_frames: u64,
}

impl VmcStabilityCollector {
	fn new(source_id: String) -> Self {
		Self {
			decoder: VmcPacketDecoder::new(source_id.clone()),
			source_id,
			tracks: BTreeMap::new(),
			blendshape_tracks: BTreeMap::new(),
			packets: 0,
			decoded_frames: 0,
			vmc_payload_frames: 0,
			decode_errors: 0,
			root_samples: 0,
			bone_samples: 0,
			blendshape_samples: 0,
			blend_apply_frames: 0,
		}
	}

	fn observe_datagram(&mut self, data: &[u8], elapsed_ms: f64, received_timestamp_ns: u64) {
		self.packets = self.packets.saturating_add(1);
		match self.decoder.decode_datagram(data, received_timestamp_ns) {
			Ok(Some(frame)) => {
				self.decoded_frames = self.decoded_frames.saturating_add(1);
				if frame.has_vmc_payload() {
					self.vmc_payload_frames = self.vmc_payload_frames.saturating_add(1);
				}
				let observed = observe_vmc_stability_frame(&mut self.tracks, &frame, elapsed_ms);
				self.root_samples = self.root_samples.saturating_add(observed.root_samples);
				self.bone_samples = self.bone_samples.saturating_add(observed.bone_samples);
				observe_vmc_stability_blendshapes(&mut self.blendshape_tracks, &frame, elapsed_ms);
				self.blendshape_samples = self.blendshape_samples.saturating_add(frame.blendshapes.len() as u64);
				if frame.blend_apply {
					self.blend_apply_frames = self.blend_apply_frames.saturating_add(1);
				}
			}
			Ok(None) => {}
			Err(_) => {
				self.decode_errors = self.decode_errors.saturating_add(1);
			}
		}
	}

	fn into_report(self, listen_addr: String, duration_ms: u64, min_samples: usize) -> VmcStabilityReport {
		let mut track_reports = self
			.tracks
			.values()
			.filter(|track| track.samples as usize >= min_samples)
			.map(VmcStabilityTrack::report)
			.collect::<Vec<_>>();
		track_reports.sort_by(|left, right| left.name.cmp(&right.name));
		let mut worst_position_step = track_reports
			.iter()
			.map(|track| VmcStabilityTrackRank {
				name: track.name.clone(),
				sample_count: track.sample_count,
				value: track.position_step_max,
			})
			.collect::<Vec<_>>();
		worst_position_step.sort_by(|left, right| right.value.partial_cmp(&left.value).unwrap_or(std::cmp::Ordering::Equal));
		worst_position_step.truncate(12);
		let mut worst_rotation_step = track_reports
			.iter()
			.map(|track| VmcStabilityTrackRank {
				name: track.name.clone(),
				sample_count: track.sample_count,
				value: track.rotation_step_max_deg,
			})
			.collect::<Vec<_>>();
		worst_rotation_step.sort_by(|left, right| right.value.partial_cmp(&left.value).unwrap_or(std::cmp::Ordering::Equal));
		worst_rotation_step.truncate(12);
		let mut blendshape_track_reports = self
			.blendshape_tracks
			.values()
			.filter(|track| track.samples as usize >= min_samples)
			.map(VmcStabilityScalarTrack::report)
			.collect::<Vec<_>>();
		blendshape_track_reports.sort_by(|left, right| left.name.cmp(&right.name));
		let mut worst_blendshape_step = blendshape_track_reports
			.iter()
			.map(|track| VmcStabilityTrackRank {
				name: track.name.clone(),
				sample_count: track.sample_count,
				value: track.value_step_max,
			})
			.collect::<Vec<_>>();
		worst_blendshape_step.sort_by(|left, right| right.value.partial_cmp(&left.value).unwrap_or(std::cmp::Ordering::Equal));
		worst_blendshape_step.truncate(12);

		VmcStabilityReport {
			listen_addr,
			source_id: self.source_id,
			duration_ms,
			packets: self.packets,
			decoded_frames: self.decoded_frames,
			vmc_payload_frames: self.vmc_payload_frames,
			decode_errors: self.decode_errors,
			root_samples: self.root_samples,
			bone_samples: self.bone_samples,
			blendshape_samples: self.blendshape_samples,
			blend_apply_frames: self.blend_apply_frames,
			tracks: track_reports,
			blendshape_tracks: blendshape_track_reports,
			worst_position_step,
			worst_rotation_step,
			worst_blendshape_step,
		}
	}
}

fn unmf_stability_report(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_unmf_stability_args(raw_args)?;
	let strategy = ZenohTopicStrategy::new(args.base_key_expr.clone(), args.topic_mode);
	let subscribe_key_expr = strategy.subscribe_key_expr();
	let mut backend = ZenohSubscriberBackend::open_default().context("UNMF/Z stability Zenoh open failed")?;
	let subscriber = Subscriber::declare(&mut backend, strategy)
		.with_context(|| format!("UNMF/Z stability subscribe failed on {subscribe_key_expr}"))?;
	let report = collect_unmf_stability_report(
		args.base_key_expr,
		subscribe_key_expr,
		args.source_id,
		&subscriber,
		args.duration,
		args.read_timeout,
		args.min_samples,
	)?;
	write_and_print_unmf_stability_report(repo, args.output.as_deref(), &report)
}

fn collect_unmf_stability_report(
	key_expr: String,
	subscribe_key_expr: String,
	source_id: String,
	subscriber: &Subscriber,
	duration: Duration,
	read_timeout: Duration,
	min_samples: usize,
) -> Result<UnmfStabilityReport> {
	let mut collector = UnmfStabilityCollector::new(source_id);
	let started = Instant::now();
	while started.elapsed() < duration {
		match subscriber.recv_frame_timeout(read_timeout) {
			Ok(Some(frame)) => collector.observe_frame(&frame, started.elapsed().as_secs_f64() * 1000.0),
			Ok(None) => {}
			Err(error) => {
				collector.decode_errors = collector.decode_errors.saturating_add(1);
				eprintln!("UNMF/Z stability decode error: {error}");
			}
		}
	}
	Ok(collector.into_report(key_expr, subscribe_key_expr, started.elapsed().as_millis() as u64, min_samples))
}

struct UnmfStabilityCollector {
	source_id: String,
	tracks: BTreeMap<String, VmcStabilityTrack>,
	blendshape_tracks: BTreeMap<String, VmcStabilityScalarTrack>,
	tracking_states: BTreeMap<(String, String), u64>,
	sample_states: BTreeMap<(String, String), u64>,
	mediapipe_quality: BTreeMap<(String, String), QualityStats>,
	mediapipe_notes: BTreeMap<(String, String), u64>,
	frames: u64,
	last_frame_timestamp_ns: Option<u64>,
	unique_frame_timestamps: u64,
	duplicate_frame_timestamps: u64,
	non_monotonic_frame_timestamps: u64,
	last_sequence: Option<u64>,
	sequence_gaps: u64,
	expected_dt_ns: RunningStats,
	decode_errors: u64,
	root_samples: u64,
	bone_samples: u64,
	face_head_samples: u64,
	hand_joint_samples: u64,
	blendshape_samples: u64,
}

impl UnmfStabilityCollector {
	fn new(source_id: String) -> Self {
		Self {
			source_id,
			tracks: BTreeMap::new(),
			blendshape_tracks: BTreeMap::new(),
			tracking_states: BTreeMap::new(),
			sample_states: BTreeMap::new(),
			mediapipe_quality: BTreeMap::new(),
			mediapipe_notes: BTreeMap::new(),
			frames: 0,
			last_frame_timestamp_ns: None,
			unique_frame_timestamps: 0,
			duplicate_frame_timestamps: 0,
			non_monotonic_frame_timestamps: 0,
			last_sequence: None,
			sequence_gaps: 0,
			expected_dt_ns: RunningStats::default(),
			decode_errors: 0,
			root_samples: 0,
			bone_samples: 0,
			face_head_samples: 0,
			hand_joint_samples: 0,
			blendshape_samples: 0,
		}
	}

	fn observe_frame(&mut self, frame: &UNMotionFrame, elapsed_ms: f64) {
		self.frames = self.frames.saturating_add(1);
		if let Some(previous) = self.last_frame_timestamp_ns {
			match frame.header.frame_timestamp_ns.cmp(&previous) {
				std::cmp::Ordering::Greater => self.unique_frame_timestamps = self.unique_frame_timestamps.saturating_add(1),
				std::cmp::Ordering::Equal => self.duplicate_frame_timestamps = self.duplicate_frame_timestamps.saturating_add(1),
				std::cmp::Ordering::Less => self.non_monotonic_frame_timestamps = self.non_monotonic_frame_timestamps.saturating_add(1),
			}
		} else {
			self.unique_frame_timestamps = self.unique_frame_timestamps.saturating_add(1);
		}
		self.last_frame_timestamp_ns = Some(frame.header.frame_timestamp_ns);
		if let Some(previous) = self.last_sequence
			&& frame.header.sequence != previous.saturating_add(1)
		{
			self.sequence_gaps = self.sequence_gaps.saturating_add(1);
		}
		self.last_sequence = Some(frame.header.sequence);
		if let Some(expected_dt_ns) = frame.header.expected_dt_ns {
			self.expected_dt_ns.push(expected_dt_ns as f64);
		}
		observe_unmf_metadata_notes(&mut self.mediapipe_quality, &mut self.mediapipe_notes, &frame.metadata.notes);
		if let Some(body) = frame.body.as_ref()
			&& let Some(humanoid) = body.humanoid.as_ref()
		{
			increment_unmf_tracking_state(&mut self.tracking_states, "body", body.tracking_state);
			if let Some(root) = humanoid.root.as_ref() {
				push_unmf_stability_sample(&mut self.tracks, "Root", root, elapsed_ms);
				self.root_samples = self.root_samples.saturating_add(1);
			}
			for bone in &humanoid.bones {
				increment_unmf_sample_state(&mut self.sample_states, &format!("bone.{:?}", bone.bone), bone.state);
				push_unmf_stability_sample(&mut self.tracks, &format!("{:?}", bone.bone), &bone.transform, elapsed_ms);
				self.bone_samples = self.bone_samples.saturating_add(1);
			}
		}
		if let Some(face) = frame.face.as_ref() {
			increment_unmf_tracking_state(&mut self.tracking_states, "face", face.tracking_state);
			if let Some(head) = face.head.as_ref() {
				push_unmf_stability_sample(&mut self.tracks, "FaceHead", head, elapsed_ms);
				self.face_head_samples = self.face_head_samples.saturating_add(1);
			}
			observe_unmf_blendshapes(&mut self.blendshape_tracks, &mut self.sample_states, &face.expressions, elapsed_ms);
			self.blendshape_samples = self.blendshape_samples.saturating_add(face.expressions.len() as u64);
		}
		observe_unmf_hand(
			&mut self.tracks,
			&mut self.tracking_states,
			"LeftHand",
			frame.left_hand.as_ref(),
			elapsed_ms,
			&mut self.hand_joint_samples,
		);
		observe_unmf_hand(
			&mut self.tracks,
			&mut self.tracking_states,
			"RightHand",
			frame.right_hand.as_ref(),
			elapsed_ms,
			&mut self.hand_joint_samples,
		);
	}

	fn into_report(self, key_expr: String, subscribe_key_expr: String, duration_ms: u64, min_samples: usize) -> UnmfStabilityReport {
		let mut track_reports = self
			.tracks
			.values()
			.filter(|track| track.samples as usize >= min_samples)
			.map(VmcStabilityTrack::report)
			.collect::<Vec<_>>();
		track_reports.sort_by(|left, right| left.name.cmp(&right.name));
		let mut worst_position_step = track_reports
			.iter()
			.map(|track| VmcStabilityTrackRank {
				name: track.name.clone(),
				sample_count: track.sample_count,
				value: track.position_step_max,
			})
			.collect::<Vec<_>>();
		worst_position_step.sort_by(|left, right| right.value.partial_cmp(&left.value).unwrap_or(std::cmp::Ordering::Equal));
		worst_position_step.truncate(12);
		let mut worst_rotation_step = track_reports
			.iter()
			.map(|track| VmcStabilityTrackRank {
				name: track.name.clone(),
				sample_count: track.sample_count,
				value: track.rotation_step_max_deg,
			})
			.collect::<Vec<_>>();
		worst_rotation_step.sort_by(|left, right| right.value.partial_cmp(&left.value).unwrap_or(std::cmp::Ordering::Equal));
		worst_rotation_step.truncate(12);
		let mut blendshape_track_reports = self
			.blendshape_tracks
			.values()
			.filter(|track| track.samples as usize >= min_samples)
			.map(VmcStabilityScalarTrack::report)
			.collect::<Vec<_>>();
		blendshape_track_reports.sort_by(|left, right| left.name.cmp(&right.name));
		let mut worst_blendshape_step = blendshape_track_reports
			.iter()
			.map(|track| VmcStabilityTrackRank {
				name: track.name.clone(),
				sample_count: track.sample_count,
				value: track.value_step_max,
			})
			.collect::<Vec<_>>();
		worst_blendshape_step.sort_by(|left, right| right.value.partial_cmp(&left.value).unwrap_or(std::cmp::Ordering::Equal));
		worst_blendshape_step.truncate(12);

		UnmfStabilityReport {
			key_expr,
			subscribe_key_expr,
			source_id: self.source_id,
			duration_ms,
			frames: self.frames,
			unique_frame_timestamps: self.unique_frame_timestamps,
			duplicate_frame_timestamps: self.duplicate_frame_timestamps,
			non_monotonic_frame_timestamps: self.non_monotonic_frame_timestamps,
			sequence_gaps: self.sequence_gaps,
			expected_dt_ns_samples: self.expected_dt_ns.count,
			expected_dt_ns_mean: self.expected_dt_ns.mean(),
			expected_dt_ns_max: self.expected_dt_ns.max,
			decode_errors: self.decode_errors,
			root_samples: self.root_samples,
			bone_samples: self.bone_samples,
			face_head_samples: self.face_head_samples,
			hand_joint_samples: self.hand_joint_samples,
			blendshape_samples: self.blendshape_samples,
			tracking_states: state_count_reports(self.tracking_states),
			sample_states: state_count_reports(self.sample_states),
			mediapipe_quality: quality_reports(self.mediapipe_quality),
			mediapipe_notes: state_count_reports(self.mediapipe_notes),
			tracks: track_reports,
			blendshape_tracks: blendshape_track_reports,
			worst_position_step,
			worst_rotation_step,
			worst_blendshape_step,
		}
	}
}

fn observe_unmf_hand(
	tracks: &mut BTreeMap<String, VmcStabilityTrack>,
	tracking_states: &mut BTreeMap<(String, String), u64>,
	side: &str,
	hand: Option<&un_motion_frame::HandMotion>,
	elapsed_ms: f64,
	hand_joint_samples: &mut u64,
) {
	let Some(hand) = hand else {
		return;
	};
	increment_unmf_tracking_state(tracking_states, side, hand.tracking_state);
	if let Some(wrist) = hand.wrist.as_ref() {
		push_unmf_stability_sample(tracks, &format!("{side}.Wrist"), wrist, elapsed_ms);
		*hand_joint_samples = hand_joint_samples.saturating_add(1);
	}
	for finger in &hand.fingers {
		for (index, joint) in finger.joints.iter().enumerate() {
			push_unmf_stability_sample(tracks, &format!("{side}.{:?}.{index}", finger.finger), joint, elapsed_ms);
			*hand_joint_samples = hand_joint_samples.saturating_add(1);
		}
	}
}

fn observe_unmf_blendshapes(
	tracks: &mut BTreeMap<String, VmcStabilityScalarTrack>,
	sample_states: &mut BTreeMap<(String, String), u64>,
	expressions: &[ExpressionSample],
	elapsed_ms: f64,
) {
	for expression in expressions {
		increment_unmf_sample_state(sample_states, &format!("blendshape.{}", expression.name), expression.state);
		tracks
			.entry(expression.name.clone())
			.and_modify(|track| track.push(elapsed_ms, expression.value))
			.or_insert_with(|| VmcStabilityScalarTrack::new(expression.name.clone(), elapsed_ms, expression.value));
	}
}

fn push_unmf_stability_sample(
	tracks: &mut BTreeMap<String, VmcStabilityTrack>,
	name: &str,
	transform: &UnmotionTransformSample,
	elapsed_ms: f64,
) {
	let vmc_transform = VmcTransform {
		name: name.to_string(),
		position: transform.translation.map(vec3_to_array).unwrap_or([0.0, 0.0, 0.0]),
		rotation: transform.rotation.map(quat_to_array).unwrap_or([0.0, 0.0, 0.0, 1.0]),
	};
	push_vmc_stability_sample(tracks, name, &vmc_transform, elapsed_ms);
}

fn increment_unmf_tracking_state(states: &mut BTreeMap<(String, String), u64>, scope: &str, state: TrackingState) {
	increment_unmf_state_count(states, scope, &format!("{state:?}"));
}

fn increment_unmf_sample_state(states: &mut BTreeMap<(String, String), u64>, scope: &str, state: SampleState) {
	increment_unmf_state_count(states, scope, &format!("{state:?}"));
}

fn increment_unmf_state_count(states: &mut BTreeMap<(String, String), u64>, scope: &str, state: &str) {
	let key = (scope.to_string(), state.to_string());
	*states.entry(key).or_insert(0) += 1;
}

fn state_count_reports(states: BTreeMap<(String, String), u64>) -> Vec<UnmfStateCountReport> {
	states
		.into_iter()
		.map(|((scope, state), count)| UnmfStateCountReport { scope, state, count })
		.collect()
}

#[derive(Clone, Debug)]
struct QualityStats {
	samples: u64,
	sum: f64,
	min: f64,
	max: f64,
}

impl QualityStats {
	fn new(score: f32) -> Self {
		let score = f64::from(score);
		Self {
			samples: 1,
			sum: score,
			min: score,
			max: score,
		}
	}

	fn push(&mut self, score: f32) {
		let score = f64::from(score);
		self.samples = self.samples.saturating_add(1);
		self.sum += score;
		self.min = self.min.min(score);
		self.max = self.max.max(score);
	}

	fn mean(&self) -> f64 {
		if self.samples == 0 { 0.0 } else { self.sum / self.samples as f64 }
	}
}

fn observe_unmf_metadata_notes(
	quality: &mut BTreeMap<(String, String), QualityStats>,
	notes: &mut BTreeMap<(String, String), u64>,
	frame_notes: &[String],
) {
	for note in frame_notes {
		if let Some(rest) = note.strip_prefix("mediapipe.quality ") {
			for token in rest.split_whitespace() {
				if let Some((part, score, reason)) = parse_mediapipe_quality_token(token) {
					quality
						.entry((part.to_string(), reason.to_string()))
						.and_modify(|stats| stats.push(score))
						.or_insert_with(|| QualityStats::new(score));
				}
			}
		} else if let Some(rest) = note.strip_prefix("mediapipe.stability ") {
			if let Some((scope, state)) = rest.split_once('=') {
				increment_unmf_state_count(notes, scope, state);
			}
		}
	}
}

fn parse_mediapipe_quality_token(token: &str) -> Option<(&str, f32, &str)> {
	let (part, rest) = token.split_once('=')?;
	let (score, reason) = rest.split_once('(')?;
	let reason = reason.strip_suffix(')')?;
	let score = score.parse::<f32>().ok()?;
	Some((part, score, reason))
}

fn quality_reports(quality: BTreeMap<(String, String), QualityStats>) -> Vec<UnmfQualityReport> {
	quality
		.into_iter()
		.map(|((part, reason), stats)| UnmfQualityReport {
			part,
			reason,
			samples: stats.samples,
			score_mean: stats.mean(),
			score_min: stats.min,
			score_max: stats.max,
		})
		.collect()
}

fn vec3_to_array(value: Vec3f) -> [f32; 3] {
	[value.x, value.y, value.z]
}

fn quat_to_array(value: Quatf) -> [f32; 4] {
	[value.x, value.y, value.z, value.w]
}

fn write_and_print_unmf_stability_report(repo: &Path, output: Option<&Path>, report: &UnmfStabilityReport) -> Result<()> {
	let json = serde_json::to_string_pretty(&report)?;
	if let Some(path) = output {
		let output_path = absolutize(repo, path);
		if let Some(parent) = output_path.parent() {
			fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
		}
		fs::write(&output_path, json.as_bytes()).with_context(|| format!("failed to write {}", output_path.display()))?;
	}
	println!("{json}");
	Ok(())
}

fn write_and_print_vmc_stability_report(repo: &Path, output: Option<&Path>, report: &VmcStabilityReport) -> Result<()> {
	let json = serde_json::to_string_pretty(&report)?;
	if let Some(path) = output {
		write_vmc_stability_report(repo, path, report)?;
	}
	println!("{json}");
	Ok(())
}

fn write_vmc_stability_report(repo: &Path, path: &Path, report: &VmcStabilityReport) -> Result<()> {
	let output_path = absolutize(repo, path);
	if let Some(parent) = output_path.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	let json = serde_json::to_string_pretty(report)?;
	fs::write(&output_path, json.as_bytes()).with_context(|| format!("failed to write {}", output_path.display()))?;
	Ok(())
}

fn vmc_stability_summary(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_vmc_stability_summary_args(raw_args)?;
	let report_paths = resolve_vmc_stability_report_paths(repo, &args)?;
	if report_paths.is_empty() {
		bail!("no stability reports found under {}", absolutize(repo, &args.reports_dir).display());
	}
	let mut reports = Vec::new();
	for path in report_paths {
		let report_path = absolutize(repo, &path);
		let text = fs::read_to_string(&report_path).with_context(|| format!("failed to read {}", report_path.display()))?;
		let report =
			serde_json::from_str::<VmcStabilityReport>(&text).with_context(|| format!("failed to parse {}", report_path.display()))?;
		reports.push(report);
	}
	reports.sort_by(|left, right| left.source_id.cmp(&right.source_id));
	let markdown = render_vmc_stability_summary(&reports, args.top);
	if let Some(path) = args.output {
		let output_path = absolutize(repo, &path);
		if let Some(parent) = output_path.parent() {
			fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
		}
		fs::write(&output_path, markdown.as_bytes()).with_context(|| format!("failed to write {}", output_path.display()))?;
	}
	print!("{markdown}");
	Ok(())
}

fn resolve_vmc_stability_report_paths(repo: &Path, args: &VmcStabilitySummaryArgs) -> Result<Vec<PathBuf>> {
	if !args.reports.is_empty() {
		return Ok(args.reports.clone());
	}
	let reports_dir = absolutize(repo, &args.reports_dir);
	if !reports_dir.exists() {
		return Ok(Vec::new());
	}
	let mut paths = Vec::new();
	for entry in fs::read_dir(&reports_dir).with_context(|| format!("failed to read {}", reports_dir.display()))? {
		let path = entry?.path();
		if path
			.extension()
			.and_then(|ext| ext.to_str())
			.is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
		{
			paths.push(path);
		}
	}
	paths.sort();
	Ok(paths)
}

fn render_vmc_stability_summary(reports: &[VmcStabilityReport], top: usize) -> String {
	let mut out = String::new();
	out.push_str("| source | addr | packets | fps | decode errors | interval max ms | interval std max ms | pos step max | rot step max deg | blend step max | worst rot track |\n");
	out.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
	for report in reports {
		let fps = if report.duration_ms > 0 {
			report.decoded_frames as f64 * 1000.0 / report.duration_ms as f64
		} else {
			0.0
		};
		let interval_max = report.tracks.iter().map(|track| track.interval_max_ms).fold(0.0_f64, f64::max);
		let interval_std = report.tracks.iter().map(|track| track.interval_std_ms).fold(0.0_f64, f64::max);
		let pos_step = report.worst_position_step.first().map(|track| track.value).unwrap_or_default();
		let worst_rot = report.worst_rotation_step.first();
		let rot_step = worst_rot.map(|track| track.value).unwrap_or_default();
		let blend_step = report.worst_blendshape_step.first().map(|track| track.value).unwrap_or_default();
		let worst_rot_track = worst_rot
			.map(|track| format!("{} ({})", track.name, track.sample_count))
			.unwrap_or_else(|| "-".to_string());
		out.push_str(&format!(
			"| {} | {} | {} | {:.2} | {} | {:.3} | {:.3} | {:.6} | {:.3} | {:.6} | {} |\n",
			report.source_id,
			report.listen_addr,
			report.packets,
			fps,
			report.decode_errors,
			interval_max,
			interval_std,
			pos_step,
			rot_step,
			blend_step,
			worst_rot_track
		));
	}

	let top = top.max(1);
	for report in reports {
		out.push('\n');
		out.push_str(&format!("## {}\n\n", report.source_id));
		out.push_str("| rank | rotation step deg | track | samples |\n");
		out.push_str("| ---: | ---: | --- | ---: |\n");
		for (index, track) in report.worst_rotation_step.iter().take(top).enumerate() {
			out.push_str(&format!(
				"| {} | {:.3} | {} | {} |\n",
				index + 1,
				track.value,
				track.name,
				track.sample_count
			));
		}
		if !report.worst_blendshape_step.is_empty() {
			out.push('\n');
			out.push_str("| rank | blendshape step | name | samples |\n");
			out.push_str("| ---: | ---: | --- | ---: |\n");
			for (index, track) in report.worst_blendshape_step.iter().take(top).enumerate() {
				out.push_str(&format!(
					"| {} | {:.6} | {} | {} |\n",
					index + 1,
					track.value,
					track.name,
					track.sample_count
				));
			}
		}
	}
	out
}

#[derive(Default)]
struct VmcStabilityObserved {
	root_samples: u64,
	bone_samples: u64,
}

fn observe_vmc_stability_frame(
	tracks: &mut BTreeMap<String, VmcStabilityTrack>,
	frame: &VmcInputFrame,
	elapsed_ms: f64,
) -> VmcStabilityObserved {
	let mut observed = VmcStabilityObserved::default();
	if let Some(root) = &frame.root {
		push_vmc_stability_sample(tracks, "Root", root, elapsed_ms);
		observed.root_samples = observed.root_samples.saturating_add(1);
	}
	for bone in &frame.bones {
		let transform = VmcTransform {
			name: bone.name.clone(),
			position: bone.position,
			rotation: bone.rotation,
		};
		push_vmc_stability_sample(tracks, &bone.name, &transform, elapsed_ms);
		observed.bone_samples = observed.bone_samples.saturating_add(1);
	}
	observed
}

fn observe_vmc_stability_blendshapes(tracks: &mut BTreeMap<String, VmcStabilityScalarTrack>, frame: &VmcInputFrame, elapsed_ms: f64) {
	for blendshape in &frame.blendshapes {
		tracks
			.entry(blendshape.name.clone())
			.and_modify(|track| track.push(elapsed_ms, blendshape.value))
			.or_insert_with(|| VmcStabilityScalarTrack::new(blendshape.name.clone(), elapsed_ms, blendshape.value));
	}
}

fn push_vmc_stability_sample(tracks: &mut BTreeMap<String, VmcStabilityTrack>, name: &str, transform: &VmcTransform, elapsed_ms: f64) {
	tracks
		.entry(name.to_string())
		.and_modify(|track| track.push(elapsed_ms, transform))
		.or_insert_with(|| VmcStabilityTrack::new(name, elapsed_ms, transform));
}

fn position_distance(left: [f32; 3], right: [f32; 3]) -> f64 {
	let dx = f64::from(left[0] - right[0]);
	let dy = f64::from(left[1] - right[1]);
	let dz = f64::from(left[2] - right[2]);
	(dx * dx + dy * dy + dz * dz).sqrt()
}

fn normalize_quat(value: [f32; 4]) -> [f32; 4] {
	let len = value.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>().sqrt();
	if len <= f64::EPSILON {
		return [0.0, 0.0, 0.0, 1.0];
	}
	[
		(f64::from(value[0]) / len) as f32,
		(f64::from(value[1]) / len) as f32,
		(f64::from(value[2]) / len) as f32,
		(f64::from(value[3]) / len) as f32,
	]
}

fn quat_angle_deg(left: [f32; 4], right: [f32; 4]) -> f64 {
	let dot = left
		.iter()
		.zip(right)
		.map(|(left, right)| f64::from(*left) * f64::from(right))
		.sum::<f64>()
		.abs()
		.clamp(-1.0, 1.0);
	(2.0 * dot.acos()).to_degrees()
}

fn round3(value: f64) -> f64 {
	(value * 1000.0).round() / 1000.0
}

fn round6(value: f64) -> f64 {
	(value * 1_000_000.0).round() / 1_000_000.0
}

fn parse_vmc_frame_capture_args(raw_args: Vec<OsString>) -> Result<VmcFrameCaptureArgs> {
	let mut listen_addr = "127.0.0.1:39551".to_string();
	let mut output = None;
	let mut output_dir = PathBuf::from("target/vmc-captures");
	let mut timeout = Duration::from_secs(5);
	let mut collect_after_boundary = Duration::ZERO;
	let mut label = None;
	let mut any_packet = false;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--addr" | "--listen" => listen_addr = next_value(&mut iter, "--addr")?.to_string_lossy().into_owned(),
			"--port" => {
				let port = next_value(&mut iter, "--port")?
					.to_string_lossy()
					.parse::<u16>()
					.context("invalid --port")?;
				listen_addr = format!("127.0.0.1:{port}");
			}
			"--output" | "-o" => output = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
			"--output-dir" => output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?),
			"--timeout-ms" => {
				timeout = Duration::from_millis(
					next_value(&mut iter, "--timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --timeout-ms")?,
				);
			}
			"--collect-ms" => {
				collect_after_boundary = Duration::from_millis(
					next_value(&mut iter, "--collect-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --collect-ms")?,
				);
			}
			"--label" => label = Some(next_value(&mut iter, "--label")?.to_string_lossy().into_owned()),
			"--any-packet" => any_packet = true,
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask vmc capture-frame [--addr 127.0.0.1:39551] [--output file.jsonl] [--output-dir target/vmc-captures] [--timeout-ms 5000] [--collect-ms 25] [--label name] [--any-packet]"
				);
				std::process::exit(0);
			}
			other if !other.starts_with('-') => listen_addr = other.to_string(),
			other => bail!("unexpected capture-frame argument: {other}"),
		}
	}
	if timeout.is_zero() {
		bail!("--timeout-ms must be greater than 0");
	}
	Ok(VmcFrameCaptureArgs {
		listen_addr,
		output,
		output_dir,
		timeout,
		collect_after_boundary,
		label,
		any_packet,
	})
}

fn parse_vmc_stability_args(raw_args: Vec<OsString>) -> Result<VmcStabilityArgs> {
	let mut listen_addr = "127.0.0.1:39551".to_string();
	let mut source_id = "vmc:stability".to_string();
	let mut duration = Duration::from_secs(5);
	let mut read_timeout = Duration::from_millis(50);
	let mut output = None;
	let mut min_samples = 2_usize;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--addr" | "--listen" => listen_addr = next_value(&mut iter, "--addr")?.to_string_lossy().into_owned(),
			"--port" => {
				let port = next_value(&mut iter, "--port")?
					.to_string_lossy()
					.parse::<u16>()
					.context("invalid --port")?;
				listen_addr = format!("127.0.0.1:{port}");
			}
			"--source-id" => source_id = next_value(&mut iter, "--source-id")?.to_string_lossy().into_owned(),
			"--duration-ms" => {
				duration = Duration::from_millis(
					next_value(&mut iter, "--duration-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --duration-ms")?,
				);
			}
			"--read-timeout-ms" => {
				read_timeout = Duration::from_millis(
					next_value(&mut iter, "--read-timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --read-timeout-ms")?,
				);
			}
			"--output" | "-o" => output = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
			"--min-samples" => {
				min_samples = next_value(&mut iter, "--min-samples")?
					.to_string_lossy()
					.parse::<usize>()
					.context("invalid --min-samples")?;
			}
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask vmc stability [--addr 127.0.0.1:39551] [--duration-ms 5000] [--source-id label] [--output report.json] [--min-samples 2]"
				);
				std::process::exit(0);
			}
			other if !other.starts_with('-') => listen_addr = other.to_string(),
			other => bail!("unexpected stability argument: {other}"),
		}
	}
	if duration.is_zero() {
		bail!("--duration-ms must be greater than 0");
	}
	if read_timeout.is_zero() {
		bail!("--read-timeout-ms must be greater than 0");
	}
	if min_samples == 0 {
		bail!("--min-samples must be greater than 0");
	}
	Ok(VmcStabilityArgs {
		listen_addr,
		source_id,
		duration,
		read_timeout,
		output,
		min_samples,
	})
}

fn parse_vmc_stability_summary_args(raw_args: Vec<OsString>) -> Result<VmcStabilitySummaryArgs> {
	let mut args = VmcStabilitySummaryArgs {
		reports_dir: PathBuf::from("target/vmc-captures/runs/stability"),
		reports: Vec::new(),
		output: None,
		top: 8,
	};
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--dir" | "--reports-dir" => args.reports_dir = PathBuf::from(next_value(&mut iter, "--dir")?),
			"--report" => args.reports.push(PathBuf::from(next_value(&mut iter, "--report")?)),
			"--output" | "-o" => args.output = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
			"--top" => {
				args.top = next_value(&mut iter, "--top")?
					.to_string_lossy()
					.parse::<usize>()
					.context("invalid --top")?;
			}
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask vmc stability-summary [--dir target/vmc-captures/runs/stability] [--report file.json] [--output summary.md] [--top 8]"
				);
				std::process::exit(0);
			}
			other if !other.starts_with('-') => args.reports.push(PathBuf::from(other)),
			other => bail!("unexpected stability-summary argument: {other}"),
		}
	}
	if args.top == 0 {
		bail!("--top must be greater than 0");
	}
	Ok(args)
}

fn parse_unmf_stability_args(raw_args: Vec<OsString>) -> Result<UnmfStabilityArgs> {
	let mut args = UnmfStabilityArgs {
		base_key_expr: "un-motion/frame".to_string(),
		topic_mode: TopicMode::Frame,
		source_id: "unmf:stability".to_string(),
		duration: Duration::from_secs(5),
		read_timeout: Duration::from_millis(50),
		output: None,
		min_samples: 2,
	};
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--key" | "--base-key" | "--base-key-expr" => {
				args.base_key_expr = next_value(&mut iter, "--key")?.to_string_lossy().into_owned()
			}
			"--topic-mode" | "--mode" => {
				let value = next_value(&mut iter, "--topic-mode")?.to_string_lossy().into_owned();
				args.topic_mode = parse_unmf_topic_mode(&value)?;
			}
			"--source-id" => args.source_id = next_value(&mut iter, "--source-id")?.to_string_lossy().into_owned(),
			"--duration-ms" => {
				args.duration = Duration::from_millis(
					next_value(&mut iter, "--duration-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --duration-ms")?,
				);
			}
			"--read-timeout-ms" => {
				args.read_timeout = Duration::from_millis(
					next_value(&mut iter, "--read-timeout-ms")?
						.to_string_lossy()
						.parse::<u64>()
						.context("invalid --read-timeout-ms")?,
				);
			}
			"--output" | "-o" => args.output = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
			"--min-samples" => {
				args.min_samples = next_value(&mut iter, "--min-samples")?
					.to_string_lossy()
					.parse::<usize>()
					.context("invalid --min-samples")?;
			}
			"--help" | "-h" => {
				eprintln!(
					"usage: cargo xtask unmf stability [--key un-motion/frame] [--topic-mode frame|by-primary-source|by-stream-id] [--duration-ms 5000] [--output report.json] [--min-samples 2]"
				);
				std::process::exit(0);
			}
			other if !other.starts_with('-') => args.base_key_expr = other.to_string(),
			other => bail!("unexpected unmf stability argument: {other}"),
		}
	}
	if args.base_key_expr.trim().is_empty() {
		bail!("--key must not be empty");
	}
	if args.duration.is_zero() {
		bail!("--duration-ms must be greater than 0");
	}
	if args.read_timeout.is_zero() {
		bail!("--read-timeout-ms must be greater than 0");
	}
	if args.min_samples == 0 {
		bail!("--min-samples must be greater than 0");
	}
	Ok(args)
}

fn parse_unmf_topic_mode(value: &str) -> Result<TopicMode> {
	match value {
		"frame" => Ok(TopicMode::Frame),
		"by-primary-source" | "primary-source" | "source" => Ok(TopicMode::ByPrimarySource),
		"by-stream-id" | "stream-id" | "stream" => Ok(TopicMode::ByStreamId),
		other => bail!("invalid --topic-mode: {other}"),
	}
}

fn flatten_osc_messages(packet: OscPacket) -> Vec<OscMessage> {
	match packet {
		OscPacket::Message(message) => vec![message],
		OscPacket::Bundle(bundle) => bundle.content.into_iter().flat_map(flatten_osc_messages).collect(),
	}
}

fn is_vmc_frame_boundary_message(message: &OscMessage) -> bool {
	message.addr == "/VMC/Ext/OK" || message.addr == "/VMC/Ext/T"
}

fn vmc_frame_capture_arg(arg: OscType) -> VmcFrameCaptureArg {
	match arg {
		OscType::String(value) => VmcFrameCaptureArg::String(value),
		OscType::Float(value) => VmcFrameCaptureArg::Float(value),
		OscType::Double(value) => VmcFrameCaptureArg::Double(value),
		OscType::Int(value) => VmcFrameCaptureArg::Int(value),
		OscType::Long(value) => VmcFrameCaptureArg::Long(value),
		OscType::Bool(value) => VmcFrameCaptureArg::Bool(value),
		other => VmcFrameCaptureArg::Other(format!("{other:?}")),
	}
}

fn sanitize_capture_label(label: &str) -> String {
	label
		.chars()
		.map(|ch| {
			if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
				ch.to_ascii_lowercase()
			} else {
				'-'
			}
		})
		.collect::<String>()
		.split('-')
		.filter(|part| !part.is_empty())
		.collect::<Vec<_>>()
		.join("-")
}

fn listen_port_label(listen_addr: &str) -> String {
	listen_addr
		.rsplit_once(':')
		.map(|(_, port)| sanitize_capture_label(port))
		.filter(|value| !value.is_empty())
		.unwrap_or_else(|| "udp".to_string())
}

fn now_unix_ms() -> u64 {
	SystemTime::now()
		.duration_since(SystemTime::UNIX_EPOCH)
		.map(|d| d.as_millis() as u64)
		.unwrap_or(0)
}

#[derive(Debug)]
struct ResizeArgs {
	input: PathBuf,
	output: PathBuf,
	width: u32,
	height: u32,
	background: Rgba<u8>,
}

fn resize_image_command(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_resize_args(raw_args)?;
	let input = absolutize(repo, &args.input);
	let output = absolutize(repo, &args.output);
	resize_letterbox(&input, &output, args.width, args.height, args.background)
}

fn parse_resize_args(raw_args: Vec<OsString>) -> Result<ResizeArgs> {
	let mut input = None;
	let mut output = None;
	let mut width = 320_u32;
	let mut height = 240_u32;
	let mut background = Rgba([0, 0, 0, 255]);
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--input" | "-i" => input = Some(PathBuf::from(next_value(&mut iter, "--input")?)),
			"--output" | "-o" => output = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
			"--width" | "-w" => width = parse_u32(next_value(&mut iter, "--width")?, "--width")?,
			"--height" | "-h" => height = parse_u32(next_value(&mut iter, "--height")?, "--height")?,
			"--background" => background = parse_rgba(&next_value(&mut iter, "--background")?.to_string_lossy())?,
			"--help" => {
				eprintln!("usage: cargo xtask image resize --input in.png --output out.png --width 320 --height 240 [--background 000000]");
				std::process::exit(0);
			}
			other if input.is_none() => input = Some(PathBuf::from(other)),
			other => bail!("unexpected resize argument: {other}"),
		}
	}
	let input = input.context("missing --input")?;
	let output = output.context("missing --output")?;
	if width == 0 || height == 0 {
		bail!("resize dimensions must be non-zero");
	}
	Ok(ResizeArgs {
		input,
		output,
		width,
		height,
		background,
	})
}

fn next_value(iter: &mut impl Iterator<Item = OsString>, name: &str) -> Result<OsString> {
	iter.next().with_context(|| format!("missing {name} value"))
}

fn parse_u32(value: OsString, name: &str) -> Result<u32> {
	value
		.to_string_lossy()
		.parse()
		.with_context(|| format!("invalid {name}: {}", value.to_string_lossy()))
}

fn parse_usize(value: OsString, name: &str) -> Result<usize> {
	value
		.to_string_lossy()
		.parse()
		.with_context(|| format!("invalid {name}: {}", value.to_string_lossy()))
}

fn parse_csv(value: OsString) -> Vec<String> {
	value
		.to_string_lossy()
		.split(',')
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(ToString::to_string)
		.collect()
}

fn parse_rgba(value: &str) -> Result<Rgba<u8>> {
	let hex = value.trim().trim_start_matches('#');
	let bytes = match hex.len() {
		6 => [
			parse_hex_byte(&hex[0..2])?,
			parse_hex_byte(&hex[2..4])?,
			parse_hex_byte(&hex[4..6])?,
			255,
		],
		8 => [
			parse_hex_byte(&hex[0..2])?,
			parse_hex_byte(&hex[2..4])?,
			parse_hex_byte(&hex[4..6])?,
			parse_hex_byte(&hex[6..8])?,
		],
		_ => bail!("background must be RRGGBB or RRGGBBAA: {value}"),
	};
	Ok(Rgba(bytes))
}

fn parse_hex_byte(value: &str) -> Result<u8> {
	u8::from_str_radix(value, 16).with_context(|| format!("invalid hex byte: {value}"))
}

fn resize_letterbox(input: &Path, output: &Path, width: u32, height: u32, background: Rgba<u8>) -> Result<()> {
	let image = image::open(input)
		.with_context(|| format!("failed to open image {}", input.display()))?
		.to_rgba8();
	let (source_width, source_height) = image.dimensions();
	if source_width == 0 || source_height == 0 {
		bail!("input image has zero dimension: {}", input.display());
	}
	let scale = (width as f64 / source_width as f64).min(height as f64 / source_height as f64);
	let resized_width = ((source_width as f64 * scale).round() as u32).max(1).min(width);
	let resized_height = ((source_height as f64 * scale).round() as u32).max(1).min(height);
	let resized = image::imageops::resize(&image, resized_width, resized_height, FilterType::CatmullRom);
	let mut canvas = ImageBuffer::from_pixel(width, height, background);
	let x = (width - resized_width) / 2;
	let y = (height - resized_height) / 2;
	canvas.copy_from(&resized, x, y)?;
	if let Some(parent) = output.parent() {
		fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	}
	canvas
		.save(output)
		.with_context(|| format!("failed to save image {}", output.display()))?;
	Ok(())
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
	if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

fn bazel_path_string(path: &Path) -> String {
	path.to_string_lossy().replace('\\', "/")
}

fn prepend_env_path(prefix: &Path, base: &OsString) -> OsString {
	let mut path = OsString::new();
	path.push(prefix);
	path.push(if cfg!(windows) { ";" } else { ":" });
	path.push(base);
	path
}

fn strip_double_dash(args: Vec<OsString>) -> Vec<OsString> {
	if args.first().is_some_and(|arg| arg == "--") {
		args[1..].to_vec()
	} else {
		args
	}
}

fn run<const N: usize>(cwd: impl AsRef<Path>, program: &str, args: [&str; N]) -> Result<()> {
	let mut cmd = Command::new(resolve_tool(program));
	cmd.current_dir(cwd.as_ref()).args(args);
	run_command(&format!("{} {}", program, args.join(" ")), &mut cmd)
}

fn run_command(label: &str, cmd: &mut Command) -> Result<()> {
	eprintln!("==> {label}");
	let status = cmd
		.stdin(Stdio::null())
		.status()
		.with_context(|| format!("failed to run {label}"))?;
	if !status.success() {
		bail!("{label} failed with {status}");
	}
	Ok(())
}

fn resolve_tool(program: &str) -> OsString {
	if cfg!(windows) {
		match program {
			"npm" => OsString::from("npm.cmd"),
			"npx" => OsString::from("npx.cmd"),
			_ => OsString::from(program),
		}
	} else {
		OsString::from(program)
	}
}

fn repo_root() -> Result<PathBuf> {
	let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
	manifest_dir
		.parent()
		.and_then(Path::parent)
		.map(Path::to_path_buf)
		.context("failed to resolve repository root from CARGO_MANIFEST_DIR")
}

fn print_usage() {
	eprintln!(
		"usage:
  cargo xtask fmt
  cargo xtask check
  cargo xtask test
  cargo xtask frontend build
  cargo xtask core smoke
  cargo xtask core lifecycle-smoke
  cargo xtask core vmc-smoke
  cargo xtask core vmc-mirror-smoke
  cargo xtask core external-vmc-smoke --listen 127.0.0.1:39550 --target 127.0.0.1:39551 --observe 127.0.0.1:39571
  cargo xtask core external-vmc-stability --listen 127.0.0.1:39560 --duration-ms 5000 [--output report.json]
  cargo xtask core external-vmc-compare --listen 127.0.0.1:39560 --duration-ms 5000 [--label wmc]
  cargo xtask core external-ifacialmocap-smoke --listen 192.168.13.13:49983 --target 127.0.0.1:39551 --observe 127.0.0.1:39571
  cargo xtask make-release-package [--version 1.2.3.beta-1] [--output-dir release-packages] [--skip-build] [--keep-staging]
  cargo xtask license-report [--output target/license-report/THIRD_PARTY_DEPENDENCIES.md]
  cargo xtask verify [--skip-frontend] [--skip-rust]
  cargo xtask run-capturer --profile Dev1 [--profile-root PATH] [--release] [--log FILTER] [-- capturer args]
  cargo xtask research penn-action prepare
  cargo xtask research penn-action summary
  cargo xtask research penn-action desktop-config --sequence 0001
  cargo xtask research ffmpeg prepare
  cargo xtask image resize --input in.png --output out.png --width 320 --height 240
  cargo xtask mediapipe native-probe -- --image image.png [probe args]
  cargo xtask mediapipe native-camera-probe -- [--list] [--device camera]
  cargo xtask vmc capture-frame --addr 127.0.0.1:39551 [--collect-ms 25] [--label warudo-sample]
  cargo xtask vmc stability --addr 127.0.0.1:39551 --duration-ms 5000 [--output report.json]
  cargo xtask vmc stability-summary [--dir target/vmc-captures/runs/stability]
  cargo xtask unmf stability --key un-motion/frame --duration-ms 5000 [--output report.json]"
	);
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_rgba_accepts_rgb_and_rgba_hex() {
		assert_eq!(parse_rgba("112233").unwrap(), Rgba([0x11, 0x22, 0x33, 0xff]));
		assert_eq!(parse_rgba("#11223344").unwrap(), Rgba([0x11, 0x22, 0x33, 0x44]));
	}

	#[test]
	fn parse_resize_args_rejects_zero_dimensions() {
		let err = parse_resize_args(vec![
			"--input".into(),
			"in.png".into(),
			"--output".into(),
			"out.png".into(),
			"--width".into(),
			"0".into(),
		])
		.unwrap_err();
		assert!(err.to_string().contains("non-zero"));
	}

	#[test]
	fn parse_run_capturer_args_accepts_profile_and_passthrough() {
		let args = parse_run_capturer_args(vec![
			"--profile".into(),
			"Dev1".into(),
			"--release".into(),
			"--log".into(),
			"info,un_motion_runtime=debug".into(),
			"--".into(),
			"--with-tray".into(),
		])
		.unwrap();
		assert_eq!(args.profile, "Dev1");
		assert!(args.release);
		assert_eq!(args.log.as_deref(), Some("info,un_motion_runtime=debug"));
		assert_eq!(args.passthrough, vec![OsString::from("--with-tray")]);
	}

	#[test]
	fn resolve_capturer_profile_matches_name_case_insensitive() {
		let root = env::temp_dir().join(format!("unmotion-xtask-profile-test-{}", now_unix_ms()));
		let profiles_dir = root.join("profiles");
		fs::create_dir_all(&profiles_dir).unwrap();
		fs::write(profiles_dir.join("dev1.toml"), "id = \"new-profile\"\nname = \"Dev1\"\n").unwrap();

		let profile = resolve_capturer_profile(&root, "dev1").unwrap();
		assert_eq!(profile.id, "new-profile");
		assert_eq!(profile.name, "Dev1");

		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn package_file_version_normalizes_prerelease_dot() {
		assert_eq!(package_file_version("1.2.3.beta-1").unwrap(), "1.2.3-beta-1");
		assert_eq!(package_file_version("1.2.3.rc-2").unwrap(), "1.2.3-rc-2");
	}

	#[test]
	fn package_file_version_rejects_unsafe_characters() {
		assert!(package_file_version("1.2.3/beta-1").is_err());
		assert!(package_file_version("").is_err());
	}

	#[test]
	fn parse_core_smoke_args_defaults_and_accepts_options() {
		let args = parse_core_smoke_args(Vec::new()).unwrap();
		assert!(args.core_exe.is_none());
		assert_eq!(args.timeout, Duration::from_secs(30));
		assert!(!args.keep_temp);

		let args = parse_core_smoke_args(vec![
			"--core-exe".into(),
			"target/release/un-motion-core.exe".into(),
			"--timeout-ms".into(),
			"5000".into(),
			"--keep-temp".into(),
		])
		.unwrap();
		assert_eq!(args.core_exe, Some(PathBuf::from("target/release/un-motion-core.exe")));
		assert_eq!(args.timeout, Duration::from_millis(5000));
		assert!(args.keep_temp);
	}

	#[test]
	fn parse_core_smoke_args_rejects_zero_timeout() {
		let err = parse_core_smoke_args(vec!["--timeout-ms".into(), "0".into()]).unwrap_err();
		assert!(err.to_string().contains("greater than 0"));
	}

	#[test]
	fn parse_core_external_ifacialmocap_smoke_args_accepts_live_bench_addresses() {
		let args = parse_core_external_ifacialmocap_smoke_args(vec![
			"--listen".into(),
			"192.168.13.13:49983".into(),
			"--target".into(),
			"127.0.0.1:39551".into(),
			"--observe".into(),
			"127.0.0.1:39571".into(),
			"--label".into(),
			"ifacialmocap-to-vseeface".into(),
			"--timeout-ms".into(),
			"5000".into(),
		])
		.unwrap();

		assert_eq!(args.listen_addr.to_string(), "192.168.13.13:49983");
		assert_eq!(args.target_addr.to_string(), "127.0.0.1:39551");
		assert_eq!(args.observe_addr.to_string(), "127.0.0.1:39571");
		assert_eq!(args.label, "ifacialmocap-to-vseeface");
		assert_eq!(args.core.timeout, Duration::from_millis(5000));
	}

	#[test]
	fn parse_core_external_vmc_stability_args_accepts_live_bench_options() {
		let args = parse_core_external_vmc_stability_args(vec![
			"--listen".into(),
			"127.0.0.1:39560".into(),
			"--output-bind".into(),
			"127.0.0.1:0".into(),
			"--label".into(),
			"unmotion-wmc".into(),
			"--duration-ms".into(),
			"2500".into(),
			"--output".into(),
			"target/stability/unmotion-wmc.json".into(),
			"--min-samples".into(),
			"3".into(),
			"--mirror".into(),
			"--timeout-ms".into(),
			"5000".into(),
		])
		.unwrap();

		assert_eq!(args.listen_addr.to_string(), "127.0.0.1:39560");
		assert_eq!(args.output_bind_addr.to_string(), "127.0.0.1:0");
		assert_eq!(args.label, "unmotion-wmc");
		assert_eq!(args.duration, Duration::from_millis(2500));
		assert_eq!(args.output, Some(PathBuf::from("target/stability/unmotion-wmc.json")));
		assert_eq!(args.min_samples, 3);
		assert!(args.mirror_correction_enabled);
		assert_eq!(args.core.timeout, Duration::from_millis(5000));
	}

	#[test]
	fn parse_core_external_vmc_compare_args_accepts_live_bench_options() {
		let args = parse_core_external_vmc_compare_args(vec![
			"--listen".into(),
			"127.0.0.1:39560".into(),
			"--output-bind".into(),
			"127.0.0.1:0".into(),
			"--label".into(),
			"wmc".into(),
			"--duration-ms".into(),
			"2500".into(),
			"--output-dir".into(),
			"target/stability".into(),
			"--min-samples".into(),
			"3".into(),
			"--mirror".into(),
			"--timeout-ms".into(),
			"5000".into(),
		])
		.unwrap();

		assert_eq!(args.listen_addr.to_string(), "127.0.0.1:39560");
		assert_eq!(args.output_bind_addr.to_string(), "127.0.0.1:0");
		assert_eq!(args.label, "wmc");
		assert_eq!(args.duration, Duration::from_millis(2500));
		assert_eq!(args.output_dir, PathBuf::from("target/stability"));
		assert_eq!(args.min_samples, 3);
		assert!(args.mirror_correction_enabled);
		assert_eq!(args.core.timeout, Duration::from_millis(5000));
	}

	#[test]
	fn release_artifact_lists_use_runtime_relative_paths() {
		let repo = repo_root().unwrap();
		let native = release_native_artifacts(&repo).unwrap();
		let models = release_model_artifacts(&repo).unwrap();
		assert!(native.iter().any(|artifact| artifact.package_path == "un-motion-mediapipe.dll"));
		assert!(native.iter().any(|artifact| artifact.package_path == "opencv_world3410.dll"));
		assert!(
			models
				.iter()
				.any(|artifact| artifact.package_path == "models/holistic_landmarker.task")
		);
	}

	#[test]
	fn release_manifest_lists_all_packaged_executables() {
		let manifest = release_package_manifest("un-motion-0.1.0", "0.1.0", &[], &[]);
		assert!(manifest.contains(&format!("launcher: {}", supervisor_launcher_name())));
		assert!(manifest.contains(&format!("core_executable: un-motion-core{}", env::consts::EXE_SUFFIX)));
		assert!(manifest.contains(&format!("supervisor_executable: un-motion-supervisor{}", env::consts::EXE_SUFFIX,)));
		assert!(manifest.contains(&format!("capturer_executable: un-motion-capturer{}", env::consts::EXE_SUFFIX,)));
	}

	#[test]
	fn supervisor_launcher_invokes_supervisor_exe_and_checks_capturer() {
		let launcher = supervisor_launcher_script();
		assert!(launcher.contains(&format!("un-motion-supervisor{}", env::consts::EXE_SUFFIX)));
		assert!(launcher.contains(&format!("un-motion-capturer{}", env::consts::EXE_SUFFIX)));
		if cfg!(windows) {
			assert!(launcher.contains("start \"\""));
		}
	}

	#[test]
	fn parse_vmc_frame_capture_args_defaults_to_un_motion_debug_port() {
		let args = parse_vmc_frame_capture_args(Vec::new()).unwrap();
		assert_eq!(args.listen_addr, "127.0.0.1:39551");
		assert_eq!(args.output_dir, PathBuf::from("target/vmc-captures"));
		assert_eq!(args.collect_after_boundary, Duration::ZERO);
	}

	#[test]
	fn parse_vmc_frame_capture_args_accepts_port_and_rejects_zero_timeout() {
		let args = parse_vmc_frame_capture_args(vec!["--port".into(), "39550".into(), "--collect-ms".into(), "25".into()]).unwrap();
		assert_eq!(args.listen_addr, "127.0.0.1:39550");
		assert_eq!(args.collect_after_boundary, Duration::from_millis(25));

		let err = parse_vmc_frame_capture_args(vec!["--timeout-ms".into(), "0".into()]).unwrap_err();
		assert!(err.to_string().contains("greater than 0"));
	}

	#[test]
	fn parse_vmc_stability_args_accepts_output_and_rejects_zero_min_samples() {
		let args = parse_vmc_stability_args(vec![
			"--port".into(),
			"39560".into(),
			"--duration-ms".into(),
			"2500".into(),
			"--source-id".into(),
			"wmc".into(),
			"--output".into(),
			"target/stability/wmc.json".into(),
			"--min-samples".into(),
			"3".into(),
		])
		.unwrap();
		assert_eq!(args.listen_addr, "127.0.0.1:39560");
		assert_eq!(args.duration, Duration::from_millis(2500));
		assert_eq!(args.source_id, "wmc");
		assert_eq!(args.output, Some(PathBuf::from("target/stability/wmc.json")));
		assert_eq!(args.min_samples, 3);

		let err = parse_vmc_stability_args(vec!["--min-samples".into(), "0".into()]).unwrap_err();
		assert!(err.to_string().contains("greater than 0"));
	}

	#[test]
	fn parse_vmc_stability_summary_args_accepts_reports_and_rejects_zero_top() {
		let args = parse_vmc_stability_summary_args(vec![
			"--dir".into(),
			"target/stability".into(),
			"--report".into(),
			"warudo.json".into(),
			"wmc.json".into(),
			"--output".into(),
			"summary.md".into(),
			"--top".into(),
			"4".into(),
		])
		.unwrap();
		assert_eq!(args.reports_dir, PathBuf::from("target/stability"));
		assert_eq!(args.reports, vec![PathBuf::from("warudo.json"), PathBuf::from("wmc.json")]);
		assert_eq!(args.output, Some(PathBuf::from("summary.md")));
		assert_eq!(args.top, 4);

		let err = parse_vmc_stability_summary_args(vec!["--top".into(), "0".into()]).unwrap_err();
		assert!(err.to_string().contains("greater than 0"));
	}

	#[test]
	fn parse_unmf_stability_args_accepts_key_mode_and_output() {
		let args = parse_unmf_stability_args(vec![
			"--key".into(),
			"un-motion/frame".into(),
			"--topic-mode".into(),
			"by-stream-id".into(),
			"--duration-ms".into(),
			"2500".into(),
			"--source-id".into(),
			"dev1-unmf".into(),
			"--output".into(),
			"target/stability/dev1-unmf.json".into(),
			"--min-samples".into(),
			"3".into(),
		])
		.unwrap();

		assert_eq!(args.base_key_expr, "un-motion/frame");
		assert_eq!(args.topic_mode, TopicMode::ByStreamId);
		assert_eq!(args.duration, Duration::from_millis(2500));
		assert_eq!(args.source_id, "dev1-unmf");
		assert_eq!(args.output, Some(PathBuf::from("target/stability/dev1-unmf.json")));
		assert_eq!(args.min_samples, 3);
	}

	#[test]
	fn parse_unmf_stability_args_rejects_zero_min_samples() {
		let err = parse_unmf_stability_args(vec!["--min-samples".into(), "0".into()]).unwrap_err();
		assert!(err.to_string().contains("greater than 0"));
	}

	#[test]
	fn parses_mediapipe_quality_note_tokens() {
		let (part, score, reason) = parse_mediapipe_quality_token("left_hand=0.700(hand_ik)").expect("quality token");

		assert_eq!(part, "left_hand");
		assert!((score - 0.7).abs() < f32::EPSILON);
		assert_eq!(reason, "hand_ik");
	}

	#[test]
	fn patch_windows_opencv_repository_path_rewrites_existing_path() {
		let workspace = r#"new_local_repository(
    name = "windows_opencv",
    build_file = "@//third_party:opencv_windows.BUILD",
    path = "C:/Users/the/tmp/UNMotion/third_party/opencv/opencv/build",
)
"#;
		let patched = patch_windows_opencv_repository_path(workspace, "C:/Users/the/tmp/un-motion/third_party/opencv/opencv/build");

		assert!(patched.contains(r#"path = "C:/Users/the/tmp/un-motion/third_party/opencv/opencv/build","#));
		assert!(!patched.contains("UNMotion/third_party"));
	}

	#[test]
	fn patch_stablehlo_reduce_window_removes_unicode_ascii_art_comment() {
		let root = env::temp_dir().join(format!("unmotion-xtask-stablehlo-test-{}", now_unix_ms()));
		fs::create_dir_all(&root).unwrap();
		let path = root.join("stablehlo_reduce_window.cc");
		fs::write(
			&path,
			"// before\n// For instance: the following window has a [2, 2] shape and [2, 3] dilations.\n//\n// ┌────┐\ntemplate <class Op, class Type>\nvoid f() {}\n",
		)
		.unwrap();

		patch_stablehlo_reduce_window_for_msvc(&path).unwrap();
		let patched = fs::read_to_string(&path).unwrap();
		let _ = fs::remove_dir_all(&root);

		assert!(patched.contains("template <class Op, class Type>"));
		assert!(!patched.contains("For instance:"));
		assert!(!patched.contains("┌"));
	}

	#[test]
	fn vmc_stability_report_deserializes_without_blendshape_step_fields() {
		let report: VmcStabilityReport = serde_json::from_str(
			r#"{
				"listenAddr": "127.0.0.1:39560",
				"sourceId": "legacy",
				"durationMs": 1000,
				"packets": 1,
				"decodedFrames": 1,
				"vmcPayloadFrames": 1,
				"decodeErrors": 0,
				"rootSamples": 0,
				"boneSamples": 1,
				"blendshapeSamples": 1,
				"blendApplyFrames": 1,
				"tracks": [],
				"worstPositionStep": [],
				"worstRotationStep": []
			}"#,
		)
		.unwrap();

		assert!(report.blendshape_tracks.is_empty());
		assert!(report.worst_blendshape_step.is_empty());
	}
}
