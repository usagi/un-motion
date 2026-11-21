use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use super::{resolve_tool, run_command};

#[derive(Debug)]
pub(super) struct Args {
	pub(super) release: bool,
	pub(super) target_dir: Option<PathBuf>,
}

pub(super) fn run_build(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_args(raw_args)?;
	build_binaries_in(repo, args.release, args.target_dir.as_deref())
}

pub(super) fn parse_args(raw_args: Vec<OsString>) -> Result<Args> {
	let mut release = false;
	let mut target_dir = None;
	let mut iter = raw_args.into_iter();
	while let Some(arg) = iter.next() {
		match arg.to_string_lossy().as_ref() {
			"--release" => release = true,
			"--target-dir" => target_dir = Some(PathBuf::from(super::next_value(&mut iter, "--target-dir")?)),
			"--help" | "-h" | "help" => {
				print_usage();
				std::process::exit(0);
			}
			other => bail!("unknown build option: {other}"),
		}
	}
	Ok(Args { release, target_dir })
}

pub(super) fn build_binaries_in(repo: &Path, release: bool, target_dir: Option<&Path>) -> Result<()> {
	if release {
		for key in ["TAURI_CONFIG", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET"] {
			if std::env::var_os(key).is_some() {
				bail!("{key} overrides the standard release frontend/output path; clear it before building");
			}
		}
	}
	build_normal_frontend(repo)?;
	let target = target_dir.map(|path| super::absolutize(repo, path));
	cargo_build_package(repo, release, "un-motion-core", "un-motion-core", target.as_deref())?;
	cargo_build_package(repo, release, "un-motion-capturer", "un-motion-capturer", target.as_deref())?;
	cargo_build_package(repo, release, "un-motion-supervisor", "un-motion-supervisor", target.as_deref())?;
	if release {
		record_normal_build(&target.unwrap_or_else(|| repo.join("target")).join("release"))?;
	}
	Ok(())
}

pub(super) const NORMAL_BUILD_RECEIPT: &str = "un-motion-build.json";

fn artifact_hashes(directory: &Path) -> Result<serde_json::Value> {
	let mut files = serde_json::Map::new();
	for binary in ["un-motion-core", "un-motion-capturer", "un-motion-supervisor"] {
		let name = format!("{binary}{}", std::env::consts::EXE_SUFFIX);
		let mut file = fs::File::open(directory.join(&name))?;
		let mut hash = Sha256::new();
		let mut buffer = [0u8; 65536];
		loop {
			let count = file.read(&mut buffer)?;
			if count == 0 {
				break;
			}
			hash.update(&buffer[..count]);
		}
		files.insert(name, format!("{:x}", hash.finalize()).into());
	}
	Ok(serde_json::Value::Object(files))
}

fn record_normal_build(directory: &Path) -> Result<()> {
	let receipt = serde_json::json!({"schema_version":1,"ui_mode":"normal","artifacts":artifact_hashes(directory)?});
	fs::write(directory.join(NORMAL_BUILD_RECEIPT), serde_json::to_vec_pretty(&receipt)?)?;
	Ok(())
}

pub(super) fn verify_normal_build(directory: &Path) -> Result<()> {
	let path = directory.join(NORMAL_BUILD_RECEIPT);
	let bytes = fs::read(&path).map_err(|error| {
		anyhow::anyhow!(
			"normal UI build receipt unavailable ({}): {error}; run cargo xtask build --release",
			path.display()
		)
	})?;
	let receipt: serde_json::Value = serde_json::from_slice(&bytes)?;
	if receipt["schema_version"] != 1 || receipt["ui_mode"] != "normal" || receipt["artifacts"] != artifact_hashes(directory)? {
		bail!("release binaries differ from normal UI build receipt; run cargo xtask build --release");
	}
	Ok(())
}

fn normal_frontend_command(repo: &Path) -> Command {
	let mut command = Command::new(resolve_tool("npm"));
	command
		.current_dir(repo.join("apps/un-motion-supervisor"))
		.env("VITE_UN_DESIGN_PREVIEW", "0")
		.args(["run", "build"]);
	command
}

pub(super) fn build_normal_frontend(repo: &Path) -> Result<()> {
	run_command("npm run build (normal UI)", &mut normal_frontend_command(repo))
}

#[cfg(test)]
mod tests {
	#[test]
	fn explicit_target_directory_is_preserved_and_missing_value_rejected() {
		let args = super::parse_args(vec![
			"--release".into(),
			"--target-dir".into(),
			"target/candidate with spaces".into(),
		])
		.unwrap();
		assert!(args.release);
		assert_eq!(args.target_dir.unwrap(), std::path::PathBuf::from("target/candidate with spaces"));
		assert!(super::parse_args(vec!["--target-dir".into()]).is_err());
	}
	#[test]
	fn release_receipt_rejects_missing_preview_and_mixed_binaries() {
		let root = std::env::temp_dir().join(format!(
			"unm-receipt-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		std::fs::create_dir(&root).unwrap();
		for name in ["un-motion-core", "un-motion-capturer", "un-motion-supervisor"] {
			std::fs::write(root.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)), name).unwrap();
		}
		assert!(super::verify_normal_build(&root).is_err());
		super::record_normal_build(&root).unwrap();
		super::verify_normal_build(&root).unwrap();
		let path = root.join(super::NORMAL_BUILD_RECEIPT);
		let original = std::fs::read(&path).unwrap();
		let mut preview: serde_json::Value = serde_json::from_slice(&original).unwrap();
		preview["ui_mode"] = "preview".into();
		std::fs::write(&path, serde_json::to_vec(&preview).unwrap()).unwrap();
		assert!(super::verify_normal_build(&root).is_err());
		std::fs::write(&path, &original).unwrap();
		let capturer = root.join(format!("un-motion-capturer{}", std::env::consts::EXE_SUFFIX));
		std::fs::write(&capturer, "other build").unwrap();
		assert!(super::verify_normal_build(&root).is_err());
		std::fs::remove_file(capturer).unwrap();
		assert!(super::verify_normal_build(&root).is_err());
		std::fs::remove_dir_all(root).unwrap();
	}
	#[test]
	fn native_build_explicitly_selects_normal_frontend() {
		let command = super::normal_frontend_command(std::path::Path::new("repo"));
		assert!(
			command
				.get_envs()
				.any(|(key, value)| key == "VITE_UN_DESIGN_PREVIEW" && value == Some(std::ffi::OsStr::new("0")))
		);
		assert_eq!(command.get_args().collect::<Vec<_>>(), ["run", "build"]);
		assert_eq!(
			command.get_current_dir(),
			Some(std::path::Path::new("repo/apps/un-motion-supervisor"))
		);
	}
}

fn cargo_build_package(repo: &Path, release: bool, package: &str, bin: &str, target_dir: Option<&Path>) -> Result<()> {
	let mut cmd = Command::new(resolve_tool("cargo"));
	cmd.current_dir(repo).arg("build");
	if let Some(path) = target_dir {
		cmd.arg("--target-dir").arg(path);
	}
	if package == "un-motion-supervisor" {
		// Already generated explicitly as normal UI above; do not rebuild it
		// with the parent shell's preview environment from build.rs.
		cmd.env("UN_MOTION_FRONTEND_PREBUILT", "1");
	}
	if release {
		cmd.arg("--release");
	}
	cmd.args(["-p", package, "--bin", bin]);
	run_command(
		&format!("cargo build{} -p {package} --bin {bin}", if release { " --release" } else { "" }),
		&mut cmd,
	)
}

fn print_usage() {
	eprintln!("usage: cargo xtask build [--release] [--target-dir PATH]");
}
