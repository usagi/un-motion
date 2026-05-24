use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
	println!("cargo:rerun-if-changed=../src");
	println!("cargo:rerun-if-changed=../package.json");
	println!("cargo:rerun-if-changed=../package-lock.json");
	println!("cargo:rerun-if-changed=../scripts/sync-sound-assets.mjs");
	println!("cargo:rerun-if-changed=../vite.config.ts");
	println!("cargo:rerun-if-changed=../svelte.config.js");
	println!("cargo:rerun-if-changed=../tsconfig.json");
	println!("cargo:rerun-if-changed=../index.html");
	println!("cargo:rerun-if-changed=../../../crates/un-motion-capturer/src");
	println!("cargo:rerun-if-changed=../../../crates/un-motion-core/src");
	println!("cargo:rerun-if-changed=../../../crates/un-motion-runtime/src");
	println!("cargo:rerun-if-changed=../../../crates/un-motion-engine-mediapipe-native/src");
	println!("cargo:rerun-if-changed=../../../crates/un-motion-engine-mediapipe-post-process/src");
	println!("cargo:rerun-if-changed=../../../crates/un-motion-pipeline/src");
	println!("cargo:rerun-if-changed=../../../crates/un-motion-output-vmc/src");
	println!("cargo:rerun-if-changed=../../../assets/sounds");
	println!("cargo:rerun-if-env-changed=UN_MOTION_FRONTEND_PREBUILT");
	println!("cargo:rerun-if-env-changed=UN_MOTION_SKIP_CAPTURER_BOOTSTRAP");

	let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
	let frontend_dir = manifest_dir.parent().expect("src-tauri parent is frontend dir").to_path_buf();
	let dist_index = frontend_dir.join("dist").join("index.html");
	let prebuilt = std::env::var_os("UN_MOTION_FRONTEND_PREBUILT").is_some();
	let release = std::env::var("PROFILE").as_deref() == Ok("release");

	let should_build = if prebuilt {
		false
	} else if release {
		// release では UN Avatar 同様に常に最新の frontend を embed する。
		true
	} else {
		// debug では既存の `dist/index.html` が vite 生成物 (assets バンドル参照
		// を持つ) なら skip、placeholder のみなら build を走らせる。これで
		// fresh clone 後の `cargo run -p un-motion-supervisor` 一発で GUI が
		// 起動できるようにする。
		!dist_has_real_bundle(&dist_index)
	};

	if should_build {
		build_frontend(&frontend_dir);
	}

	// Phase E core fix: `cargo run --release` 一発で Supervisor と Capturer 双方が
	// 最新コードでビルドされるよう、Supervisor の release build.rs から
	// `un-motion-capturer --release` を bootstrap target-dir 経由でビルドし、
	// 出来た exe を Supervisor exe の隣 (`target/release/`) にコピーする。
	// 旧来は workspace の `default-members = un-motion-supervisor` だけだったので、
	// Capturer source が変わっても `target/release/un-motion-capturer.exe` は古い
	// バイナリのまま残り、Supervisor が古い Capturer (auto-start 機能なし、など) を
	// 起動して沈黙故障する事故が発生していた。
	if release && !prebuilt && std::env::var_os("UN_MOTION_SKIP_CAPTURER_BOOTSTRAP").is_none() {
		bootstrap_capturer(&manifest_dir);
	}

	tauri_build::build();
}

