//! `un-motion-capturer`: 入力ソース (MediaPipe Native / iFacialMocap / 外部 VMC / UNMotion) を
//! `UNMotionFrame v1.1` と VMC で配信する Capturer プロセス。
//!
//! UN Motion Supervisor (`un-motion-supervisor`) から spawn される想定で、自分の HTTP API
//! を `--bind` で指定された port (既定 0 = OS 割り当て) で listen し、stdout に `listening
//! on <addr>` を出力する。Supervisor 側はこの行をパースして HTTP client から制御する。
//!
//! Phase D 時点では既存の `un-motion-core::run_api_server` をそのまま起動する thin
//! wrapper。tray は持たず、終了は HTTP `POST /api/core/exit` または SIGTERM (Ctrl+C)
//! でクリーンに行う。
//!
//! Phase E で named pipe IPC に切り替える可能性もあるが、HTTP/SSE は既存資産で十分
//! 機能するためまずは HTTP で揃える。

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Parser;
use serde::Serialize;
use un_motion_core::{
	CoreApiConfig, CoreControlState, CoreProfileDocument, CoreProfileDocumentProfile, CoreProfileDocumentStore, TrayOptions,
	run_api_server, run_core_with_tray,
};
#[cfg(windows)]
use un_motion_input_webcam_directshow::{DirectShowCaptureConfig, DirectShowWebcamBackend, WebcamCaptureBackend};

#[derive(Debug, Parser)]
#[command(name = "un-motion-capturer", about = "UN Motion Capturer process")]
struct Cli {
	/// HTTP API bind アドレス (例: 127.0.0.1:0)。0 を指定すると OS が空きポートを割り当て、
	/// listen 後に stdout に "listening on <addr>" を 1 行出力する。
	#[arg(long, default_value = "127.0.0.1:0")]
	bind: String,

	/// non-loopback (LAN 経由) アクセスを許可する。デフォルトは false (loopback のみ)。
	#[arg(long, default_value_t = false)]
	allow_non_loopback: bool,

	/// HTTP API worker threads for this Capturer process.
	///
	/// Values are clamped to 1..=system logical cores. The default is 2 to avoid
	/// multiplying Actix's CPU-count default across multiple Capturer processes.
	#[arg(long = "api-workers", default_value_t = un_motion_core::DEFAULT_API_WORKER_THREADS)]
	api_workers: usize,

	/// Profile root ディレクトリ (`conf.toml` と `profiles/*.toml` を含む)。
	/// 省略時は workspace の標準位置 (`conf.toml` を ancestors から探す) を使用。
	#[arg(long)]
	profile_root: Option<PathBuf>,

	/// HTTP API を bind する直前に `selected_profile_id` で runtime を auto-start する。
	/// 既定は true (un-motion-capturer の本来の用途。Supervisor / 単体 `cargo run`
	/// のどちらの経路でも「起動 = 推論開始」を実現する)。
	/// debug 時に API server だけ立ち上げて runtime を後で手動 start したいときは
	/// `--no-auto-start` を渡す。
	#[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
	auto_start: bool,

	/// 起動時に auto-start する profile id。Supervisor は profile ごとの Capturer を
	/// spawn するときにこれを渡し、共有 user store の active profile を変えずに
	/// この Capturer 内だけの selected profile として扱う。
	#[arg(long)]
	active_profile: Option<String>,

	/// システムトレイ常駐を有効化する (Windows のみ)。動作中はタスクトレイにアイコンが
	/// 出てホバー / コンテキストメニューから PID 確認と Supervisor Console 起動 / Exit
	/// が行える。Supervisor から spawn する場合は既定 ON、`cargo run -p un-motion-capturer`
	/// で単体起動する場合は `--with-tray` を明示する。
	///
	/// 内部的には `un-motion-core::run_core_with_tray` が main thread を握り、API server
	/// は別 thread で動かす。非 Windows 環境では tray 実装が無いため、このフラグが ON
	/// でも警告を出して通常 mode で起動する。
	#[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
	with_tray: bool,

	/// `--with-tray` 時にコンテキストメニュー "Open Supervisor Console" から起動する
	/// Supervisor の exe path。`None` の場合は実行ファイルと同じディレクトリにある
	/// `un-motion-supervisor[.exe]` を自動探索する。
	#[arg(long)]
	supervisor_exe: Option<PathBuf>,

