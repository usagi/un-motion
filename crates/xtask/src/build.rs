use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use super::{resolve_tool, run, run_command};

#[derive(Debug)]
pub(super) struct Args {
	pub(super) release: bool,
}

pub(super) fn run_build(repo: &Path, raw_args: Vec<OsString>) -> Result<()> {
	let args = parse_args(raw_args)?;
	build_binaries(repo, args.release)
}

pub(super) fn parse_args(raw_args: Vec<OsString>) -> Result<Args> {
	let mut release = false;
	for arg in raw_args {
		match arg.to_string_lossy().as_ref() {
			"--release" => release = true,
			"--help" | "-h" | "help" => {
				print_usage();
				std::process::exit(0);
			}
			other => bail!("unknown build option: {other}"),
		}
	}
	Ok(Args { release })
}

pub(super) fn build_binaries(repo: &Path, release: bool) -> Result<()> {
	run(repo.join("apps/un-motion-supervisor"), "npm", ["run", "build"])?;
	cargo_build_package(repo, release, "un-motion-core", "un-motion-core")?;
	cargo_build_package(repo, release, "un-motion-capturer", "un-motion-capturer")?;
	cargo_build_package(repo, release, "un-motion-supervisor", "un-motion-supervisor")?;
	Ok(())
}

fn cargo_build_package(repo: &Path, release: bool, package: &str, bin: &str) -> Result<()> {
	let mut cmd = Command::new(resolve_tool("cargo"));
	cmd.current_dir(repo).arg("build");
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
	eprintln!("usage: cargo xtask build [--release]");
}