fn bootstrap_capturer(manifest_dir: &Path) {
	// manifest_dir = apps/un-motion-supervisor/src-tauri → ../../../ で workspace root
	let repo = manifest_dir
		.parent()
		.and_then(Path::parent)
		.and_then(Path::parent)
		.expect("src-tauri is under apps/un-motion-supervisor/src-tauri")
		.to_path_buf();
	let bootstrap_target = repo.join("target").join("bootstrap");
	let profile_dir = cargo_profile_dir();

	println!("cargo:warning=bootstrap: building un-motion-capturer release binary");
	let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
	let status = Command::new(cargo)
		.current_dir(&repo)
		// 子プロセスの再帰 bootstrap を抑止する (Supervisor を再ビルドしないが念のため)。
		.env("UN_MOTION_SKIP_CAPTURER_BOOTSTRAP", "1")
		.args([
			"build",
			"-p",
			"un-motion-capturer",
			"--bin",
			"un-motion-capturer",
			"--release",
			"--target-dir",
		])
		.arg(&bootstrap_target)
		.status()
		.expect("spawn cargo build un-motion-capturer");
	if !status.success() {
		panic!("cargo build un-motion-capturer failed with {status}");
	}

	println!("cargo:warning=bootstrap: copying un-motion-capturer next to un-motion-supervisor");
	let exe_file = exe_name("un-motion-capturer");
	let from = bootstrap_target.join("release").join(&exe_file);
	let to = profile_dir.join(&exe_file);
	match std::fs::copy(&from, &to) {
		Ok(_) => cleanup_versioned_capturer_sidecars(&profile_dir),
		Err(error) if cfg!(windows) && error.raw_os_error() == Some(32) => {
			let fallback = sidecar_capturer_path(&profile_dir);
			if let Some(parent) = fallback.parent() {
				std::fs::create_dir_all(parent).unwrap_or_else(|err| panic!("create fallback capturer dir {}: {err}", parent.display()));
			}
			std::fs::copy(&from, &fallback)
				.unwrap_or_else(|err| panic!("copy {} to fallback {}: {err}", from.display(), fallback.display()));
			println!(
				"cargo:warning=bootstrap: {} was locked; wrote fresh capturer sidecar {}",
				to.display(),
				fallback.display()
			);
		}
		Err(error) => panic!("copy {} to {}: {error}", from.display(), to.display()),
	}
}

fn cleanup_versioned_capturer_sidecars(profile_dir: &Path) {
	let dir = profile_dir.join("runtimes");
	let Ok(entries) = std::fs::read_dir(&dir) else {
		return;
	};
	let prefix = "un-motion-capturer-";
	let suffix = exe_suffix();
	for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
		let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
			continue;
		};
		if name.starts_with(prefix) && name.ends_with(suffix) {
			let _ = std::fs::remove_file(path);
		}
	}
}

fn sidecar_capturer_path(profile_dir: &Path) -> PathBuf {
	let millis = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_millis())
		.unwrap_or(0);
	profile_dir
		.join("runtimes")
		.join(format!("un-motion-capturer-{millis}{}", exe_suffix()))
}

fn cargo_profile_dir() -> PathBuf {
	// OUT_DIR = target/<profile>/build/<crate>-<hash>/out → ../../.. が target/<profile>
	let mut path = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
	for _ in 0..3 {
		path = path.parent().expect("OUT_DIR ancestor under target/<profile>").to_path_buf();
	}
	path
}

fn exe_name(name: &str) -> String {
	if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn exe_suffix() -> &'static str {
	if cfg!(windows) { ".exe" } else { "" }
}

/// `dist/index.html` が vite build が出した実体 (assets バンドル参照あり) か
/// どうかを判定する。Phase D 初期に置いた placeholder と区別する。
fn dist_has_real_bundle(index: &Path) -> bool {
	let Ok(text) = std::fs::read_to_string(index) else {
		return false;
	};
	text.contains("/assets/") && text.contains("<script") && text.contains("</script>")
}

fn build_frontend(frontend_dir: &Path) {
	let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
	let node_modules = frontend_dir.join("node_modules");
	if !node_modules.exists() {
		println!(
			"cargo:warning=`{}/node_modules` not found, running `npm install` (first-run only, may take a while)",
			frontend_dir.display()
		);
		let status = Command::new(npm).current_dir(frontend_dir).args(["install"]).status();
		match status {
			Ok(status) if !status.success() => panic!("`npm install` failed for supervisor frontend"),
			Ok(_) => {}
			Err(error) => panic!("failed to spawn `npm install` for supervisor frontend: {error}"),
		}
	}
	println!(
		"cargo:warning=Running `npm run build` for un-motion-supervisor frontend ({})",
		frontend_dir.display()
	);
	let status = Command::new(npm).current_dir(frontend_dir).args(["run", "build"]).status();
	match status {
		Ok(status) if !status.success() => panic!("`npm run build` failed for supervisor frontend"),
		Ok(_) => {}
		Err(error) => panic!("failed to spawn `npm run build` for supervisor frontend: {error}"),
	}
}