	/// Experimental: selected profile の DirectShow/ccap-rs 入力を 1 秒だけ取得し、
	/// MediaPipe に渡る前の RGB frame 輝度揺れを JSON で出力して終了する。
	#[arg(long, default_value_t = false)]
	experimental_flicker_test: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlickerTestSummary {
	profile_id: String,
	profile_name: String,
	device: String,
	requested_format: String,
	actual_format: String,
	frames: u32,
	elapsed_ms: f32,
	avg_fps: f32,
	luma_mean_avg: f32,
	luma_mean_min: f32,
	luma_mean_max: f32,
	luma_mean_range: f32,
	luma_mean_std_dev: f32,
	luma_delta_rms: f32,
	luma_delta_max: f32,
	frame_diff_avg: f32,
	frame_diff_max: f32,
	dominant_hz: f32,
	dominant_amplitude: f32,
}

fn main() -> anyhow::Result<()> {
	// `UN_MOTION_LOG` 環境変数で log filter を上書き可能。
	// 既定は `info,un_motion_runtime=debug` で、Phase E e2e Step B のために
	// Zenoh / VMC output worker の送信頻度 (`un_motion_runtime::{vmc,zenoh}_output`) を
	// debug で stderr に流す。
	//
	// 重要: 出力先を **明示的に stderr** にする。`tracing_subscriber::fmt()` の既定
	// writer は stdout だが、Supervisor から spawn された Capturer の stdout は
	// `Stdio::null()` で握り潰され、stderr のみが `Stdio::piped()` で読まれて GUI の
	// 「Capturer #N stderr」欄に集約される。これを取り違えると GUI 上は「何も
	// 出ていない」沈黙故障に見える (Phase E debug で実際に踏んだ)。
	// 既定 `info`: VMC 受信 engine の 1 frame ごとの DEBUG (約 280 lines/sec) は
	// Supervisor GUI の stderr ring buffer を高速ローテートさせ可読性を著しく下げる
	// ため除外する。詳細トレースが必要なときは `UN_MOTION_LOG` 環境変数で opt-in:
	//   set UN_MOTION_LOG=info,un_motion_runtime=debug
	let filter =
		tracing_subscriber::EnvFilter::try_from_env("UN_MOTION_LOG").unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
	// `with_ansi(false)`: Supervisor は Capturer の stderr を `Stdio::piped()` で
	// 受け取り、そのまま Tauri WebView (HTML) のテキストノードに流すため、
	// 既定 (TTY 検出ベース) で ANSI 色エスケープシーケンスが出力されていると
	// `[2m...[0m`, `[32m INFO[0m` のような目視ノイズになる (Phase E debug で
	// ユーザー報告)。GUI 表示先には色情報を出さないと決め打ちして無効化する。
	tracing_subscriber::fmt()
		.with_env_filter(filter)
		.with_target(true)
		.with_level(true)
		.with_writer(std::io::stderr)
		.with_ansi(false)
		.compact()
		.init();

	let cli = Cli::parse();
	let bind_addr: SocketAddr = cli.bind.parse().with_context(|| format!("invalid --bind value: {}", cli.bind))?;
	let api_config = CoreApiConfig {
		bind_addr,
		allow_non_loopback: cli.allow_non_loopback,
		tray_enabled: false,
		api_worker_threads: un_motion_core::normalize_api_worker_threads(cli.api_workers),
		auto_start_runtime: cli.auto_start,
	};

	let state = if let Some(root) = cli.profile_root.as_ref() {
		// CLI 明示指定 (テスト / ポータブル運用) は seeding せずそのまま使う。
		let store = CoreProfileDocumentStore::from_root(root);
		let mut document = store.load();
		apply_cli_active_profile(&mut document, cli.active_profile.as_deref());
		if cli.experimental_flicker_test {
			run_experimental_flicker_test(&document)?;
			return Ok(());
		}
		CoreControlState::from_profile_document(document, Some(store))
	} else {
		// Phase E settings policy: ユーザーディレクトリを共有しつつ初回起動時のみ
		// bundled templates をコピーする。Supervisor も Capturer も同じ helper を
		// 使う設計だが、Capturer を単体起動 (`un-motion-capturer.exe`) した場合に
		// 備えて Capturer 側でも seeding を試みる (`seed_from_templates` は idempotent)。
		let store = CoreProfileDocumentStore::from_user_dir(user_config_dir());
		if let Some(template_dir) = bundled_template_profiles_dir()
			&& let Err(error) = store.seed_from_templates(&template_dir)
		{
			tracing::warn!(
				template_dir = %template_dir.display(),
				%error,
				"failed to seed user profile dir from bundled templates (continuing with whatever is on disk)",
			);
		}
		let mut document = store.load();
		apply_cli_active_profile(&mut document, cli.active_profile.as_deref());
		if cli.experimental_flicker_test {
			run_experimental_flicker_test(&document)?;
			return Ok(());
		}
		CoreControlState::from_profile_document(document, Some(store))
	};

	let pid = std::process::id();
	tracing::info!(
		?bind_addr,
		allow_non_loopback = cli.allow_non_loopback,
		auto_start = cli.auto_start,
		active_profile = cli.active_profile.as_deref().unwrap_or("-"),
		with_tray = cli.with_tray,
		pid,
		"starting un-motion-capturer",
	);

	if cli.with_tray && cfg!(target_os = "windows") {
		// Windows tray 経路: main thread を tray (tao event loop) が握る。
		// API server は内部で `actix_web::rt::System::new().block_on(...)` を別 thread で
		// spawn する。`report_bind_addr` 相当の listening 通知は ここで先出しする
		// (Supervisor は `/healthz` で待つので tao event loop 開始前で十分)。
		report_bind_addr_sync(bind_addr);
		let tray_options = build_capturer_tray_options(pid, cli.supervisor_exe.as_deref())?;
		run_core_with_tray(api_config, state, tray_options)?;
		return Ok(());
	}

	if cli.with_tray && !cfg!(target_os = "windows") {
		tracing::warn!("--with-tray is currently only supported on Windows; falling back to console mode");
	}

	let runtime = tokio::runtime::Builder::new_multi_thread()
		.worker_threads(api_config.normalized_api_worker_threads())
		.enable_all()
		.build()?;
	runtime.block_on(async move {
		report_bind_addr(bind_addr).await;
		run_api_server(api_config, state).await
	})?;
	Ok(())
}

/// `--with-tray` 時に組み立てる TrayOptions。
///
/// メニュー構成:
/// - tooltip: `UN Motion Capturer (PID xxxxx)` (タスクトレイホバー時)
/// - "Open Supervisor Console": `supervisor_exe` を `Command::spawn`
/// - "Exit Capturer (PID xxxxx)": tao event loop を終了して main thread を解放
///
/// `open_on_startup = false` を指定して Capturer 起動時に Supervisor を勝手に立ち上げない
/// (Capturer は Supervisor から spawn されることもあれば、独立起動されることもある)。
fn build_capturer_tray_options(pid: u32, supervisor_exe_hint: Option<&Path>) -> anyhow::Result<TrayOptions> {
	let supervisor_exe = supervisor_exe_hint.map(PathBuf::from).or_else(resolve_supervisor_exe_from_self);
	let tooltip = format!("UN Motion Capturer (PID {pid})");
	let open_label = "Open Supervisor Console".to_string();
	let exit_label = format!("Exit Capturer (PID {pid})");
	Ok(TrayOptions {
		tooltip: Some(tooltip),
		open_exe: supervisor_exe,
		open_menu_label: Some(open_label),
		exit_menu_label: Some(exit_label),
		open_on_startup: Some(false),
	})
}

fn apply_cli_active_profile(document: &mut un_motion_core::CoreProfileDocument, active_profile: Option<&str>) {
	let Some(active_profile) = active_profile.map(str::trim).filter(|value| !value.is_empty()) else {
		return;
	};
	if document.profiles.iter().any(|profile| profile.id == active_profile) {
		document.selected_profile_id = active_profile.to_string();
	} else {
		tracing::warn!(
			active_profile,
			"requested --active-profile was not found in profile document; keeping stored selected profile",
		);
	}
}

fn run_experimental_flicker_test(document: &CoreProfileDocument) -> anyhow::Result<()> {
	let profile = document
		.profiles
		.iter()
		.find(|profile| profile.id == document.selected_profile_id)
		.with_context(|| format!("selected profile was not found: {}", document.selected_profile_id))?;
	let summary = measure_profile_directshow_flicker(profile)?;
	println!("{}", serde_json::to_string_pretty(&summary)?);
	Ok(())
}

#[cfg(windows)]
fn measure_profile_directshow_flicker(profile: &CoreProfileDocumentProfile) -> anyhow::Result<FlickerTestSummary> {
	let runtime = profile.runtime_selection.as_ref();
	let pipeline = profile.pipeline_components.as_ref();
	let engine = runtime.and_then(|runtime| runtime.engine.as_deref()).unwrap_or("mediapipe-native");
	if engine != "mediapipe-native" {
		bail!("--experimental-flicker-test requires mediapipe-native profile, got {engine}");
	}
	let input = pipeline
		.and_then(|pipeline| pipeline.input.as_deref())
		.unwrap_or("webcam-directshow");
	if input != "webcam-directshow" {
		bail!("--experimental-flicker-test currently measures DirectShow/ccap-rs only, got input {input}");
	}
	let (runtime_width, runtime_height) = runtime
		.and_then(|runtime| runtime.resolution.as_deref())
		.and_then(parse_resolution)
		.unwrap_or((640, 480));
	let width = pipeline.and_then(|pipeline| pipeline.input_width).unwrap_or(runtime_width);
	let height = pipeline.and_then(|pipeline| pipeline.input_height).unwrap_or(runtime_height);
	let fps = pipeline
		.and_then(|pipeline| pipeline.input_fps)
		.or_else(|| runtime.and_then(|runtime| runtime.fps))
		.unwrap_or(30)
		.clamp(1, 240);
	let device = runtime.and_then(|runtime| runtime.device.as_deref()).unwrap_or_default();
	let config = DirectShowCaptureConfig::new(width, height, fps)
		.with_pixel_format(pipeline.and_then(|pipeline| pipeline.input_pixel_format.clone()));
	let requested_format = config.requested_label();
	let mut backend = DirectShowWebcamBackend::with_capture_config(config);
	let selected = backend
		.list_devices()?
		.into_iter()
		.find(|candidate| candidate.id == device || candidate.name == device || device.is_empty())
		.with_context(|| format!("DirectShow camera not found for profile device '{device}'"))?;

	let mut frames = 0_u32;
	let mut actual_format = "-".to_string();
	let mut luma_means = Vec::new();
	let mut frame_diffs = Vec::new();
	let mut previous_luma: Option<Vec<f32>> = None;
	let first_frame = backend
		.capture_next_image(&selected.id)?
		.context("DirectShow camera did not produce a frame before timeout")?;
	let started = Instant::now();
	observe_flicker_frame(
		&mut frames,
		&mut actual_format,
		&mut luma_means,
		&mut frame_diffs,
		&mut previous_luma,
		&backend,
		first_frame,
	);
	while started.elapsed() < Duration::from_secs(1) {
		let frame = backend
			.capture_next_image(&selected.id)?
			.context("DirectShow camera did not produce a frame before timeout")?;
		observe_flicker_frame(
			&mut frames,
			&mut actual_format,
			&mut luma_means,
			&mut frame_diffs,
			&mut previous_luma,
			&backend,
			frame,
		);
	}
	if luma_means.is_empty() {
		bail!("DirectShow camera did not produce analyzable RGB frames");
	}

	let elapsed_ms = started.elapsed().as_secs_f32() * 1000.0;
	let avg_fps = if elapsed_ms > 0.0 {
		frames as f32 * 1000.0 / elapsed_ms
	} else {
		0.0
	};
	let luma_mean_avg = mean_f32(&luma_means);
	let luma_mean_min = luma_means.iter().copied().fold(f32::INFINITY, f32::min);
	let luma_mean_max = luma_means.iter().copied().fold(f32::NEG_INFINITY, f32::max);
	let deltas = luma_means.windows(2).map(|pair| pair[1] - pair[0]).collect::<Vec<_>>();
	let (dominant_hz, dominant_amplitude) = dominant_luma_frequency(&luma_means, avg_fps);
	Ok(FlickerTestSummary {
		profile_id: profile.id.clone(),
		profile_name: profile.name.clone(),
		device: selected.name,
		requested_format,
		actual_format,
		frames,
		elapsed_ms,
		avg_fps,
		luma_mean_avg,
		luma_mean_min,
		luma_mean_max,
		luma_mean_range: luma_mean_max - luma_mean_min,
		luma_mean_std_dev: std_dev_f32(&luma_means, luma_mean_avg),
		luma_delta_rms: rms_f32(&deltas),
		luma_delta_max: deltas.iter().map(|value| value.abs()).fold(0.0_f32, f32::max),
		frame_diff_avg: mean_f32(&frame_diffs),
		frame_diff_max: frame_diffs.iter().copied().fold(0.0_f32, f32::max),
		dominant_hz,
		dominant_amplitude,
	})
}

#[cfg(windows)]
fn observe_flicker_frame(
	frames: &mut u32,
	actual_format: &mut String,
	luma_means: &mut Vec<f32>,
	frame_diffs: &mut Vec<f32>,
	previous_luma: &mut Option<Vec<f32>>,
	backend: &DirectShowWebcamBackend,
	frame: un_motion_interfaces::ImageFrame,
) {
	*actual_format = backend
		.active_format_label()
		.unwrap_or_else(|| format!("{}x{} {:?}", frame.width, frame.height, frame.pixel_format));
	let luma = frame_luma_samples(&frame.data, frame.width, frame.height, frame.stride_bytes);
	if luma.is_empty() {
		return;
	}
	if let Some(previous) = previous_luma.as_ref() {
		frame_diffs.push(mean_abs_diff(previous, &luma));
	}
	let mean = mean_f32(&luma);
	*previous_luma = Some(luma);
	luma_means.push(mean);
	*frames = frames.saturating_add(1);
}

#[cfg(not(windows))]
fn measure_profile_directshow_flicker(_profile: &CoreProfileDocumentProfile) -> anyhow::Result<FlickerTestSummary> {
	bail!("--experimental-flicker-test currently supports DirectShow/ccap-rs on Windows only")
}

fn parse_resolution(value: &str) -> Option<(u32, u32)> {
	let (width, height) = value.split_once('x').or_else(|| value.split_once('X'))?;
	Some((width.parse().ok()?, height.parse().ok()?))
}

fn frame_luma_samples(bytes: &[u8], width: u32, height: u32, stride_bytes: u32) -> Vec<f32> {
	let width = width as usize;
	let height = height as usize;
	let stride = stride_bytes as usize;
	if width == 0 || height == 0 || stride < width.saturating_mul(3) {
		return Vec::new();
	}
	let step_x = (width / 160).max(1);
	let step_y = (height / 90).max(1);
	let mut values = Vec::with_capacity((width / step_x + 1).saturating_mul(height / step_y + 1));
	for y in (0..height).step_by(step_y) {
		let row = y.saturating_mul(stride);
		for x in (0..width).step_by(step_x) {
			let index = row.saturating_add(x.saturating_mul(3));
			if index + 2 >= bytes.len() {
				continue;
			}
			let r = bytes[index] as f32;
			let g = bytes[index + 1] as f32;
			let b = bytes[index + 2] as f32;
			values.push((0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0);
		}
	}
	values
}

fn mean_f32(values: &[f32]) -> f32 {
	if values.is_empty() {
		return 0.0;
	}
	values.iter().sum::<f32>() / values.len() as f32
}

fn std_dev_f32(values: &[f32], mean: f32) -> f32 {
	if values.len() < 2 {
		return 0.0;
	}
	(values
		.iter()
		.map(|value| {
			let delta = value - mean;
			delta * delta
		})
		.sum::<f32>()
		/ values.len() as f32)
		.sqrt()
}

fn rms_f32(values: &[f32]) -> f32 {
	if values.is_empty() {
		return 0.0;
	}
	(values.iter().map(|value| value * value).sum::<f32>() / values.len() as f32).sqrt()
}

fn mean_abs_diff(left: &[f32], right: &[f32]) -> f32 {
	let len = left.len().min(right.len());
	if len == 0 {
		return 0.0;
	}
	left.iter().zip(right.iter()).take(len).map(|(a, b)| (a - b).abs()).sum::<f32>() / len as f32
}

fn dominant_luma_frequency(values: &[f32], sample_fps: f32) -> (f32, f32) {
	if values.len() < 4 || sample_fps <= 0.0 {
		return (0.0, 0.0);
	}
	let mean = mean_f32(values);
	let n = values.len();
	let mut best_hz = 0.0;
	let mut best_amplitude = 0.0;
	for bin in 1..=n / 2 {
		let mut re = 0.0_f32;
		let mut im = 0.0_f32;
		for (index, value) in values.iter().enumerate() {
			let phase = std::f32::consts::TAU * bin as f32 * index as f32 / n as f32;
			let centered = *value - mean;
			re += centered * phase.cos();
			im -= centered * phase.sin();
		}
		let amplitude = (re * re + im * im).sqrt() * 2.0 / n as f32;
		if amplitude > best_amplitude {
			best_amplitude = amplitude;
			best_hz = bin as f32 * sample_fps / n as f32;
		}
	}
	(best_hz, best_amplitude)
}

/// Phase E settings policy (Capturer 側 entry point 用)。Supervisor の
/// `app_config_dir()` と同じ規約: `%APPDATA%\UN Motion\` (Linux:
/// `$XDG_CONFIG_HOME/un-motion/`)。`UN_MOTION_CONFIG_DIR` で上書き可。
fn user_config_dir() -> PathBuf {
	if let Some(path) = env::var_os("UN_MOTION_CONFIG_DIR") {
		return PathBuf::from(path);
	}
	if let Some(path) = env::var_os("APPDATA") {
		return PathBuf::from(path).join("UN Motion");
	}
	if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
		return PathBuf::from(path).join("un-motion");
	}
	if let Some(path) = env::var_os("HOME") {
		return PathBuf::from(path).join(".config").join("un-motion");
	}
	PathBuf::from(".").join("un-motion-config")
}

/// Phase E: bundled テンプレートの探索ルール。Supervisor の
/// `bundled_template_profiles_dir()` と同じ規約。
fn bundled_template_profiles_dir() -> Option<PathBuf> {
	if let Ok(exe) = env::current_exe()
		&& let Some(exe_dir) = exe.parent()
	{
		let candidate = exe_dir.join("profiles");
		if candidate.is_dir() {
			return Some(candidate);
		}
	}
	// dev: cargo run -p un-motion-capturer。crate は `crates/un-motion-capturer/`
	// にあるので 2 つ上が workspace root。
	let workspace_candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
		.parent()
		.and_then(Path::parent)
		.map(|root| root.join("profiles"));
	if let Some(p) = workspace_candidate
		&& p.is_dir()
	{
		return Some(p);
	}
	None
}

/// 実行ファイル (= `un-motion-capturer[.exe]`) と同じディレクトリにある
/// `un-motion-supervisor[.exe]` を探す。`cargo run --release` 時に Supervisor の
/// build.rs が `target/release` に両方の exe を配置する想定。
fn resolve_supervisor_exe_from_self() -> Option<PathBuf> {
	let current_exe = std::env::current_exe().ok()?;
	let dir = current_exe.parent()?;
	let candidates = if cfg!(target_os = "windows") {
		vec![dir.join("un-motion-supervisor.exe")]
	} else {
		vec![dir.join("un-motion-supervisor")]
	};
	candidates.into_iter().find(|p| p.is_file())
}

fn report_bind_addr_sync(bind_addr: SocketAddr) {
	let addr = if bind_addr.port() == 0 {
		SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
	} else {
		bind_addr
	};
	println!("listening on {addr}");
}

/// HTTP server が listen している (はずの) アドレスを stdout に "listening on <addr>" 形式で
/// 1 行出力する。port が `0` の場合は OS 割り当てを actix_web 4 では取り出す術がないため、
/// 利用者は明示ポートを指定するか、HTTP API の `/healthz` で待ち合わせる運用とする。
async fn report_bind_addr(bind_addr: SocketAddr) {
	let addr = if bind_addr.port() == 0 {
		SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
	} else {
		bind_addr
	};
	println!("listening on {addr}");
}
