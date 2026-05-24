//! UN Motion Supervisor (Tauri) — Phase D 実装。
//!
//! 役割:
//! * 1 つの Supervisor Console (Svelte GUI) で複数の `un-motion-capturer` プロセスを管理する。
//! * 各 Capturer は HTTP API server を持ち、Supervisor は loopback の HTTP/SSE 経由で
//!   制御・状態取得を行う。
//! * UN Avatar Supervisor (`apps/un-avatar-supervisor`) と UI 規約・Tauri command 名・状態
//!   モデルを揃え、`un-motion-supervisor → un-motion-capturer → Zenoh →
//!   un-avatar-supervisor → un-avatar-render-wgpu` の対称構造を保つ。
//!
//! 現状 (D4-a):
//! * Capturer の `launch` / `stop` / `runtime_status` / `list` を UN Avatar の renderer 制御
//!   パターンを翻訳して実装。
//! * Profile 一覧 / 切替 (D4-b) は次の commit で追加。
//! * Svelte UI 骨組み (D4-c) も次の commit で UN Avatar から移植。

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use std::env;

use serde::{Deserialize, Serialize};

mod i18n;

// rust-i18n の compile-time セットアップ。`locales/` 配下を権威ソースとして取り込み、
// fallback は `ja-JP`。`backend` は `i18n::UN_I18N_STORE` の clone (共有 crate
// `un-i18n` の `UnI18nStore`)。詳細は `i18n.rs` と <https://github.com/usagi/un-common>。
rust_i18n::i18n!("locales", fallback = "ja-JP", backend = (*crate::i18n::UN_I18N_STORE).clone());

use rust_i18n::t;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use un_motion_profile_schema::{
	CoreProfileDocument, CoreProfileDocumentProfile, CoreProfileDocumentStore, ProfileModifierSettings, ProfilePipelineComponents,
	ProfileRuntimeSettings, profile_engine_summary,
};

/// メインウィンドウのラベル。UN Avatar Supervisor の規約に合わせる。
const MAIN_WINDOW_LABEL: &str = "main";
const APP_TITLE: &str = "UN Motion Supervisor";
/// メインウィンドウのタイトルバー文字列。`CARGO_PKG_VERSION` 経由でバージョンを
/// 取り込み、`Cargo.toml` を更新するだけで自動的に反映される (ハードコード忘れの事故防止)。
/// 表示形式は `U.N. Motion - 1.0.0` (UN Avatar Supervisor と統一)。
fn app_title_with_version() -> String {
	format!("U.N. Motion - {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn command_without_console(command: &mut Command) -> &mut Command {
	#[cfg(windows)]
	{
		command.creation_flags(CREATE_NO_WINDOW);
	}
	command
}

/// stderr buffer の最大行数。UN Avatar Supervisor と揃える。
/// Capturer 1 プロセスあたりの stderr ring buffer 上限行数。
///
/// UN Avatar Supervisor の 120 行 (`MAX_RENDERER_LOG_LINES`) より多めにする:
/// Capturer は VMC 受信 engine の累積統計や Zenoh の bind 行など、起動時の
/// セットアップだけで 15-20 行ほど消費し、運用中も periodic INFO ライン
/// (300 frame ごとの cumulative 等) を吐く。Logs タブで「数分前の起動 → 現在」
/// の流れを一通り目視追跡できる程度の余裕として 800 行を確保する。
/// 1 行平均 200 byte でも total 160KB/Capturer 程度なのでメモリ影響は軽微。
const MAX_CAPTURER_LOG_LINES: usize = 800;
const MAX_STOPPED_CAPTURER_HISTORY: usize = 20;

/// Capturer 起動後に `/healthz` が ok を返すまで待つ最大時間。
const HEALTHZ_WAIT_TIMEOUT: Duration = Duration::from_secs(8);
const HEALTHZ_POLL_INTERVAL: Duration = Duration::from_millis(120);

/// HTTP GET / POST のタイムアウト。loopback なので短めで十分。
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
/// Runtime 内で複数フレームをサンプリングする操作は、通常の health / snapshot
/// request より長く待つ必要がある。
const HTTP_RUNTIME_ACTION_TIMEOUT: Duration = Duration::from_secs(15);

/// Supervisor が保持する Capturer インスタンスのマップと運用上の連番。
/// UN Avatar の `SupervisorState` と同じ責務 (Mutex 越しに一元管理する) を持つ。
struct SupervisorState {
	next_id: u32,
	capturers: BTreeMap<u32, ManagedCapturer>,
	http_client: reqwest::blocking::Client,
}

impl SupervisorState {
	fn new() -> Self {
		let http_client = reqwest::blocking::Client::builder()
			.timeout(HTTP_REQUEST_TIMEOUT)
			.build()
			.expect("failed to build reqwest blocking client");
		Self {
			next_id: 0,
			capturers: BTreeMap::new(),
			http_client,
		}
	}
}

impl Default for SupervisorState {
	fn default() -> Self {
		Self::new()
	}
}

/// アプリ全体の永続的な設定 (Settings タブで編集する項目)。
///
/// Phase E settings policy (ユーザー決定): "Seed 廃止 + bundled templates +
/// 初回コピー" 方式で UN Avatar と統一する。Supervisor / Capturer は
/// `%APPDATA%\UN Motion\` (Linux: `$XDG_CONFIG_HOME/un-motion/`) を **ユーザー
/// ディレクトリ**として共有し、以下の内訳で永続化する:
///
/// * `<user_dir>/settings.toml` — `AppRuntimeSettings` (このファイル)
/// * `<user_dir>/conf.toml`     — `[desktop]` セクション (profile order / launch default)
/// * `<user_dir>/profiles/*.toml` — 各 profile (初回起動時にリポジトリ同梱
///   `<workspace>/profiles/*.toml` から自動コピー)
///
/// 旧 workspace `conf.toml` の `[supervisor]` セクションは Phase E で廃止された。
/// 自動 migration は行わない (ユーザー決定): 既存ユーザーは新しい
/// `settings.toml` を新規に持つだけになる。
///
/// UN Avatar Supervisor の `AppRuntimeSettings` と機能セット / フィールド名 / 既定値を
/// 揃え、両アプリの Settings 画面で同じ操作が同じ意味で並ぶようにする。未知フィールドは
/// 無視する形にしてあるので、テンプレ更新でフィールドが追加されても旧 TOML はそのまま
/// 読み込める。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRuntimeSettings {
	/// Supervisor Console を閉じたときに動作中の Capturer をすべて停止するか。
	/// 既定 ON が「ユーザーにとって直感的」というユーザー要望に基づく。Capturer を
	/// 残しておきたい場合 (Capturer system tray からの起動運用) は OFF にする。
	#[serde(default = "default_true")]
	pub stop_capturers_on_exit: bool,
	/// system tray アイコンを表示するか。OFF だと以下 3 つの toggle は無効化される。
	#[serde(default)]
	pub system_tray_enabled: bool,
	/// ウィンドウを最小化したとき tray に隠すか。
	#[serde(default = "default_true")]
	pub minimize_to_tray: bool,
	/// Capturer が動作中、X ボタンで閉じる代わりに tray に隠すか。
	#[serde(default = "default_true")]
	pub close_to_tray_while_running: bool,
	/// 次回起動時に tray に隠した状態で起動するか。
	#[serde(default)]
	pub start_minimized_to_tray: bool,
	/// UI のテーマモード (`light` / `dark` / `system`)。
	#[serde(default = "default_theme_mode")]
	pub theme_mode: String,
	/// 終了時の Supervisor Console ウィンドウの outer 位置 (px)。None なら OS 既定位置で起動。
	#[serde(default)]
	pub console_window_x: Option<i32>,
	#[serde(default)]
	pub console_window_y: Option<i32>,
	/// 終了時の Supervisor Console ウィンドウの inner サイズ (px)。
	#[serde(default)]
	pub console_window_width: Option<u32>,
	#[serde(default)]
	pub console_window_height: Option<u32>,
	/// Profiles → Quick Launch を押したときに Capturers タブへ自動遷移するか。
	/// UN Avatar Supervisor 側でも同じ Quick Launch 用語へ揃える前提の設定。
	#[serde(default)]
	pub jump_to_capturers_on_quick_launch: bool,
	/// 起動時に Capturers の launch target として選択中の profile/group を自動起動する。
	#[serde(default)]
	pub auto_launch_selected_on_startup: bool,
	/// Capturer process ごとの Actix Web API worker threads。
	#[serde(default = "default_api_worker_threads")]
	pub api_worker_threads: usize,
	/// UI 表示言語 (BCP-47 完全形, 例: `ja-JP` / `en-US`)。空文字なら `i18n::resolve_default_locale`
	/// (OS locale → サポート言語 → `ja-JP` 最終フォールバック) で起動時に解決する。
	#[serde(default)]
	pub locale: String,
	/// file-video input が使う外部 FFmpeg 実行ファイル。配布物には FFmpeg を同梱せず、
	/// ユーザー環境の `ffmpeg(.exe)` か明示指定されたパスを使う。
	#[serde(default)]
	pub external_tools_ffmpeg_path: Option<String>,
	/// Capturers タブの calibration ボタンを押してから sampling を開始するまでの猶予秒。
	#[serde(default = "default_calibration_start_delay_seconds")]
	pub calibration_start_delay_seconds: u32,
	/// Neutral / pose calibration で集める有効サンプル数。
	#[serde(default = "default_calibration_sample_count")]
	pub calibration_sample_count: usize,
	/// Calibration countdown / start sound volume, 0.0-1.0.
	#[serde(default = "default_calibration_sound_volume")]
	pub calibration_sound_volume: f32,
	/// 未設定なら bundled `/sounds/un-calibration-sound-pun.flac` を使う。
	#[serde(default)]
	pub calibration_countdown_sound_path: Option<String>,
	/// 未設定なら bundled `/sounds/un-calibration-sound-pon.flac` を使う。
	#[serde(default)]
	pub calibration_start_sound_path: Option<String>,
	/// UNMF/JSONL Snapshot save dialog の前回保存フォルダー。
	/// 未設定または存在しない場合は OS の Documents フォルダーを既定にする。
	#[serde(default)]
	pub snapshot_save_dir: Option<String>,
	/// Snapshot 保存時に開発分析用の仕様外サイドカー JSONL も保存する。
	#[serde(default)]
	pub snapshot_save_analysis_extras: bool,
}

fn default_true() -> bool {
	true
}

fn default_theme_mode() -> String {
	"system".to_string()
}

fn default_calibration_start_delay_seconds() -> u32 {
	3
}

fn default_calibration_sample_count() -> usize {
	45
}

fn default_calibration_sound_volume() -> f32 {
	0.75
}

fn logical_core_count() -> usize {
	std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get).max(1)
}

fn normalize_api_worker_threads(value: usize) -> usize {
	value.clamp(1, logical_core_count())
}

fn default_api_worker_threads() -> usize {
	normalize_api_worker_threads(2)
}

impl Default for AppRuntimeSettings {
	fn default() -> Self {
		Self {
			stop_capturers_on_exit: true,
			system_tray_enabled: false,
			minimize_to_tray: true,
			close_to_tray_while_running: true,
			start_minimized_to_tray: false,
			theme_mode: default_theme_mode(),
			console_window_x: None,
			console_window_y: None,
			console_window_width: None,
			console_window_height: None,
			jump_to_capturers_on_quick_launch: false,
			auto_launch_selected_on_startup: false,
			api_worker_threads: default_api_worker_threads(),
			locale: String::new(),
			external_tools_ffmpeg_path: None,
			calibration_start_delay_seconds: default_calibration_start_delay_seconds(),
			calibration_sample_count: default_calibration_sample_count(),
			calibration_sound_volume: default_calibration_sound_volume(),
			calibration_countdown_sound_path: None,
			calibration_start_sound_path: None,
			snapshot_save_dir: None,
			snapshot_save_analysis_extras: false,
		}
	}
}

/// アプリ設定の保存場所を解決する。
///
/// ユーザー方針: 「`%APPDATA%` 配下の UN Avatar 方式に揃える」 (Phase E debug 期の
/// ユーザー指示)。`UN_MOTION_CONFIG_DIR` 環境変数で上書き可 (テストや
/// portable 運用に対応)。
///
/// 既定:
/// * Windows: `%APPDATA%\UN Motion\`
/// * Linux  : `$XDG_CONFIG_HOME/un-motion/` or `$HOME/.config/un-motion/`
/// * fallback (環境変数いずれも未定義): workspace の
///   `target/tmp/un-motion-config/` (CI / dev で用いる、レポにコミットされない)。
fn app_config_dir() -> PathBuf {
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
	repo_root().join("target").join("tmp").join("un-motion-config")
}

fn app_settings_path() -> PathBuf {
	app_config_dir().join("settings.toml")
}

/// Phase E: bundled テンプレートが置かれるディレクトリ候補を探す。
///
/// 1. `<exe_dir>/profiles/` (release install / portable zip 想定)
/// 2. `<workspace_root>/profiles/` (dev: `cargo run`)
///
/// どちらも見つからない場合は `None` を返す。`None` のときも `default_profiles()`
/// (= ハードコードの "Default" 1 件) フォールバックで Capturer は起動できる。
fn bundled_template_profiles_dir() -> Option<PathBuf> {
	if let Ok(exe) = env::current_exe()
		&& let Some(exe_dir) = exe.parent()
	{
		let candidate = exe_dir.join("profiles");
		if candidate.is_dir() {
			return Some(candidate);
		}
	}
	let workspace = repo_root().join("profiles");
	if workspace.is_dir() {
		return Some(workspace);
	}
	None
}

/// Phase E: ユーザーディレクトリの `CoreProfileDocumentStore` を返す。
/// 初回起動 (= user dir 配下の `profiles/` が空) のときは bundled テンプレートを
/// コピーする。それ以外は何もしない (ユーザーが削除した profile を勝手に復活
/// させない / ユーザー編集を上書きしない)。
fn user_profile_store() -> CoreProfileDocumentStore {
	let store = CoreProfileDocumentStore::from_user_dir(app_config_dir());
	if let Some(template_dir) = bundled_template_profiles_dir()
		&& let Err(error) = store.seed_from_templates(&template_dir)
	{
		tracing::warn!(
			template_dir = %template_dir.display(),
			%error,
			"failed to seed user profile dir from bundled templates (Capturer will still start with the hardcoded default profile)",
		);
	}
	store
}

fn profile_path_for_id(id: &str) -> PathBuf {
	let profiles_dir = user_profile_store().profiles_dir().to_path_buf();
	if let Ok(entries) = fs::read_dir(&profiles_dir) {
		for entry in entries.flatten() {
			let path = entry.path();
			if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
				continue;
			}
			let Ok(raw) = fs::read_to_string(&path) else {
				continue;
			};
			let Ok(value) = raw.parse::<toml::Value>() else {
				continue;
			};
			if value.get("id").and_then(toml::Value::as_str) == Some(id) {
				return path;
			}
		}
	}
	profiles_dir.join(format!("unknown-{id}.toml"))
}

/// `%APPDATA%\UN Motion\settings.toml` から設定を読み出す。
/// ファイルが無い / parse 失敗の場合は既定値を返す。
fn load_app_settings() -> AppRuntimeSettings {
	let path = app_settings_path();
	let Ok(raw) = fs::read_to_string(&path) else {
		return AppRuntimeSettings::default();
	};
	toml::from_str(&raw).unwrap_or_default()
}

/// `%APPDATA%\UN Motion\settings.toml` に設定を書き出す。
/// 親ディレクトリが無い場合は作成する。
fn write_app_settings(settings: &AppRuntimeSettings) -> Result<(), String> {
	let path = app_settings_path();
	if let Some(dir) = path.parent()
		&& !dir.as_os_str().is_empty()
	{
		fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
	}
	let text = toml::to_string_pretty(settings).map_err(|e| format!("serialize settings.toml: {e}"))?;
	fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

/// 起動中の Capturer プロセス本体。`un-motion-capturer` を spawn して `Child` を保持し、
/// HTTP API アドレスを通じて状態を取りに行く。
struct ManagedCapturer {
	info: CapturerInstance,
	child: Child,
	bind_addr: SocketAddr,
	started_at: Instant,
	stderr_tail: Arc<Mutex<Vec<String>>>,
	/// Phase E telemetry: 前回 `capturer_runtime_status` で観測した累積カウンタ。
	/// 次回観測時に Δcount / Δt で FPS を算出する。値が無い間は FPS = 0 を返す
	/// (まだ「2 サンプル目が無い」状態)。Supervisor の 1.5s poll cadence で更新される。
	prev_telemetry: Option<TelemetrySample>,
}

/// Capturer の `output_telemetry` を時刻と一緒に切り取った 1 サンプル。
/// Supervisor 側で前回値と比較して FPS を算出するためだけに保持する。
#[derive(Clone, Debug)]
struct TelemetrySample {
	sampled_at: Instant,
	vmc_datagrams: u64,
	vmc_packets: u64,
	zenoh_frames: u64,
	/// `stream_id → (kind, source_id, raw_received, frames_emitted)`。
	sources: std::collections::BTreeMap<String, SourceSampleEntry>,
}

#[derive(Clone, Debug)]
struct SourceSampleEntry {
	kind: String,
	source_id: String,
	raw_received: u64,
	frames_emitted: u64,
	observed_source_fps_milli: u64,
}

/// GUI に返却する Capturer 情報。UN Avatar `RendererInstance` と同じ目的の DTO。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturerInstance {
	pub id: u32,
	pub name: String,
	pub state: CapturerState,
	pub pid: Option<u32>,
	pub bind_addr: Option<String>,
	pub profile_id: Option<String>,
	pub uptime_secs: u64,
	pub last_stderr: Option<String>,
	pub stderr_tail: Vec<String>,
	pub exit_code: Option<i32>,
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapturerState {
	Starting,
	Running,
	Stopping,
	Exited,
	Crashed,
}

/// Capturer の runtime telemetry を Svelte に返す DTO。UN Avatar
/// `RendererRuntimeStatus` の UN Motion 版で、`/api/runtime/snapshot` の生 JSON も
/// まとめて返す (snapshot は CoreSnapshot 型を変更しても Svelte 側で柔軟に表示する
/// ため `serde_json::Value` で受け取り再シリアライズする)。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturerRuntimeStatus {
	pub id: u32,
	pub state: CapturerState,
	pub pid: Option<u32>,
	pub bind_addr: Option<String>,
	pub healthy: bool,
	pub uptime_secs: u64,
	pub note: Option<String>,
	pub snapshot: Option<serde_json::Value>,
	/// Phase E telemetry: 直前サンプルとの差分から算出した FPS。
	/// 「初回観測」では `None` (1 サンプル目を保存しただけで Δt 無し)、
	/// 2 回目以降は `Some(...)`。
	#[serde(skip_serializing_if = "Option::is_none")]
	pub fps: Option<CapturerOutputFps>,
}

/// 各出力ステージの送信レート (frame/sec, datagram/sec) を 1 つの DTO にまとめたもの。
/// Capturer 側は cumulative counter しか持たないので、Supervisor が Δcount / Δt を計算する。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturerOutputFps {
	/// 直前サンプルからの経過秒。表示用 (例えば `1.5 s`)。
	pub interval_secs: f32,
	/// VMC/UDP 出力: UDP datagram / sec。
	pub vmc_datagrams_per_sec: f32,
	/// VMC/UDP 出力: OSC packet / sec (`/VMC/Ext/Bone/Pos` 等の論理 message 単位)。
	pub vmc_packets_per_sec: f32,
	/// UNMF/Z 出力: 完成 frame / sec。
	pub zenoh_frames_per_sec: f32,
	/// 各 source stage の受信 / emit レート。
	pub sources: Vec<CapturerSourceFps>,
}

/// 1 つの source/engine ステージのレート。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturerSourceFps {
	pub kind: String,
	pub stream_id: String,
	pub source_id: String,
	/// raw 単位 (= UDP datagram / camera frame / TCP message) の受信レート。
	pub raw_per_sec: f32,
	/// UNMotionFrame として emit したレート。
	pub frames_per_sec: f32,
	/// 入力 backend が直接報告した実 source fps。Webcam では camera capture 側のfps。
	pub observed_source_fps: Option<f32>,
}

/// GUI 側で `await invoke('list_capturers')` する想定の Tauri command。
#[tauri::command]
fn list_capturers(state: State<'_, Mutex<SupervisorState>>) -> Result<Vec<CapturerInstance>, String> {
	let mut state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	refresh_capturers(&mut state);
	Ok(state.capturers.values().map(|capturer| capturer.info.clone()).collect())
}

/// Capturer プロセスを spawn して HTTP API の起動を待ち、インスタンス情報を返す。
///
/// Capturer は `--active-profile <id>` を受け取って起動と同時に指定 profile の
/// runtime を立ち上げる。
/// これにより「Launch Capturer = MediaPipe Native + Zenoh / VMC 送出開始」が成立する。
///
/// **重複起動**: UN Avatar の `launch_renderer` (`allow_multiple_renderers == false`) と同様、
/// 同一プロファイルで **すでに** Starting / Running / Stopping の Capturer がある場合は
/// 新規 spawn せず既存インスタンスを返す（UN Motion ではプロファイル単位の複数 Capturer は想定しない）。
#[tauri::command]
fn launch_capturer(
	profile_id: Option<String>,
	allow_non_loopback: Option<bool>,
	state: State<'_, Mutex<SupervisorState>>,
	app_settings: State<'_, Mutex<AppRuntimeSettings>>,
) -> Result<CapturerInstance, String> {
	let api_worker_threads = app_settings
		.lock()
		.map(|settings| normalize_api_worker_threads(settings.api_worker_threads))
		.unwrap_or_else(|_| default_api_worker_threads());
	let mut state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	refresh_capturers(&mut state);

	let resolved_profile_id: Option<String> = profile_id
		.as_ref()
		.map(|s| s.trim().to_string())
		.and_then(|s| (!s.is_empty()).then_some(s));

	let is_live = |c: &ManagedCapturer| {
		matches!(
			c.info.state,
			CapturerState::Starting | CapturerState::Running | CapturerState::Stopping
		)
	};

	if let Some(ref key) = resolved_profile_id {
		if let Some(existing) = state
			.capturers
			.values()
			.find(|c| is_live(c) && c.info.profile_id.as_deref() == Some(key.as_str()))
		{
			return Ok(existing.info.clone());
		}
	} else if let Some(existing) = state.capturers.values().find(|c| is_live(c) && c.info.profile_id.is_none()) {
		return Ok(existing.info.clone());
	}

	let bind_addr = reserve_loopback_address()?;
	let mut command = capturer_command(
		bind_addr,
		allow_non_loopback.unwrap_or(false),
		resolved_profile_id.as_deref(),
		api_worker_threads,
	)?;
	configure_hidden_child(&mut command);
	let mut child = command
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(|e| format!("launch capturer: {e}"))?;
	let stderr_tail = spawn_stderr_tail(child.stderr.take());
	let pid = child.id();
	state.next_id = state.next_id.saturating_add(1);
	let id = state.next_id;
	let started_at = Instant::now();
	// healthz を待つ前にプロセスが死ぬケースを早期検出する。
	if let Err(e) = wait_for_healthz(&state.http_client, bind_addr, &mut child, &stderr_tail) {
		let _ = force_stop_capturer_child(&mut child, "launch_capturer: healthz failed, abort spawn");
		let tail = stderr_tail.lock().map(|tail| tail.clone()).unwrap_or_default();
		let exit_code = child.try_wait().ok().flatten().and_then(|status| status.code());
		state.capturers.insert(
			id,
			ManagedCapturer {
				info: CapturerInstance {
					id,
					name: format!("Capturer #{id}"),
					state: CapturerState::Crashed,
					pid: None,
					bind_addr: Some(bind_addr.to_string()),
					profile_id: resolved_profile_id,
					uptime_secs: started_at.elapsed().as_secs(),
					last_stderr: tail.last().cloned(),
					stderr_tail: tail,
					exit_code,
				},
				child,
				bind_addr,
				started_at,
				stderr_tail,
				prev_telemetry: None,
			},
		);
		return Err(e);
	}
	let info = CapturerInstance {
		id,
		name: format!("Capturer #{id}"),
		state: CapturerState::Running,
		pid: Some(pid),
		bind_addr: Some(bind_addr.to_string()),
		profile_id: resolved_profile_id,
		uptime_secs: 0,
		last_stderr: None,
		stderr_tail: Vec::new(),
		exit_code: None,
	};
	let info_for_return = info.clone();
	state.capturers.insert(
		id,
		ManagedCapturer {
			info,
			child,
			bind_addr,
			started_at,
			stderr_tail,
			prev_telemetry: None,
		},
	);
	Ok(info_for_return)
}

/// Capturer プロセスを終了させる。まず `POST /api/core/exit` で graceful 終了を試み、
/// 一定時間内に終わらなければ kill する。
#[tauri::command]
fn stop_capturer(id: u32, state: State<'_, Mutex<SupervisorState>>) -> Result<(), String> {
	let mut state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	let http_client = state.http_client.clone();
	if let Some(capturer) = state.capturers.get_mut(&id) {
		stop_managed_capturer(id, capturer, &http_client)?;
	}
	Ok(())
}

/// Capturer の `/api/runtime/snapshot` を取得して GUI 用 DTO に詰め直す。
#[tauri::command]
fn capturer_runtime_status(id: u32, state: State<'_, Mutex<SupervisorState>>) -> Result<CapturerRuntimeStatus, String> {
	let mut state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	refresh_capturers(&mut state);
	let http_client = state.http_client.clone();
	let capturer = state.capturers.get_mut(&id).ok_or_else(|| format!("capturer not found: {id}"))?;
	let bind_addr = capturer.bind_addr;
	let info_state = capturer.info.state.clone();
	let pid = capturer.info.pid;
	let uptime_secs = capturer.info.uptime_secs;

	let (healthy, note, snapshot) = if matches!(info_state, CapturerState::Running | CapturerState::Starting) {
		match fetch_runtime_snapshot(&http_client, bind_addr) {
			Ok(snapshot) => (true, None, Some(snapshot)),
			Err(error) => (false, Some(error), None),
		}
	} else {
		(false, None, None)
	};

	// Phase E telemetry: snapshot から累積カウンタを抜き出し、前回サンプルと
	// 差分を取って FPS を算出する。初回観測 (`prev_telemetry` が None) では
	// `fps = None` を返し、サンプルだけ保存する。
	let fps = if let Some(snapshot_value) = snapshot.as_ref()
		&& let Some(cur_sample) = extract_telemetry_sample(snapshot_value)
	{
		let computed = capturer
			.prev_telemetry
			.as_ref()
			.and_then(|prev| compute_fps_from_samples(prev, &cur_sample));
		capturer.prev_telemetry = Some(cur_sample);
		computed
	} else {
		None
	};

	Ok(CapturerRuntimeStatus {
		id,
		state: info_state,
		pid,
		bind_addr: Some(bind_addr.to_string()),
		healthy,
		uptime_secs,
		note,
		snapshot,
		fps,
	})
}

#[tauri::command]
fn calibrate_capturer_neutral(
	id: u32,
	pose: Option<String>,
	valid_sample_count: Option<usize>,
	state: State<'_, Mutex<SupervisorState>>,
) -> Result<ProfileDetail, String> {
	let state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	let capturer = state.capturers.get(&id).ok_or_else(|| format!("capturer not found: {id}"))?;
	if !matches!(capturer.info.state, CapturerState::Running | CapturerState::Starting) {
		return Err(format!("capturer is not running: {id}"));
	}
	let profile_id = capturer
		.info
		.profile_id
		.clone()
		.or_else(|| fetch_capturer_active_profile_id(&state.http_client, capturer.bind_addr))
		.ok_or_else(|| "active profile is unknown".to_string())?;
	let url = format!("http://{}/api/runtime/calibration/neutral", capturer.bind_addr);
	let body = serde_json::json!({
		"pose": pose.as_deref().unwrap_or("U"),
		"validSampleCount": valid_sample_count.unwrap_or(45).clamp(1, 240),
	});
	let response = state
		.http_client
		.post(&url)
		.timeout(HTTP_RUNTIME_ACTION_TIMEOUT)
		.json(&body)
		.send()
		.map_err(|error| format!("POST {url} failed: {error}"))?;
	if !response.status().is_success() {
		return Err(response_error("POST", &url, response));
	}
	let envelope: ProfileDocumentEnvelope = response.json().map_err(|error| format!("decode calibration response: {error}"))?;
	let saved = save_profile_from_capturer_selection(&profile_id, envelope.selection)
		.map_err(|error| format!("save profile after calibration: {error}"))?;
	push_document_to_matching_capturers(&state, &saved, &profile_id);
	saved
		.profiles
		.iter()
		.find(|profile| profile.id == profile_id)
		.map(ProfileDetail::from)
		.ok_or_else(|| format!("profile vanished after calibration: {profile_id}"))
}

#[tauri::command]
fn clear_capturer_neutral_calibration(id: u32, state: State<'_, Mutex<SupervisorState>>) -> Result<ProfileDetail, String> {
	let state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	let capturer = state.capturers.get(&id).ok_or_else(|| format!("capturer not found: {id}"))?;
	if !matches!(capturer.info.state, CapturerState::Running | CapturerState::Starting) {
		return Err(format!("capturer is not running: {id}"));
	}
	let profile_id = capturer
		.info
		.profile_id
		.clone()
		.or_else(|| fetch_capturer_active_profile_id(&state.http_client, capturer.bind_addr))
		.ok_or_else(|| "active profile is unknown".to_string())?;
	let url = format!("http://{}/api/runtime/calibration/neutral/clear", capturer.bind_addr);
	let response = state
		.http_client
		.post(&url)
		.send()
		.map_err(|error| format!("POST {url} failed: {error}"))?;
	if !response.status().is_success() {
		return Err(response_error("POST", &url, response));
	}
	let envelope: ProfileDocumentEnvelope = response
		.json()
		.map_err(|error| format!("decode calibration clear response: {error}"))?;
	let saved = save_profile_from_capturer_selection(&profile_id, envelope.selection)
		.map_err(|error| format!("save profile after calibration clear: {error}"))?;
	push_document_to_matching_capturers(&state, &saved, &profile_id);
	saved
		.profiles
		.iter()
		.find(|profile| profile.id == profile_id)
		.map(ProfileDetail::from)
		.ok_or_else(|| format!("profile vanished after calibration clear: {profile_id}"))
}

#[tauri::command]
fn build_capturer_face_pose_model(
	id: u32,
	valid_sample_count: Option<usize>,
	state: State<'_, Mutex<SupervisorState>>,
) -> Result<ProfileDetail, String> {
	let state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	let capturer = state.capturers.get(&id).ok_or_else(|| format!("capturer not found: {id}"))?;
	if !matches!(capturer.info.state, CapturerState::Running | CapturerState::Starting) {
		return Err(format!("capturer is not running: {id}"));
	}
	let profile_id = capturer
		.info
		.profile_id
		.clone()
		.or_else(|| fetch_capturer_active_profile_id(&state.http_client, capturer.bind_addr))
		.ok_or_else(|| "active profile is unknown".to_string())?;
	let url = format!("http://{}/api/runtime/face-pose-model/build", capturer.bind_addr);
	let body = serde_json::json!({
		"validSampleCount": valid_sample_count.unwrap_or(90).clamp(1, 240),
	});
	let response = state
		.http_client
		.post(&url)
		.timeout(HTTP_RUNTIME_ACTION_TIMEOUT)
		.json(&body)
		.send()
		.map_err(|error| format!("POST {url} failed: {error}"))?;
	if !response.status().is_success() {
		return Err(response_error("POST", &url, response));
	}
	let envelope: ProfileDocumentEnvelope = response
		.json()
		.map_err(|error| format!("decode face pose model response: {error}"))?;
	let saved = save_profile_from_capturer_selection(&profile_id, envelope.selection)
		.map_err(|error| format!("save profile after face pose model build: {error}"))?;
	push_document_to_matching_capturers(&state, &saved, &profile_id);
	saved
		.profiles
		.iter()
		.find(|profile| profile.id == profile_id)
		.map(ProfileDetail::from)
		.ok_or_else(|| format!("profile vanished after face pose model build: {profile_id}"))
}

fn save_profile_from_capturer_selection(profile_id: &str, capturer_document: CoreProfileDocument) -> Result<CoreProfileDocument, String> {
	let store = user_profile_store();
	let user_document = merge_profile_from_capturer_selection(profile_id, store.load(), capturer_document)?;
	store.save(user_document).map_err(|error| error.to_string())
}

fn merge_profile_from_capturer_selection(
	profile_id: &str,
	mut user_document: CoreProfileDocument,
	capturer_document: CoreProfileDocument,
) -> Result<CoreProfileDocument, String> {
	let updated_profile = capturer_document
		.profiles
		.into_iter()
		.find(|profile| profile.id == profile_id)
		.ok_or_else(|| format!("capturer response does not contain profile: {profile_id}"))?;
	let profile = user_document
		.profiles
		.iter_mut()
		.find(|profile| profile.id == profile_id)
		.ok_or_else(|| format!("profile not found in user store: {profile_id}"))?;
	*profile = updated_profile;
	Ok(user_document)
}

#[tauri::command]
fn save_capturer_unmf_pose(
	id: u32,
	duration_ms: Option<u64>,
	snapshot_kind: Option<String>,
	state: State<'_, Mutex<SupervisorState>>,
	app_settings: State<'_, Mutex<AppRuntimeSettings>>,
) -> Result<Option<String>, String> {
	let (http_client, bind_addr) = {
		let state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
		let capturer = state.capturers.get(&id).ok_or_else(|| format!("capturer not found: {id}"))?;
		if !matches!(capturer.info.state, CapturerState::Running | CapturerState::Starting) {
			return Err(format!("capturer is not running: {id}"));
		}
		(state.http_client.clone(), capturer.bind_addr)
	};
	let duration = SnapshotDuration::from_request(duration_ms, snapshot_kind.as_deref());
	let frames = capture_unmf_frames_from_capturer(&http_client, bind_addr, duration)?;
	let pending_analysis = if snapshot_analysis_extras_enabled(&app_settings) {
		Some(capture_runtime_analysis_extras_to_temp(&http_client, &[bind_addr], duration)?)
	} else {
		None
	};
	let ts = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);
	let saved = save_unmf_jsonl(
		frames,
		format!("un-motion-{}-{ts}.unmf.jsonl", duration.file_label()),
		&app_settings,
	)?;
	if let (Some(path), Some(pending)) = (saved.as_deref(), pending_analysis.as_ref()) {
		write_runtime_analysis_extras(path, pending, duration)?;
	}
	Ok(saved)
}

#[tauri::command]
fn save_all_capturers_unmf_pose(
	duration_ms: Option<u64>,
	snapshot_kind: Option<String>,
	state: State<'_, Mutex<SupervisorState>>,
	app_settings: State<'_, Mutex<AppRuntimeSettings>>,
) -> Result<Option<String>, String> {
	let (http_client, targets) = {
		let state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
		let targets = state
			.capturers
			.values()
			.filter(|capturer| matches!(capturer.info.state, CapturerState::Running))
			.map(|capturer| capturer.bind_addr)
			.collect::<Vec<_>>();
		(state.http_client.clone(), targets)
	};
	if targets.is_empty() {
		return Err("no running capturers".to_string());
	}
	let duration = SnapshotDuration::from_request(duration_ms, snapshot_kind.as_deref());
	let mut handles = Vec::with_capacity(targets.len());
	for bind_addr in targets.iter().copied() {
		let client = http_client.clone();
		handles.push(std::thread::spawn(move || {
			capture_unmf_frames_from_capturer(&client, bind_addr, duration)
		}));
	}
	let mut frames = Vec::new();
	for handle in handles {
		let mut captured = handle.join().map_err(|_| "snapshot worker thread panicked".to_string())??;
		frames.append(&mut captured);
	}
	let pending_analysis = if snapshot_analysis_extras_enabled(&app_settings) {
		Some(capture_runtime_analysis_extras_to_temp(&http_client, &targets, duration)?)
	} else {
		None
	};
	let ts = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);
	let saved = save_unmf_jsonl(
		frames,
		format!("un-motion-{}-all-{ts}.unmf.jsonl", duration.file_label()),
		&app_settings,
	)?;
	if let (Some(path), Some(pending)) = (saved.as_deref(), pending_analysis.as_ref()) {
		write_runtime_analysis_extras(path, pending, duration)?;
	}
	Ok(saved)
}

fn capture_unmf_pose_from_capturer(http_client: &reqwest::blocking::Client, bind_addr: SocketAddr) -> Result<serde_json::Value, String> {
	let url = format!("http://{bind_addr}/api/runtime/unmf/pose");
	http_client
		.get(&url)
		.send()
		.map_err(|error| format!("GET {url} failed: {error}"))?
		.error_for_status()
		.map_err(|error| format!("GET {url} failed: {error}"))?
		.json()
		.map_err(|error| format!("decode UNMotionFrame response: {error}"))
}

#[derive(Clone, Copy)]
enum SnapshotDuration {
	OneFrame,
	OneSecond,
	ThreeSeconds,
}

impl SnapshotDuration {
	fn from_request(duration_ms: Option<u64>, snapshot_kind: Option<&str>) -> Self {
		match snapshot_kind.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
			Some("3s") | Some("three-seconds") | Some("three_seconds") => Self::ThreeSeconds,
			// 旧 1m Snapshot UI からの遅延呼び出しが残っていても、現行の 3s 扱いへ倒す。
			Some("1m") | Some("one-minute") | Some("one_minute") => Self::ThreeSeconds,
			Some("1s") | Some("one-second") | Some("one_second") => Self::OneSecond,
			Some("1f") | Some("one-frame") | Some("one_frame") => Self::OneFrame,
			_ => Self::from_ms(duration_ms.unwrap_or(0)),
		}
	}

	fn from_ms(duration_ms: u64) -> Self {
		if duration_ms >= 3000 {
			Self::ThreeSeconds
		} else if duration_ms >= 1000 {
			Self::OneSecond
		} else {
			Self::OneFrame
		}
	}

	fn file_label(self) -> &'static str {
		match self {
			Self::OneFrame => "1f-snapshot",
			Self::OneSecond => "1s-snapshot",
			Self::ThreeSeconds => "3s-snapshot",
		}
	}

	fn capture_duration_ms(self) -> u64 {
		match self {
			Self::OneFrame => 100,
			Self::OneSecond => 1000,
			Self::ThreeSeconds => 3000,
		}
	}
}

fn capture_unmf_frames_from_capturer(
	http_client: &reqwest::blocking::Client,
	bind_addr: SocketAddr,
	duration: SnapshotDuration,
) -> Result<Vec<serde_json::Value>, String> {
	match duration {
		SnapshotDuration::OneFrame => capture_unmf_pose_from_capturer(http_client, bind_addr).map(|frame| vec![frame]),
		SnapshotDuration::OneSecond | SnapshotDuration::ThreeSeconds => {
			let started = Instant::now();
			let mut frames = Vec::new();
			while started.elapsed() < Duration::from_millis(duration.capture_duration_ms()) {
				let frame = capture_unmf_pose_from_capturer(http_client, bind_addr)?;
				frames.push(frame);
				std::thread::sleep(Duration::from_millis(10));
			}
			if frames.is_empty() {
				frames.push(capture_unmf_pose_from_capturer(http_client, bind_addr)?);
			}
			Ok(frames)
		}
	}
}

fn response_error(method: &str, url: &str, response: reqwest::blocking::Response) -> String {
	let status = response.status();
	let body = response.text().unwrap_or_default();
	let body = body.trim();
	if body.is_empty() {
		format!("{method} {url} HTTP {status}")
	} else {
		format!("{method} {url} HTTP {status}: {body}")
	}
}

fn save_unmf_jsonl(
	frames: Vec<serde_json::Value>,
	default_file_name: String,
	app_settings: &Mutex<AppRuntimeSettings>,
) -> Result<Option<String>, String> {
	let initial_dir = snapshot_save_initial_dir(app_settings);
	let path = rfd::FileDialog::new()
		.set_directory(initial_dir)
		.set_file_name(default_file_name)
		.add_filter("UNMotionFrame JSONL", &["jsonl"])
		.add_filter("JSON", &["json"])
		.add_filter("All files", &["*"])
		.save_file();
	let Some(path) = path else {
		return Ok(None);
	};
	let mut content = String::new();
	for frame in frames {
		content.push_str(&serde_json::to_string(&frame).map_err(|error| format!("encode UNMotionFrame JSONL: {error}"))?);
		content.push('\n');
	}
	fs::write(&path, content.as_bytes()).map_err(|error| format!("write UNMotionFrame JSONL: {error}"))?;
	if snapshot_analysis_extras_enabled(app_settings) {
		write_snapshot_analysis_sidecar(&path, &content)?;
	}
	if let Some(parent) = path.parent() {
		update_snapshot_save_dir(app_settings, parent)?;
	}
	Ok(Some(path.display().to_string()))
}

fn snapshot_analysis_extras_enabled(app_settings: &Mutex<AppRuntimeSettings>) -> bool {
	app_settings
		.lock()
		.map(|settings| settings.snapshot_save_analysis_extras)
		.unwrap_or(false)
}

fn write_snapshot_analysis_sidecar(snapshot_path: &Path, frame_jsonl: &str) -> Result<(), String> {
	let sidecar_path = snapshot_path.with_file_name(format!(
		"{}.analysis.jsonl",
		snapshot_path
			.file_name()
			.and_then(|name| name.to_str())
			.unwrap_or("snapshot.unmf.jsonl")
	));
	let captured_at_unix_ms = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|duration| duration.as_millis())
		.unwrap_or(0);
	let mut content = String::new();
	let metadata = serde_json::json!({
		"kind": "snapshot-analysis-metadata",
		"schema": "un-motion.dev.snapshot-analysis.v1",
		"capturedAtUnixMs": captured_at_unix_ms,
		"snapshotFile": snapshot_path.display().to_string(),
		"notes": [
			"UNMotionFrame JSONL is duplicated below for development analysis.",
			"Each frame also contains flattened scalar channels for jitter analysis.",
			"The final summary contains per-channel min/max/stddev and frame-to-frame delta statistics.",
			"When runtime analysis extras are enabled, input images and MediaPipe native landmark dumps are written to the sibling .analysis-extras.jsonl file and .analysis-assets directory."
		]
	});
	content.push_str(&serde_json::to_string(&metadata).map_err(|error| format!("encode snapshot analysis metadata: {error}"))?);
	content.push('\n');
	let mut stats = SnapshotAnalysisStats::default();
	let mut first_capture_timestamp_ns = None;
	for (index, line) in frame_jsonl.lines().filter(|line| !line.trim().is_empty()).enumerate() {
		let frame: serde_json::Value =
			serde_json::from_str(line).map_err(|error| format!("decode UNMotionFrame for analysis sidecar: {error}"))?;
		if first_capture_timestamp_ns.is_none() {
			first_capture_timestamp_ns = frame_capture_timestamp_ns(&frame);
		}
		let channels = flatten_unmotion_frame_channels(&frame);
		stats.observe_frame(&frame, &channels);
		let entry = serde_json::json!({
			"kind": "unmotion-frame",
			"index": index,
			"elapsedMs": frame_elapsed_ms(&frame, first_capture_timestamp_ns),
			"header": frame.get("header").cloned().unwrap_or(serde_json::Value::Null),
			"channelCount": channels.len(),
			"channels": channels,
			"frame": frame,
		});
		content.push_str(&serde_json::to_string(&entry).map_err(|error| format!("encode snapshot analysis frame: {error}"))?);
		content.push('\n');
	}
	let summary = serde_json::json!({
		"kind": "snapshot-analysis-summary",
		"schema": "un-motion.dev.snapshot-analysis.v1",
		"frames": stats.frames,
		"durationMs": stats.duration_ms(),
		"channels": stats.channel_summary(),
		"rotationDeltaRad": stats.rotation_summary(),
		"strongestScalarDelta": stats.top_scalar_delta_channels(24),
		"strongestRotationDeltaRad": stats.top_rotation_delta_channels(24),
	});
	content.push_str(&serde_json::to_string(&summary).map_err(|error| format!("encode snapshot analysis summary: {error}"))?);
	content.push('\n');
	fs::write(&sidecar_path, content.as_bytes()).map_err(|error| format!("write snapshot analysis sidecar: {error}"))
}

struct PendingRuntimeAnalysisExtras {
	capturers: Vec<PendingCapturerAnalysisExtras>,
}

struct PendingCapturerAnalysisExtras {
	bind_addr: SocketAddr,
	temp_dir: PathBuf,
	response: serde_json::Value,
}

fn capture_runtime_analysis_extras_to_temp(
	http_client: &reqwest::blocking::Client,
	targets: &[SocketAddr],
	duration: SnapshotDuration,
) -> Result<PendingRuntimeAnalysisExtras, String> {
	let base = std::env::temp_dir().join(format!(
		"un-motion-analysis-extras-{}-{}",
		std::process::id(),
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map(|duration| duration.as_millis())
			.unwrap_or(0)
	));
	fs::create_dir_all(&base).map_err(|error| format!("create temp analysis dir: {error}"))?;
	let mut capturers = Vec::with_capacity(targets.len());
	for (index, bind_addr) in targets.iter().enumerate() {
		let temp_dir = base.join(format!("capturer-{index}-{}", bind_addr.port()));
		fs::create_dir_all(&temp_dir).map_err(|error| format!("create temp capturer analysis dir: {error}"))?;
		let response = capture_runtime_analysis_extras(http_client, *bind_addr, &temp_dir, duration)?;
		capturers.push(PendingCapturerAnalysisExtras {
			bind_addr: *bind_addr,
			temp_dir,
			response,
		});
	}
	Ok(PendingRuntimeAnalysisExtras { capturers })
}

fn write_runtime_analysis_extras(
	snapshot_path: &str,
	pending: &PendingRuntimeAnalysisExtras,
	duration: SnapshotDuration,
) -> Result<(), String> {
	let snapshot_path = PathBuf::from(snapshot_path);
	let base_name = snapshot_path
		.file_name()
		.and_then(|name| name.to_str())
		.unwrap_or("snapshot.unmf.jsonl");
	let parent = snapshot_path.parent().unwrap_or_else(|| Path::new("."));
	let assets_dir = parent.join(format!("{base_name}.analysis-assets"));
	fs::create_dir_all(&assets_dir).map_err(|error| format!("create analysis assets dir: {error}"))?;
	let sidecar_path = snapshot_path.with_file_name(format!("{base_name}.analysis-extras.jsonl"));
	let mut content = String::new();
	let metadata = serde_json::json!({
		"kind": "runtime-analysis-extras-metadata",
		"schema": "un-motion.dev.analysis-extras.v1",
		"snapshotFile": snapshot_path.display().to_string(),
		"assetsDir": assets_dir.display().to_string(),
		"durationMs": duration.capture_duration_ms(),
		"notes": [
			"Analysis extras are captured by the Capturer source path.",
			"MediaPipe webcam sources include input RGB PNG paths, native landmarks, and final UNMotionFrame samples."
		],
	});
	content.push_str(&serde_json::to_string(&metadata).map_err(|error| format!("encode analysis extras metadata: {error}"))?);
	content.push('\n');
	for (index, pending_capturer) in pending.capturers.iter().enumerate() {
		let target_dir = assets_dir.join(format!("capturer-{index}-{}", pending_capturer.bind_addr.port()));
		move_analysis_assets(&pending_capturer.temp_dir, &target_dir)?;
		let entry = serde_json::json!({
			"kind": "runtime-analysis-extras",
			"capturerBind": pending_capturer.bind_addr.to_string(),
			"assetsDir": target_dir.display().to_string(),
			"response": rewrite_analysis_asset_paths(&pending_capturer.response, &pending_capturer.temp_dir, &target_dir),
		});
		content.push_str(&serde_json::to_string(&entry).map_err(|error| format!("encode analysis extras entry: {error}"))?);
		content.push('\n');
	}
	fs::write(&sidecar_path, content.as_bytes()).map_err(|error| format!("write analysis extras sidecar: {error}"))
}

fn move_analysis_assets(from: &Path, to: &Path) -> Result<(), String> {
	fs::create_dir_all(to).map_err(|error| format!("create analysis assets dir: {error}"))?;
	for entry in fs::read_dir(from).map_err(|error| format!("read temp analysis assets: {error}"))? {
		let entry = entry.map_err(|error| format!("read temp analysis asset entry: {error}"))?;
		let source = entry.path();
		let target = to.join(entry.file_name());
		if source.is_file() {
			if target.exists() {
				fs::remove_file(&target).map_err(|error| format!("replace analysis asset: {error}"))?;
			}
			fs::rename(&source, &target)
				.or_else(|_| {
					fs::copy(&source, &target)?;
					fs::remove_file(&source)
				})
				.map_err(|error| format!("move analysis asset: {error}"))?;
		}
	}
	let _ = fs::remove_dir(from);
	Ok(())
}

fn rewrite_analysis_asset_paths(value: &serde_json::Value, from: &Path, to: &Path) -> serde_json::Value {
	match value {
		serde_json::Value::String(text) => {
			let from_text = from.display().to_string();
			if text.contains(&from_text) {
				serde_json::Value::String(text.replace(&from_text, &to.display().to_string()))
			} else {
				value.clone()
			}
		}
		serde_json::Value::Array(items) => {
			serde_json::Value::Array(items.iter().map(|item| rewrite_analysis_asset_paths(item, from, to)).collect())
		}
		serde_json::Value::Object(map) => serde_json::Value::Object(
			map.iter()
				.map(|(key, item)| (key.clone(), rewrite_analysis_asset_paths(item, from, to)))
				.collect(),
		),
		_ => value.clone(),
	}
}

fn capture_runtime_analysis_extras(
	http_client: &reqwest::blocking::Client,
	bind_addr: SocketAddr,
	output_dir: &Path,
	duration: SnapshotDuration,
) -> Result<serde_json::Value, String> {
	let url = format!("http://{bind_addr}/api/runtime/analysis-extras");
	let request = serde_json::json!({
		"outputDir": output_dir,
		"durationMs": duration.capture_duration_ms(),
	});
	http_client
		.post(&url)
		.timeout(Duration::from_millis(duration.capture_duration_ms()).saturating_add(HTTP_RUNTIME_ACTION_TIMEOUT))
		.json(&request)
		.send()
		.map_err(|error| format!("POST {url} failed: {error}"))?
		.error_for_status()
		.map_err(|error| format!("POST {url} failed: {error}"))?
		.json()
		.map_err(|error| format!("decode analysis extras response: {error}"))
}

#[derive(Default)]
struct SnapshotAnalysisStats {
	frames: usize,
	first_capture_timestamp_ns: Option<u64>,
	last_capture_timestamp_ns: Option<u64>,
	channels: BTreeMap<String, NumericChannelStats>,
	rotations: BTreeMap<String, RotationChannelStats>,
}

impl SnapshotAnalysisStats {
	fn observe_frame(&mut self, frame: &serde_json::Value, channels: &BTreeMap<String, f64>) {
		self.frames = self.frames.saturating_add(1);
		if let Some(timestamp) = frame_capture_timestamp_ns(frame) {
			self.first_capture_timestamp_ns.get_or_insert(timestamp);
			self.last_capture_timestamp_ns = Some(timestamp);
		}
		for (name, value) in channels {
			self.channels.entry(name.clone()).or_default().observe(*value);
		}
		for (name, quat) in flatten_unmotion_frame_quaternions(frame) {
			self.rotations.entry(name).or_default().observe(quat);
		}
	}

	fn duration_ms(&self) -> Option<f64> {
		let first = self.first_capture_timestamp_ns?;
		let last = self.last_capture_timestamp_ns?;
		(last >= first).then_some((last - first) as f64 / 1_000_000.0)
	}

	fn channel_summary(&self) -> BTreeMap<String, serde_json::Value> {
		self.channels
			.iter()
			.map(|(name, stats)| (name.clone(), stats.summary_value()))
			.collect()
	}

	fn rotation_summary(&self) -> BTreeMap<String, serde_json::Value> {
		self.rotations
			.iter()
			.map(|(name, stats)| (name.clone(), stats.summary_value()))
			.collect()
	}

	fn top_scalar_delta_channels(&self, limit: usize) -> Vec<serde_json::Value> {
		let mut items = self
			.channels
			.iter()
			.filter_map(|(name, stats)| {
				(stats.max_abs_delta > 0.0).then(|| {
					serde_json::json!({
						"name": name,
						"maxAbsDelta": stats.max_abs_delta,
						"meanAbsDelta": stats.mean_abs_delta(),
						"stddev": stats.stddev(),
					})
				})
			})
			.collect::<Vec<_>>();
		items.sort_by(|a, b| json_f64_field(b, "maxAbsDelta").total_cmp(&json_f64_field(a, "maxAbsDelta")));
		items.truncate(limit);
		items
	}

	fn top_rotation_delta_channels(&self, limit: usize) -> Vec<serde_json::Value> {
		let mut items = self
			.rotations
			.iter()
			.filter_map(|(name, stats)| {
				(stats.max_delta_rad > 0.0).then(|| {
					serde_json::json!({
						"name": name,
						"maxDeltaRad": stats.max_delta_rad,
						"meanDeltaRad": stats.mean_delta_rad(),
						"maxDeltaDeg": stats.max_delta_rad.to_degrees(),
						"meanDeltaDeg": stats.mean_delta_rad().to_degrees(),
					})
				})
			})
			.collect::<Vec<_>>();
		items.sort_by(|a, b| json_f64_field(b, "maxDeltaRad").total_cmp(&json_f64_field(a, "maxDeltaRad")));
		items.truncate(limit);
		items
	}
}

#[derive(Default)]
struct NumericChannelStats {
	count: usize,
	min: f64,
	max: f64,
	sum: f64,
	sum_sq: f64,
	prev: Option<f64>,
	delta_count: usize,
	abs_delta_sum: f64,
	max_abs_delta: f64,
}

impl NumericChannelStats {
	fn observe(&mut self, value: f64) {
		if !value.is_finite() {
			return;
		}
		if self.count == 0 {
			self.min = value;
			self.max = value;
		} else {
			self.min = self.min.min(value);
			self.max = self.max.max(value);
		}
		self.count = self.count.saturating_add(1);
		self.sum += value;
		self.sum_sq += value * value;
		if let Some(prev) = self.prev {
			let delta = (value - prev).abs();
			self.delta_count = self.delta_count.saturating_add(1);
			self.abs_delta_sum += delta;
			self.max_abs_delta = self.max_abs_delta.max(delta);
		}
		self.prev = Some(value);
	}

	fn mean(&self) -> f64 {
		if self.count == 0 { 0.0 } else { self.sum / self.count as f64 }
	}

	fn stddev(&self) -> f64 {
		if self.count < 2 {
			return 0.0;
		}
		let mean = self.mean();
		((self.sum_sq / self.count as f64) - mean * mean).max(0.0).sqrt()
	}

	fn mean_abs_delta(&self) -> f64 {
		if self.delta_count == 0 {
			0.0
		} else {
			self.abs_delta_sum / self.delta_count as f64
		}
	}

	fn summary_value(&self) -> serde_json::Value {
		serde_json::json!({
			"count": self.count,
			"min": self.min,
			"max": self.max,
			"range": if self.count == 0 { 0.0 } else { self.max - self.min },
			"mean": self.mean(),
			"stddev": self.stddev(),
			"meanAbsDelta": self.mean_abs_delta(),
			"maxAbsDelta": self.max_abs_delta,
		})
	}
}

#[derive(Default)]
struct RotationChannelStats {
	count: usize,
	prev: Option<[f64; 4]>,
	delta_count: usize,
	delta_sum_rad: f64,
	max_delta_rad: f64,
}

impl RotationChannelStats {
	fn observe(&mut self, quat: [f64; 4]) {
		let Some(quat) = normalized_quat(quat) else {
			return;
		};
		self.count = self.count.saturating_add(1);
		if let Some(prev) = self.prev {
			let delta = quat_delta_angle_rad(prev, quat);
			self.delta_count = self.delta_count.saturating_add(1);
			self.delta_sum_rad += delta;
			self.max_delta_rad = self.max_delta_rad.max(delta);
		}
		self.prev = Some(quat);
	}

	fn mean_delta_rad(&self) -> f64 {
		if self.delta_count == 0 {
			0.0
		} else {
			self.delta_sum_rad / self.delta_count as f64
		}
	}

	fn summary_value(&self) -> serde_json::Value {
		serde_json::json!({
			"count": self.count,
			"deltaCount": self.delta_count,
			"meanDeltaRad": self.mean_delta_rad(),
			"maxDeltaRad": self.max_delta_rad,
			"meanDeltaDeg": self.mean_delta_rad().to_degrees(),
			"maxDeltaDeg": self.max_delta_rad.to_degrees(),
		})
	}
}

fn flatten_unmotion_frame_channels(frame: &serde_json::Value) -> BTreeMap<String, f64> {
	let mut out = BTreeMap::new();
	if let Some(root) = frame.pointer("/body/humanoid/root") {
		flatten_transform_channels("body.root", root, &mut out);
	}
	if let Some(bones) = frame.pointer("/body/humanoid/bones").and_then(|value| value.as_array()) {
		for bone in bones {
			if let Some(name) = bone.get("bone").and_then(|value| value.as_str())
				&& let Some(transform) = bone.get("transform")
			{
				flatten_transform_channels(&format!("body.bone.{name}"), transform, &mut out);
				if let Some(confidence) = bone.get("confidence").and_then(|value| value.as_f64()) {
					out.insert(format!("body.bone.{name}.confidence"), confidence);
				}
			}
		}
	}
	if let Some(head) = frame.pointer("/face/head") {
		flatten_transform_channels("face.head", head, &mut out);
	}
	if let Some(expressions) = frame.pointer("/face/expressions").and_then(|value| value.as_array()) {
		for expression in expressions {
			if let Some(name) = expression.get("name").and_then(|value| value.as_str())
				&& let Some(value) = expression.get("value").and_then(|value| value.as_f64())
			{
				out.insert(format!("face.expression.{name}"), value);
			}
		}
	}
	flatten_hand_channels("left_hand", frame.get("left_hand"), &mut out);
	flatten_hand_channels("right_hand", frame.get("right_hand"), &mut out);
	for signal in frame.get("signals").and_then(|value| value.as_array()).into_iter().flatten() {
		if let Some(name) = signal.get("name").and_then(|value| value.as_str())
			&& let Some(value) = signal.get("value")
		{
			flatten_motion_signal_channels(&format!("signal.{name}"), value, &mut out);
		}
	}
	out
}

fn flatten_unmotion_frame_quaternions(frame: &serde_json::Value) -> BTreeMap<String, [f64; 4]> {
	let mut out = BTreeMap::new();
	if let Some(root) = frame.pointer("/body/humanoid/root") {
		flatten_transform_quaternions("body.root", root, &mut out);
	}
	if let Some(bones) = frame.pointer("/body/humanoid/bones").and_then(|value| value.as_array()) {
		for bone in bones {
			if let Some(name) = bone.get("bone").and_then(|value| value.as_str())
				&& let Some(transform) = bone.get("transform")
			{
				flatten_transform_quaternions(&format!("body.bone.{name}"), transform, &mut out);
			}
		}
	}
	if let Some(head) = frame.pointer("/face/head") {
		flatten_transform_quaternions("face.head", head, &mut out);
	}
	flatten_hand_quaternions("left_hand", frame.get("left_hand"), &mut out);
	flatten_hand_quaternions("right_hand", frame.get("right_hand"), &mut out);
	for signal in frame.get("signals").and_then(|value| value.as_array()).into_iter().flatten() {
		if let Some(name) = signal.get("name").and_then(|value| value.as_str())
			&& let Some(value) = signal.get("value")
			&& let Some(quat) = quat_from_signal_value(value)
		{
			out.insert(format!("signal.{name}.quat"), quat);
		}
	}
	out
}

fn flatten_transform_channels(prefix: &str, transform: &serde_json::Value, out: &mut BTreeMap<String, f64>) {
	if let Some(translation) = transform.get("translation") {
		flatten_vec3_channels(&format!("{prefix}.translation"), translation, out);
	}
	if let Some(rotation) = transform.get("rotation") {
		flatten_quat_channels(&format!("{prefix}.rotation"), rotation, out);
	}
	if let Some(scale) = transform.get("scale") {
		flatten_vec3_channels(&format!("{prefix}.scale"), scale, out);
	}
}

fn flatten_transform_quaternions(prefix: &str, transform: &serde_json::Value, out: &mut BTreeMap<String, [f64; 4]>) {
	if let Some(rotation) = transform.get("rotation").and_then(quat_from_object) {
		out.insert(format!("{prefix}.rotation"), rotation);
	}
}

fn flatten_hand_channels(prefix: &str, hand: Option<&serde_json::Value>, out: &mut BTreeMap<String, f64>) {
	let Some(hand) = hand else {
		return;
	};
	if let Some(wrist) = hand.get("wrist") {
		flatten_transform_channels(&format!("{prefix}.wrist"), wrist, out);
	}
	if let Some(fingers) = hand.get("fingers").and_then(|value| value.as_array()) {
		for finger in fingers {
			let Some(finger_name) = finger.get("finger").and_then(|value| value.as_str()) else {
				continue;
			};
			if let Some(confidence) = finger.get("confidence").and_then(|value| value.as_f64()) {
				out.insert(format!("{prefix}.finger.{finger_name}.confidence"), confidence);
			}
			if let Some(joints) = finger.get("joints").and_then(|value| value.as_array()) {
				for (index, joint) in joints.iter().enumerate() {
					flatten_transform_channels(&format!("{prefix}.finger.{finger_name}.joint.{index}"), joint, out);
				}
			}
		}
	}
}

fn flatten_hand_quaternions(prefix: &str, hand: Option<&serde_json::Value>, out: &mut BTreeMap<String, [f64; 4]>) {
	let Some(hand) = hand else {
		return;
	};
	if let Some(wrist) = hand.get("wrist") {
		flatten_transform_quaternions(&format!("{prefix}.wrist"), wrist, out);
	}
	if let Some(fingers) = hand.get("fingers").and_then(|value| value.as_array()) {
		for finger in fingers {
			let Some(finger_name) = finger.get("finger").and_then(|value| value.as_str()) else {
				continue;
			};
			if let Some(joints) = finger.get("joints").and_then(|value| value.as_array()) {
				for (index, joint) in joints.iter().enumerate() {
					flatten_transform_quaternions(&format!("{prefix}.finger.{finger_name}.joint.{index}"), joint, out);
				}
			}
		}
	}
}

fn flatten_vec3_channels(prefix: &str, value: &serde_json::Value, out: &mut BTreeMap<String, f64>) {
	if let Some(x) = value.get("x").and_then(|value| value.as_f64()) {
		out.insert(format!("{prefix}.x"), x);
	}
	if let Some(y) = value.get("y").and_then(|value| value.as_f64()) {
		out.insert(format!("{prefix}.y"), y);
	}
	if let Some(z) = value.get("z").and_then(|value| value.as_f64()) {
		out.insert(format!("{prefix}.z"), z);
	}
}

fn flatten_quat_channels(prefix: &str, value: &serde_json::Value, out: &mut BTreeMap<String, f64>) {
	if let Some(x) = value.get("x").and_then(|value| value.as_f64()) {
		out.insert(format!("{prefix}.x"), x);
	}
	if let Some(y) = value.get("y").and_then(|value| value.as_f64()) {
		out.insert(format!("{prefix}.y"), y);
	}
	if let Some(z) = value.get("z").and_then(|value| value.as_f64()) {
		out.insert(format!("{prefix}.z"), z);
	}
	if let Some(w) = value.get("w").and_then(|value| value.as_f64()) {
		out.insert(format!("{prefix}.w"), w);
	}
}

fn flatten_motion_signal_channels(prefix: &str, value: &serde_json::Value, out: &mut BTreeMap<String, f64>) {
	if let Some(scalar) = value.get("Scalar").and_then(|value| value.as_f64()) {
		out.insert(prefix.to_string(), scalar);
		return;
	}
	if let Some(vec3) = value.get("Vec3") {
		flatten_vec3_channels(prefix, vec3, out);
		return;
	}
	if let Some(quat) = value.get("Quat") {
		flatten_quat_channels(prefix, quat, out);
	}
}

fn quat_from_signal_value(value: &serde_json::Value) -> Option<[f64; 4]> {
	value.get("Quat").and_then(quat_from_object)
}

fn quat_from_object(value: &serde_json::Value) -> Option<[f64; 4]> {
	Some([
		value.get("x")?.as_f64()?,
		value.get("y")?.as_f64()?,
		value.get("z")?.as_f64()?,
		value.get("w")?.as_f64()?,
	])
}

fn normalized_quat(quat: [f64; 4]) -> Option<[f64; 4]> {
	let len = (quat[0] * quat[0] + quat[1] * quat[1] + quat[2] * quat[2] + quat[3] * quat[3]).sqrt();
	(len > 0.0 && len.is_finite()).then_some([quat[0] / len, quat[1] / len, quat[2] / len, quat[3] / len])
}

fn quat_delta_angle_rad(a: [f64; 4], b: [f64; 4]) -> f64 {
	let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().clamp(0.0, 1.0);
	2.0 * dot.acos()
}

fn frame_capture_timestamp_ns(frame: &serde_json::Value) -> Option<u64> {
	frame.pointer("/header/capture_timestamp_ns").and_then(|value| value.as_u64())
}

fn frame_elapsed_ms(frame: &serde_json::Value, first_capture_timestamp_ns: Option<u64>) -> Option<f64> {
	let first = first_capture_timestamp_ns?;
	let current = frame_capture_timestamp_ns(frame)?;
	(current >= first).then_some((current - first) as f64 / 1_000_000.0)
}

fn json_f64_field(value: &serde_json::Value, field: &str) -> f64 {
	value.get(field).and_then(|value| value.as_f64()).unwrap_or(0.0)
}

fn snapshot_save_initial_dir(app_settings: &Mutex<AppRuntimeSettings>) -> PathBuf {
	app_settings
		.lock()
		.ok()
		.and_then(|settings| settings.snapshot_save_dir.as_deref().map(PathBuf::from))
		.filter(|path| path.is_dir())
		.unwrap_or_else(default_snapshot_save_dir)
}

fn update_snapshot_save_dir(app_settings: &Mutex<AppRuntimeSettings>, dir: &Path) -> Result<(), String> {
	let dir = dir.to_string_lossy().to_string();
	let mut settings = app_settings.lock().map_err(|_| "app settings state poisoned".to_string())?;
	if settings.snapshot_save_dir.as_deref() == Some(dir.as_str()) {
		return Ok(());
	}
	settings.snapshot_save_dir = Some(dir);
	normalize_app_settings(&mut settings);
	write_app_settings(&settings)
}

fn default_snapshot_save_dir() -> PathBuf {
	if let Some(path) = env::var_os("USERPROFILE") {
		let documents = PathBuf::from(path).join("Documents");
		if documents.is_dir() {
			return documents;
		}
	}
	if let Some(path) = env::var_os("HOME") {
		let home = PathBuf::from(path);
		let documents = home.join("Documents");
		if documents.is_dir() {
			return documents;
		}
		return home;
	}
	repo_root()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileDocumentEnvelope {
	selection: CoreProfileDocument,
}

/// Profile 一覧。Supervisor の Profiles タブは編集・並び替えの権威であるローカル
/// `CoreProfileDocumentStore` を直接読む。稼働中 Capturer 側の HTTP 一覧は push 反映の
/// 瞬間に古い順序を返すことがあるため、ここでは UI の表示順に使わない。
#[tauri::command]
fn list_profiles(_state: State<'_, Mutex<SupervisorState>>) -> Result<Vec<ProfileSummary>, String> {
	Ok(user_profile_store().load().profiles_summary())
}

/// Supervisor Console の前回選択 profile id を返す。
///
/// これは Launch UI の復元用であり、稼働中 Capturer の runtime state ではない。
#[tauri::command]
fn selected_profile_id(_state: State<'_, Mutex<SupervisorState>>) -> Result<String, String> {
	Ok(user_profile_store().load().selected_profile_id)
}

/// Supervisor Console の前回選択 profile id を保存する。
///
/// 稼働中 Capturer には触れない。実行 profile は `launch_capturer(profile_id)` が
/// プロセス単位で決める。
#[tauri::command]
fn set_selected_profile_id(profile_id: Option<String>, _state: State<'_, Mutex<SupervisorState>>) -> Result<(), String> {
	let store = user_profile_store();
	let mut doc = store.load();
	let normalized = profile_id
		.map(|value| value.trim().to_string())
		.filter(|value| !value.is_empty())
		.unwrap_or_default();
	if !normalized.is_empty() && !doc.profiles.iter().any(|profile| profile.id == normalized) {
		return Err(format!("profile not found: {normalized}"));
	}
	if doc.selected_profile_id == normalized {
		return Ok(());
	}
	doc.selected_profile_id = normalized;
	store.save(doc).map_err(|e| format!("save profile document: {e}"))?;
	Ok(())
}

/// Profile を新規作成する。`name` を slug 化した id を生成し、既存と重複しないように
/// 連番サフィックスを付ける。新規 profile は desktop 配信向けに Legs/Feet filter
/// を OFF で開始する。
#[tauri::command]
fn create_profile(name: String, _state: State<'_, Mutex<SupervisorState>>) -> Result<ProfileDetail, String> {
	let trimmed = name.trim();
	let name = if trimmed.is_empty() {
		"New Profile".to_string()
	} else {
		trimmed.to_string()
	};
	let store = user_profile_store();
	let mut doc = store.load();
	let id = next_profile_id(&doc, &name);
	let new_profile = CoreProfileDocumentProfile {
		id: id.clone(),
		name,
		created_at: String::new(),
		note: String::new(),
		default_source_enabled: true,
		default_source_label: "Default".to_string(),
		icon_path: None,
		group: String::new(),
		runtime_selection: Some(new_profile_runtime_defaults()),
		pipeline_components: None,
	};
	doc.profiles.push(new_profile);
	doc.selected_profile_id = id.clone();
	let saved = store.save(doc).map_err(|e| format!("save profile document: {e}"))?;
	let detail = saved
		.profiles
		.iter()
		.find(|profile| profile.id == id)
		.map(ProfileDetail::from)
		.ok_or_else(|| format!("created profile vanished after save: {id}"))?;
	Ok(detail)
}

fn new_profile_runtime_defaults() -> ProfileRuntimeSettings {
	ProfileRuntimeSettings {
		engine: Some("mediapipe-native".to_string()),
		vmc_enabled: Some(false),
		zenoh_enabled: Some(true),
		zenoh_key_expr: Some("un-motion/frame".to_string()),
		zenoh_topic_mode: Some("frame".to_string()),
		zenoh_producer: Some("un-motion-capturer".to_string()),
		media_pipe_delegate: Some("xnnpack".to_string()),
		media_pipe_num_threads: Some(2),
		media_pipe_holistic_flow_limiter_enabled: Some(true),
		media_pipe_holistic_flow_limiter_max_in_flight: Some(1),
		media_pipe_holistic_flow_limiter_max_in_queue: Some(1),
		modifier: Some(ProfileModifierSettings {
			head_enabled: Some(true),
			face_enabled: Some(true),
			hands_enabled: Some(true),
			arms_ik_enabled: Some(true),
			torso_enabled: Some(true),
			legs_enabled: Some(false),
			feet_enabled: Some(false),
			torso_pitch_scale: Some(1.0),
			smoothing_ema_enabled: Some(false),
			smoothing_ema_alpha: Some(0.7),
			smoothing_one_euro_enabled: Some(true),
			smoothing_confidence_adaptive_cutoff: Some(true),
			adaptive_min_cutoff_hz: Some(1.0),
			adaptive_beta: Some(0.12),
			adaptive_derivative_cutoff_hz: Some(1.0),
			..ProfileModifierSettings::default()
		}),
		..ProfileRuntimeSettings::default()
	}
}

/// Profile を削除する。最後の 1 件も削除できる。空になったら launch default は空文字にする。
#[tauri::command]
fn delete_profile(profile_id: String, state: State<'_, Mutex<SupervisorState>>) -> Result<(), String> {
	let state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	let store = user_profile_store();
	let mut doc = store.load();
	if !doc.profiles.iter().any(|profile| profile.id == profile_id) {
		return Err(format!("profile not found: {profile_id}"));
	}
	doc.profiles.retain(|profile| profile.id != profile_id);
	doc.profile_sources.retain(|source| source.profile_id != profile_id);
	if doc.selected_profile_id == profile_id {
		doc.selected_profile_id = doc.profiles.first().map(|profile| profile.id.clone()).unwrap_or_default();
	}
	if state.capturers.values().any(|capturer| {
		matches!(
			capturer.info.state,
			CapturerState::Starting | CapturerState::Running | CapturerState::Stopping
		) && capturer.info.profile_id.as_deref() == Some(profile_id.as_str())
	}) {
		return Err(format!("profile is in use by a running capturer: {profile_id}"));
	}
	store.save(doc).map_err(|e| format!("save profile document: {e}"))?;
	Ok(())
}

/// Profile を複製する。新しい id / 表示名は元のものから ` copy` を付けて自動採番する。
#[tauri::command]
fn duplicate_profile(profile_id: String, _state: State<'_, Mutex<SupervisorState>>) -> Result<ProfileDetail, String> {
	let store = user_profile_store();
	let mut doc = store.load();
	let original = doc
		.profiles
		.iter()
		.find(|profile| profile.id == profile_id)
		.cloned()
		.ok_or_else(|| format!("profile not found: {profile_id}"))?;
	let new_name = next_profile_name(&doc, &format!("{} copy", original.name));
	let new_id = next_profile_id(&doc, &new_name);
	let duplicated = CoreProfileDocumentProfile {
		id: new_id.clone(),
		name: new_name,
		created_at: String::new(),
		..original
	};
	doc.profiles.push(duplicated);
	let saved = store.save(doc).map_err(|e| format!("save profile document: {e}"))?;
	let detail = saved
		.profiles
		.iter()
		.find(|profile| profile.id == new_id)
		.map(ProfileDetail::from)
		.ok_or_else(|| format!("duplicated profile vanished after save: {new_id}"))?;
	Ok(detail)
}

#[tauri::command]
fn reorder_profiles(profile_ids: Vec<String>, _state: State<'_, Mutex<SupervisorState>>) -> Result<Vec<ProfileSummary>, String> {
	let store = user_profile_store();
	let mut doc = store.load();
	let mut ordered = Vec::with_capacity(doc.profiles.len());
	for id in &profile_ids {
		if let Some(index) = doc.profiles.iter().position(|profile| &profile.id == id) {
			ordered.push(doc.profiles.remove(index));
		}
	}
	ordered.append(&mut doc.profiles);
	doc.profiles = ordered;
	let saved = store.save(doc).map_err(|e| format!("save profile document: {e}"))?;
	Ok(saved.profiles_summary())
}

/// Profile の詳細フィールドを GUI 用 DTO で返す。
#[tauri::command]
fn get_profile_detail(profile_id: String) -> Result<ProfileDetail, String> {
	let doc = user_profile_store().load();
	doc.profiles
		.iter()
		.find(|profile| profile.id == profile_id)
		.map(ProfileDetail::from)
		.ok_or_else(|| format!("profile not found: {profile_id}"))
}

/// Profile の個別フィールドを更新する。`field` はドット区切りパス (例: `"name"` /
/// `"runtime_selection.fps"`)、`value` は `serde_json::Value` (null で `Option::None`
/// を表現)。UN Avatar Supervisor の `update_avatar_setting_value` 規約に倣う。
#[tauri::command]
fn update_profile_field(
	profile_id: String,
	field: String,
	value: serde_json::Value,
	state: State<'_, Mutex<SupervisorState>>,
) -> Result<ProfileDetail, String> {
	let state = state.lock().map_err(|_| "supervisor state poisoned".to_string())?;
	let store = user_profile_store();
	let mut doc = store.load();
	{
		let profile = doc
			.profiles
			.iter_mut()
			.find(|profile| profile.id == profile_id)
			.ok_or_else(|| format!("profile not found: {profile_id}"))?;
		apply_profile_field(profile, &field, value)?;
	}
	let saved = store.save(doc).map_err(|e| format!("save profile document: {e}"))?;
	push_document_to_matching_capturers(&state, &saved, &profile_id);
	saved
		.profiles
		.iter()
		.find(|profile| profile.id == profile_id)
		.map(ProfileDetail::from)
		.ok_or_else(|| format!("profile vanished after save: {profile_id}"))
}

/// `package.json` の version を取りたいが、Phase D 初期は Cargo.toml の version をそのまま返す。
#[tauri::command]
fn app_version() -> String {
	env!("CARGO_PKG_VERSION").to_string()
}

/// GUI Camera device dropdown 向けの 1 件分。`id` は backend が `set_device` で
/// 受け取れる安定識別子 (DirectShow: moniker UUID / MediaFoundation: index 文字列 や
/// `cam0` 等)、`label` は表示用 (`Cam Link 4K` のような UTF-8 文字列)。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebcamDeviceDto {
	pub id: String,
	pub label: String,
}

/// MediaPipe webcam source type 別にデバイス一覧を返す Tauri command。
/// `backend` は `"directshow"` または `"mediafoundation"`。それ以外は空配列を返す。
///
/// Windows ビルドのみ実装で、非 Windows では常に空配列を返す。
/// 失敗時はメッセージのみ返して GUI が text input fallback できるようにする
/// (空配列 / Error の判別を GUI 側で扱う)。
#[tauri::command]
fn enumerate_webcams(backend: String) -> Result<Vec<WebcamDeviceDto>, String> {
	#[cfg(target_os = "windows")]
	{
		match backend.as_str() {
			"directshow" => match un_motion_input_webcam_directshow::list_directshow_devices() {
				Ok(devices) => Ok(devices.into_iter().map(|d| WebcamDeviceDto { id: d.id, label: d.name }).collect()),
				Err(e) => Err(format!("DirectShow enumeration failed: {e}")),
			},
			"mediafoundation" => match un_motion_input_webcam_nokhwa::list_mediafoundation_devices() {
				Ok(devices) => Ok(devices.into_iter().map(|d| WebcamDeviceDto { id: d.id, label: d.name }).collect()),
				Err(e) => Err(format!("MediaFoundation enumeration failed: {e}")),
			},
			other => Err(format!("unknown webcam backend: {other}")),
		}
	}
	#[cfg(not(target_os = "windows"))]
	{
		let _ = backend;
		Ok(Vec::new())
	}
}

/// 選択された Camera device で使用可能な (resolution, fps, pixel_format) の組み合わせを
/// 返す。DirectShow backend のみ対応 (MediaFoundation は nokhwa の `compatible_camera_format`
/// 経由で取れるが Phase 4b に持ち越す)。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebcamFormatDto {
	pub width: u32,
	pub height: u32,
	/// `None` の場合は backend が fps を報告しなかったもの。GUI は "auto" 等で表示する。
	pub fps: Option<u32>,
	pub pixel_format: String,
	/// `1280x720@30 NV12` のような表示用ラベル (`DirectShowCaptureFormatInfo::native_label`)。
	pub label: String,
}

#[tauri::command]
fn enumerate_webcam_formats(backend: String, device_id_or_name: String) -> Result<Vec<WebcamFormatDto>, String> {
	#[cfg(target_os = "windows")]
	{
		match backend.as_str() {
			"directshow" => match un_motion_input_webcam_directshow::list_directshow_capture_formats(&device_id_or_name) {
				Ok(formats) => Ok(formats
					.into_iter()
					.map(|f| WebcamFormatDto {
						width: f.width,
						height: f.height,
						fps: f.fps,
						pixel_format: f.pixel_format.clone(),
						label: f.native_label(),
					})
					.collect()),
				Err(e) => Err(format!("DirectShow format enumeration failed: {e}")),
			},
			"mediafoundation" => match un_motion_input_webcam_nokhwa::list_mediafoundation_capture_formats(&device_id_or_name) {
				Ok(formats) => Ok(formats
					.into_iter()
					.map(|f| WebcamFormatDto {
						width: f.width,
						height: f.height,
						fps: f.fps,
						pixel_format: f.pixel_format.clone(),
						label: f.native_label(),
					})
					.collect()),
				// MediaFoundation 経路で Camera::new が失敗する一般的な要因は:
				// * カメラが別アプリ (Discord / OBS / iFacialMocap / etc.) で開かれている
				// * 仮想カメラ (OBS Virtual Camera) で MSMF query 未実装
				// * 権限拒否
				// いずれもユーザーが操作できる問題なので Err 文言は短くまとめる。
				// GUI は警告メッセージとして表示するだけで他 backend を破壊しない。
				Err(e) => Err(format!("MediaFoundation format enumeration failed: {e}")),
			},
			other => Err(format!("unknown webcam backend: {other}")),
		}
	}
	#[cfg(not(target_os = "windows"))]
	{
		let _ = (backend, device_id_or_name);
		Ok(Vec::new())
	}
}

/// 動作中の Capturer をすべて停止する。CloseRequested ハンドラと Tauri command の両方
/// から呼ばれるので、`SupervisorState` への lock 取得とエラーハンドリングをここに集約。
fn stop_all_in_state(state: &Mutex<SupervisorState>) {
	let Ok(mut state) = state.lock() else {
		return;
	};
	let http_client = state.http_client.clone();
	let ids: Vec<u32> = state.capturers.keys().copied().collect();
	for id in ids {
		if let Some(capturer) = state.capturers.get_mut(&id) {
			if matches!(capturer.info.state, CapturerState::Exited | CapturerState::Crashed) {
				continue;
			}
			if let Err(e) = stop_managed_capturer(id, capturer, &http_client) {
				tracing::warn!(target: "un_motion_supervisor", capturer_id = id, error = %e, "stop_all: stop_managed_capturer failed");
			}
		}
	}
}

/// AppHandle から「Supervisor Console 終了時に Capturer を全停止すべきか」を読む。
/// AppRuntimeSettings state が未登録 / 取得失敗時は安全側の既定値 (= 停止する) を返す。
fn should_stop_capturers_on_exit(app_handle: &tauri::AppHandle) -> bool {
	app_handle
		.try_state::<Mutex<AppRuntimeSettings>>()
		.and_then(|s| s.lock().ok().map(|s| s.stop_capturers_on_exit))
		.unwrap_or(true)
}

#[tauri::command]
fn get_app_settings(state: State<'_, Mutex<AppRuntimeSettings>>) -> Result<AppRuntimeSettings, String> {
	state
		.lock()
		.map(|s| s.clone())
		.map_err(|_| "app settings state poisoned".to_string())
}

/// `stop_capturers_on_exit` を更新して TOML に永続化する。GUI Settings タブから呼ぶ。
#[tauri::command]
fn set_stop_capturers_on_exit(value: bool, state: State<'_, Mutex<AppRuntimeSettings>>) -> Result<AppRuntimeSettings, String> {
	let mut guard = state.lock().map_err(|_| "app settings state poisoned".to_string())?;
	guard.stop_capturers_on_exit = value;
	let snapshot = guard.clone();
	drop(guard);
	write_app_settings(&snapshot)?;
	Ok(snapshot)
}

/// UN Avatar Supervisor の同名 command と同じく、Svelte 側から
/// `AppRuntimeSettings` 全体を 1 リクエストで送って永続化する。
/// 個別 toggle ごとに往復するより記述量が少なく、Settings タブの「保存タイミング」を
/// 単一にできる。値の検証はサーバ側で行う ( theme_mode などの enum 系)。
#[tauri::command]
fn sync_app_settings(
	app: tauri::AppHandle,
	mut settings: AppRuntimeSettings,
	state: State<'_, Mutex<AppRuntimeSettings>>,
) -> Result<AppRuntimeSettings, String> {
	normalize_app_settings(&mut settings);
	let old_system_tray_enabled = state.lock().map(|s| s.system_tray_enabled).unwrap_or(false);
	let old_locale = state.lock().map(|s| s.locale.clone()).unwrap_or_default();
	let mut guard = state.lock().map_err(|_| "app settings state poisoned".to_string())?;
	if old_system_tray_enabled != settings.system_tray_enabled {
		if settings.system_tray_enabled {
			setup_tray(&app).map_err(|e| format!("setup tray: {e}"))?;
		} else {
			drop(app.remove_tray_by_id(TRAY_ICON_ID));
		}
	}
	// locale が変わったら rust-i18n のグローバル locale を即時切り替え、tray menu
	// などネイティブ chrome 側の `t!()` 出力が次回参照時から新言語になる。Svelte 側は
	// 自前で `locale.set()` → loader 再実行で別途切替する。
	if old_locale != settings.locale && !settings.locale.is_empty() {
		crate::i18n::apply_locale(&settings.locale);
	}
	write_app_settings(&settings)?;
	*guard = settings.clone();
	Ok(settings)
}

fn normalize_app_settings(settings: &mut AppRuntimeSettings) {
	match settings.theme_mode.as_str() {
		"light" | "dark" | "system" => {}
		_ => settings.theme_mode = default_theme_mode(),
	}
	// 不正な locale (TOML が無い) は空文字 (= 自動解決) に戻す。
	if !settings.locale.is_empty() && !crate::i18n::UN_I18N_STORE.has_locale(&settings.locale) {
		tracing::warn!(locale = %settings.locale, "i18n: unsupported locale value, resetting to auto");
		settings.locale.clear();
	}
	settings.external_tools_ffmpeg_path = settings
		.external_tools_ffmpeg_path
		.as_deref()
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(ToString::to_string);
	settings.calibration_start_delay_seconds = settings.calibration_start_delay_seconds.min(30);
	settings.calibration_sample_count = settings.calibration_sample_count.clamp(1, 240);
	settings.calibration_sound_volume = settings.calibration_sound_volume.clamp(0.0, 1.0);
	settings.api_worker_threads = normalize_api_worker_threads(settings.api_worker_threads);
	settings.calibration_countdown_sound_path = settings
		.calibration_countdown_sound_path
		.as_deref()
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(ToString::to_string);
	settings.calibration_start_sound_path = settings
		.calibration_start_sound_path
		.as_deref()
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(ToString::to_string);
	settings.snapshot_save_dir = settings
		.snapshot_save_dir
		.as_deref()
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(ToString::to_string);
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
	if !(url.starts_with("https://") || url.starts_with("http://")) {
		return Err(format!("refused to open non-http(s) url: {url}"));
	}
	#[cfg(windows)]
	{
		command_without_console(Command::new("explorer.exe").arg(&url))
			.spawn()
			.map_err(|e| format!("open url: {e}"))?;
		return Ok(());
	}
	#[cfg(target_os = "macos")]
	{
		Command::new("open").arg(&url).spawn().map_err(|e| format!("open url: {e}"))?;
		return Ok(());
	}
	#[cfg(target_os = "linux")]
	{
		Command::new("xdg-open").arg(&url).spawn().map_err(|e| format!("open url: {e}"))?;
		return Ok(());
	}
	#[cfg_attr(any(windows, target_os = "macos", target_os = "linux"), allow(unreachable_code))]
	Err("unsupported platform".to_string())
}

const FFMPEG_HOME_URL: &str = "https://ffmpeg.org/";

#[tauri::command]
fn open_ffmpeg_home() -> Result<(), String> {
	open_external_url(FFMPEG_HOME_URL.to_string())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFileMetadata {
	pub width: Option<u32>,
	pub height: Option<u32>,
	pub fps: Option<f32>,
	pub fps_rounded: Option<u32>,
	pub source: String,
}

#[tauri::command]
fn probe_video_file_metadata(path: String, ffmpeg_path: Option<String>) -> Result<VideoFileMetadata, String> {
	let video_path = PathBuf::from(path.trim());
	if !video_path.is_file() {
		return Err(format!("video file not found: {}", video_path.display()));
	}
	let settings = load_app_settings();
	let ffmpeg = ffmpeg_path
		.as_deref()
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(PathBuf::from)
		.or_else(|| settings.external_tools_ffmpeg_path.as_deref().map(PathBuf::from));
	let ffprobe = resolve_ffprobe(ffmpeg.as_deref()).ok_or_else(|| {
		"ffprobe was not found. Set an ffmpeg.exe path in Settings; ffprobe.exe is normally in the same folder.".to_string()
	})?;
	probe_video_with_ffprobe(&ffprobe, &video_path)
}

fn resolve_ffprobe(ffmpeg_path: Option<&Path>) -> Option<PathBuf> {
	let exe = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
	if let Some(path) = ffmpeg_path {
		let name = path
			.file_name()
			.and_then(|name| name.to_str())
			.unwrap_or_default()
			.to_ascii_lowercase();
		if name == exe || (!cfg!(windows) && name == "ffprobe") {
			if path.is_file() {
				return Some(path.to_path_buf());
			}
		}
		if let Some(parent) = path.parent() {
			let sibling = parent.join(exe);
			if sibling.is_file() {
				return Some(sibling);
			}
		}
	}
	find_path_executable("ffprobe")
}

fn probe_video_with_ffprobe(ffprobe: &Path, video_path: &Path) -> Result<VideoFileMetadata, String> {
	let output = command_without_console(
		Command::new(ffprobe)
			.args([
				"-v",
				"error",
				"-select_streams",
				"v:0",
				"-show_entries",
				"stream=width,height,avg_frame_rate,r_frame_rate",
				"-of",
				"json",
			])
			.arg(video_path),
	)
	.output()
	.map_err(|e| format!("run ffprobe: {e}"))?;
	if !output.status.success() {
		return Err(format!(
			"ffprobe exited with {}: {}",
			output.status,
			String::from_utf8_lossy(&output.stderr).trim()
		));
	}
	let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("parse ffprobe json: {e}"))?;
	let stream = value
		.get("streams")
		.and_then(|streams| streams.as_array())
		.and_then(|streams| streams.first())
		.ok_or_else(|| "ffprobe returned no video stream".to_string())?;
	let width = stream
		.get("width")
		.and_then(|value| value.as_u64())
		.and_then(|value| u32::try_from(value).ok());
	let height = stream
		.get("height")
		.and_then(|value| value.as_u64())
		.and_then(|value| u32::try_from(value).ok());
	let fps = stream
		.get("avg_frame_rate")
		.and_then(|value| value.as_str())
		.and_then(parse_rate)
		.or_else(|| stream.get("r_frame_rate").and_then(|value| value.as_str()).and_then(parse_rate));
	Ok(VideoFileMetadata {
		width,
		height,
		fps,
		fps_rounded: fps.map(|value| value.round().clamp(1.0, 240.0) as u32),
		source: ffprobe.display().to_string(),
	})
}

fn parse_rate(value: &str) -> Option<f32> {
	let (num, den) = value.split_once('/')?;
	let num = num.parse::<f32>().ok()?;
	let den = den.parse::<f32>().ok()?;
	if num > 0.0 && den > 0.0 { Some(num / den) } else { None }
}

fn find_path_executable(name: &str) -> Option<PathBuf> {
	#[cfg(windows)]
	{
		let exe = if name.ends_with(".exe") {
			name.to_string()
		} else {
			format!("{name}.exe")
		};
		let output = command_without_console(Command::new("where.exe").arg(exe)).output().ok()?;
		if !output.status.success() {
			return None;
		}
		String::from_utf8_lossy(&output.stdout)
			.lines()
			.map(str::trim)
			.filter(|line| !line.is_empty())
			.map(PathBuf::from)
			.find(|path| path.is_file())
	}
	#[cfg(not(windows))]
	{
		let output = Command::new("which").arg(name).output().ok()?;
		if !output.status.success() {
			return None;
		}
		String::from_utf8_lossy(&output.stdout)
			.lines()
			.map(str::trim)
			.filter(|line| !line.is_empty())
			.map(PathBuf::from)
			.find(|path| path.is_file())
	}
}

#[tauri::command]
fn show_supervisor_console(app: tauri::AppHandle) -> Result<(), String> {
	show_main_window(&app);
	Ok(())
}

/// tray アイコン ID。`remove_tray_by_id` での個別解除ができるように一意 ID を割り当てる。
const TRAY_ICON_ID: &str = "un-motion-tray";

/// 終了時に保存した位置・サイズが Settings にあるなら復元して main window を構築する。
/// UN Avatar Supervisor の `setup_main_window` をそのまま UN Motion 用に翻訳したもの。
fn setup_main_window(app: &mut tauri::App) -> tauri::Result<WebviewWindow> {
	let app_settings = app
		.try_state::<Mutex<AppRuntimeSettings>>()
		.and_then(|s| s.lock().ok().map(|s| s.clone()))
		.unwrap_or_default();
	const MIN_LOGICAL_W: u32 = 820;
	const MIN_LOGICAL_H: u32 = 620;
	const MAX_LOGICAL_W: u32 = 3840;
	const MAX_LOGICAL_H: u32 = 2160;
	let width = app_settings
		.console_window_width
		.filter(|w| (MIN_LOGICAL_W..=MAX_LOGICAL_W).contains(w))
		.unwrap_or(1190);
	let height = app_settings
		.console_window_height
		.filter(|h| (MIN_LOGICAL_H..=MAX_LOGICAL_H).contains(h))
		.unwrap_or(620);
	let visible = !app_settings.start_minimized_to_tray || !app_settings.system_tray_enabled;
	let mut builder = WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
		.title(app_title_with_version())
		.icon(Image::from_bytes(include_bytes!(
			"../../../../assets/brand/un-motion-artwork-supervisor.png"
		))?)?
		.inner_size(f64::from(width), f64::from(height))
		.min_inner_size(f64::from(MIN_LOGICAL_W), f64::from(MIN_LOGICAL_H))
		.resizable(true)
		.visible(visible);
	if let (Some(x), Some(y)) = (app_settings.console_window_x, app_settings.console_window_y)
		&& (-16384..=16384).contains(&x)
		&& (-16384..=16384).contains(&y)
	{
		builder = builder.position(f64::from(x), f64::from(y));
	}
	builder.build()
}

fn attach_close_handler(window: WebviewWindow, app_handle: tauri::AppHandle) {
	window.on_window_event(move |event| match event {
		WindowEvent::CloseRequested { api, .. } => {
			persist_console_window_geometry(&app_handle);
			if should_hide_on_close(&app_handle) {
				api.prevent_close();
				if let Some(window) = app_handle.get_webview_window(MAIN_WINDOW_LABEL) {
					let _ = window.hide();
				}
				return;
			}
			if should_stop_capturers_on_exit(&app_handle)
				&& let Some(state) = app_handle.try_state::<Mutex<SupervisorState>>()
			{
				tracing::info!(target: "un_motion_supervisor", "CloseRequested: stop_all_capturers (stop_capturers_on_exit=true)");
				stop_all_in_state(&state);
			}
		}
		WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
			update_console_window_geometry_in_memory(&app_handle);
		}
		_ => {}
	});
}

fn update_console_window_geometry_in_memory(app_handle: &tauri::AppHandle) {
	let Some(window) = app_handle.get_webview_window(MAIN_WINDOW_LABEL) else {
		return;
	};
	let Some(state) = app_handle.try_state::<Mutex<AppRuntimeSettings>>() else {
		return;
	};
	let scale = window.scale_factor().unwrap_or(1.0).max(0.1);
	let Ok(mut state) = state.lock() else { return };
	if let Ok(pos) = window.outer_position() {
		let logical = pos.to_logical::<f64>(scale);
		state.console_window_x = Some(logical.x.round() as i32);
		state.console_window_y = Some(logical.y.round() as i32);
	}
	if let Ok(size) = window.inner_size() {
		let logical = size.to_logical::<f64>(scale);
		state.console_window_width = Some(logical.width.round().max(0.0) as u32);
		state.console_window_height = Some(logical.height.round().max(0.0) as u32);
	}
}

fn persist_console_window_geometry(app_handle: &tauri::AppHandle) {
	update_console_window_geometry_in_memory(app_handle);
	let Some(state) = app_handle.try_state::<Mutex<AppRuntimeSettings>>() else {
		return;
	};
	let Ok(state) = state.lock() else { return };
	if let Err(e) = write_app_settings(&state) {
		eprintln!("un-motion-supervisor: persist console window geometry failed: {e}");
	}
}

fn should_hide_on_close(app_handle: &tauri::AppHandle) -> bool {
	let settings = app_handle
		.try_state::<Mutex<AppRuntimeSettings>>()
		.and_then(|settings| settings.lock().ok().map(|settings| settings.clone()))
		.unwrap_or_default();
	if !settings.system_tray_enabled {
		return false;
	}
	settings.minimize_to_tray || (settings.close_to_tray_while_running && capturer_running(app_handle))
}

fn capturer_running(app_handle: &tauri::AppHandle) -> bool {
	app_handle
		.try_state::<Mutex<SupervisorState>>()
		.and_then(|state| {
			state.lock().ok().map(|state| {
				state
					.capturers
					.values()
					.any(|capturer| matches!(capturer.info.state, CapturerState::Starting | CapturerState::Running))
			})
		})
		.unwrap_or(false)
}

fn show_main_window(app: &tauri::AppHandle) {
	if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
		let _ = window.show();
		let _ = window.unminimize();
		let _ = window.set_focus();
	}
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
	if app.tray_by_id(TRAY_ICON_ID).is_some() {
		return Ok(());
	}
	let open = MenuItem::with_id(app, "open", t!("tray.open").to_string(), true, None::<&str>)?;
	let stop_all = MenuItem::with_id(app, "stop_all", t!("tray.stop_all").to_string(), true, None::<&str>)?;
	let quit = MenuItem::with_id(app, "quit", t!("tray.quit").to_string(), true, None::<&str>)?;
	let menu = Menu::new(app)?;
	menu.append(&open)?;
	menu.append(&stop_all)?;
	menu.append(&quit)?;
	let icon = Image::from_bytes(include_bytes!("../../../../assets/brand/un-motion-artwork-supervisor.png"))?;
	TrayIconBuilder::with_id(TRAY_ICON_ID)
		.tooltip(APP_TITLE)
		.icon(icon)
		.menu(&menu)
		.show_menu_on_left_click(false)
		.on_menu_event(handle_tray_menu_event)
		.on_tray_icon_event(|tray, event| {
			if let TrayIconEvent::DoubleClick {
				button: MouseButton::Left, ..
			} = event
			{
				show_main_window(tray.app_handle());
			}
		})
		.build(app)?;
	Ok(())
}

fn handle_tray_menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
	match event.id().as_ref() {
		"open" => show_main_window(app),
		"stop_all" => {
			if let Some(state) = app.try_state::<Mutex<SupervisorState>>() {
				stop_all_in_state(&state);
			}
		}
		"quit" => {
			if let Some(state) = app.try_state::<Mutex<SupervisorState>>() {
				stop_all_in_state(&state);
			}
			persist_console_window_geometry(app);
			app.exit(0);
		}
		_ => {}
	}
}

/// GUI から「即座に全停止」を呼ぶための Tauri command。
/// `App.svelte` 左メニュー / Settings タブ等から手動で呼べるようにしておく。
#[tauri::command]
fn stop_all_capturers(state: State<'_, Mutex<SupervisorState>>) -> Result<(), String> {
	stop_all_in_state(state.inner());
	Ok(())
}

/// Tauri アプリのエントリポイント。`main.rs` から呼ばれる。
pub fn run() {
	// tracing 出力を有効化 (cargo run / 端末から起動したとき stdout に流す)。
	// すでに別のレイヤが初期化済みなら try_init で握り潰す。
	let _ = tracing_subscriber::fmt()
		.with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
		.try_init();
	tauri::Builder::default()
		.plugin(tauri_plugin_notification::init())
		.manage(Mutex::new(SupervisorState::default()))
		.manage(Mutex::new(load_app_settings()))
		.invoke_handler(tauri::generate_handler![
			list_capturers,
			launch_capturer,
			stop_capturer,
			stop_all_capturers,
			capturer_runtime_status,
			calibrate_capturer_neutral,
			clear_capturer_neutral_calibration,
			build_capturer_face_pose_model,
			save_capturer_unmf_pose,
			save_all_capturers_unmf_pose,
			list_profiles,
			selected_profile_id,
			set_selected_profile_id,
			create_profile,
			delete_profile,
			duplicate_profile,
			reorder_profiles,
			get_profile_detail,
			update_profile_field,
			get_app_settings,
			set_stop_capturers_on_exit,
			sync_app_settings,
			open_external_url,
			open_ffmpeg_home,
			probe_video_file_metadata,
			save_supervisor_logs,
			pick_file_path,
			reveal_profiles_dir,
			reveal_supervisor_logs_dir,
			show_supervisor_console,
			enumerate_webcams,
			enumerate_webcam_formats,
			app_version,
			crate::i18n::i18n_get_svelte_bundle,
			crate::i18n::i18n_available_locales,
			crate::i18n::i18n_resolve_default_locale,
		])
		.setup(|app| {
			// AppRuntimeSettings.locale が未設定なら OS locale → サポート言語 → ja-JP の順で
			// 解決し、rust-i18n のグローバル locale を反映する。tray menu / native notification
			// が ja/en どちらで出るかはこの値で決定される。Svelte UI は別途 register(locale, ...)
			// から `i18n_get_svelte_bundle` を呼び出して同じ TOML 由来のバンドルを受け取る。
			if let Some(settings_mutex) = app.try_state::<Mutex<AppRuntimeSettings>>() {
				let resolved = {
					let mut settings = settings_mutex.lock().expect("AppRuntimeSettings mutex");
					if settings.locale.is_empty() || !crate::i18n::UN_I18N_STORE.has_locale(&settings.locale) {
						let resolved = crate::i18n::resolve_default_locale(&crate::i18n::UN_I18N_STORE);
						tracing::info!(locale = %resolved, "i18n: resolving locale from OS / fallback");
						settings.locale = resolved.clone();
						resolved
					} else {
						settings.locale.clone()
					}
				};
				crate::i18n::apply_locale(&resolved);
			}
			let window = setup_main_window(app)?;
			tracing::info!("un-motion-supervisor main window created");
			let app_handle = app.handle().clone();
			attach_close_handler(window, app_handle.clone());
			let tray_enabled = app
				.try_state::<Mutex<AppRuntimeSettings>>()
				.and_then(|s| s.lock().ok().map(|s| s.system_tray_enabled))
				.unwrap_or(false);
			if tray_enabled && let Err(e) = setup_tray(app.handle()) {
				eprintln!("un-motion-supervisor: tray setup failed: {e}");
			}
			Ok(())
		})
		.run(tauri::generate_context!())
		.expect("error while running UN Motion Supervisor");
}

// ---------- profile helpers ----------

/// GUI に返す Profile 詳細 DTO。UN Avatar `AvatarSetting` の縮約版で、Phase D
/// で GUI から編集する Field のみフラットに公開する。Phase E で
/// `pipeline_components` の各サブフィールド (input_path / input_fps / 等) や
/// `modifier.post_process_rules` の bool 群を加える予定。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDetail {
	pub id: String,
	pub name: String,
	pub note: String,
	pub path: String,
	pub icon_path: Option<String>,
	pub group: String,
	pub runtime: ProfileRuntimeView,
	pub pipeline: ProfilePipelineView,
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRuntimeView {
	pub fps: Option<u32>,
	pub vmc_enabled: Option<bool>,
	pub vmc_target_addr: Option<String>,
	pub zenoh_enabled: Option<bool>,
	pub zenoh_key_expr: Option<String>,
	pub zenoh_topic_mode: Option<String>,
	pub zenoh_stream_id: Option<String>,
	pub zenoh_producer: Option<String>,
	pub engine: Option<String>,
	pub device: Option<String>,
	pub resolution: Option<String>,
	pub media_pipe_running_mode: Option<String>,
	pub media_pipe_holistic_enabled: Option<bool>,
	pub media_pipe_delegate: Option<String>,
	pub media_pipe_num_threads: Option<u32>,
	pub media_pipe_holistic_flow_limiter_enabled: Option<bool>,
	pub media_pipe_holistic_flow_limiter_max_in_flight: Option<u32>,
	pub media_pipe_holistic_flow_limiter_max_in_queue: Option<u32>,
	pub vmc_receive_listen_addr: Option<String>,
	pub ifacialmocap_receive_listen_addr: Option<String>,
	pub modifier_head_enabled: Option<bool>,
	pub modifier_face_enabled: Option<bool>,
	pub modifier_hands_enabled: Option<bool>,
	pub modifier_arms_ik_enabled: Option<bool>,
	pub modifier_torso_enabled: Option<bool>,
	pub modifier_legs_enabled: Option<bool>,
	pub modifier_feet_enabled: Option<bool>,
	pub modifier_torso_pitch_scale: Option<f32>,
	pub modifier_neutral_calibration_enabled: Option<bool>,
	pub modifier_neutral_calibration_sample_count: Option<usize>,
	pub modifier_neutral_calibration_pose: Option<String>,
	pub modifier_mirror_mode: Option<String>,
	pub modifier_eye_open_bias: Option<f32>,
	/// Phase E-α-8: Modifier の Smoothing preset (off / low / medium / high /
	/// adaptive)。None は未設定 = `off` 扱い。TOML 上は `smoothingPreset`。
	pub modifier_smoothing_preset: Option<String>,
	pub modifier_smoothing_ema_enabled: Option<bool>,
	pub modifier_smoothing_ema_alpha: Option<f32>,
	pub modifier_smoothing_one_euro_enabled: Option<bool>,
	pub modifier_smoothing_confidence_adaptive_cutoff: Option<bool>,
	pub modifier_adaptive_min_cutoff_hz: Option<f32>,
	pub modifier_adaptive_beta: Option<f32>,
	pub modifier_adaptive_derivative_cutoff_hz: Option<f32>,
	pub modifier_face_pose_model_enabled: Option<bool>,
	pub modifier_face_pose_model_neutral_nose_drop_eye_mouth: Option<f32>,
	pub modifier_face_pose_model_sample_count: Option<u32>,
	pub modifier_anatomical_constraints: Option<bool>,
	pub modifier_hold_lost_landmarks: Option<bool>,
	pub modifier_ease_recovery: Option<bool>,
	pub modifier_limit_rotation_jumps: Option<bool>,
	pub modifier_head_source_switch_blend: Option<bool>,
	pub modifier_head_from_face_matrix: Option<bool>,
	pub modifier_lost_signal_behavior: Option<String>,
	pub modifier_lost_signal_rest_pose_blend: Option<f32>,
	pub modifier_lost_signal_hold_seconds: Option<f32>,
	pub modifier_lost_signal_head_behavior: Option<String>,
	pub modifier_lost_signal_head_rest_pose_blend: Option<f32>,
	pub modifier_lost_signal_head_hold_seconds: Option<f32>,
	pub modifier_lost_signal_hands_behavior: Option<String>,
	pub modifier_lost_signal_hands_rest_pose_blend: Option<f32>,
	pub modifier_lost_signal_hands_hold_seconds: Option<f32>,
	pub modifier_lost_signal_arms_behavior: Option<String>,
	pub modifier_lost_signal_arms_rest_pose_blend: Option<f32>,
	pub modifier_lost_signal_arms_hold_seconds: Option<f32>,
	pub modifier_lost_signal_recovery_seconds: Option<f32>,
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePipelineView {
	pub engine: Option<String>,
	pub input: Option<String>,
	pub post_process: Option<String>,
	pub input_path: Option<String>,
	pub input_fps: Option<u32>,
	pub input_width: Option<u32>,
	pub input_height: Option<u32>,
	pub input_pixel_format: Option<String>,
	pub input_repeat: Option<bool>,
	pub input_ffmpeg_path: Option<String>,
	pub input_denoise_mode: Option<String>,
	pub input_denoise_temporal_iir_hz: Option<f32>,
	pub input_resize_enabled: Option<bool>,
	pub input_resize_axis: Option<String>,
	pub input_resize_reference: Option<u32>,
	pub input_resize_width: Option<u32>,
	pub input_resize_height: Option<u32>,
	pub input_resize_preserve_aspect: Option<bool>,
	pub input_resize_pad_color: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileSummary {
	pub id: String,
	pub name: String,
	pub note: String,
	pub icon_path: Option<String>,
	pub group: String,
	pub engine: Option<String>,
	pub runtime_selection: Option<ProfileRuntimeSettings>,
	pub pipeline_components: Option<ProfilePipelineComponents>,
}

impl From<&CoreProfileDocumentProfile> for ProfileDetail {
	fn from(profile: &CoreProfileDocumentProfile) -> Self {
		let mut runtime = profile.runtime_selection.as_ref().map(ProfileRuntimeView::from).unwrap_or_default();
		if runtime.engine.as_deref().map(str::trim).unwrap_or_default().is_empty() {
			runtime.engine = profile_engine_summary(profile).or_else(|| Some("mediapipe-native".to_string()));
		}
		let pipeline = profile
			.pipeline_components
			.as_ref()
			.map(ProfilePipelineView::from)
			.unwrap_or_default();
		Self {
			id: profile.id.clone(),
			name: profile.name.clone(),
			note: profile.note.clone(),
			path: profile_path_for_id(&profile.id).display().to_string(),
			icon_path: profile.icon_path.clone(),
			group: profile.group.clone(),
			runtime,
			pipeline,
		}
	}
}

impl From<&ProfileRuntimeSettings> for ProfileRuntimeView {
	fn from(settings: &ProfileRuntimeSettings) -> Self {
		let modifier = settings.modifier.as_ref();
		let post_process_rules = modifier.and_then(|modifier| modifier.post_process_rules.as_ref());
		let face_pose_model = modifier.and_then(|modifier| modifier.face_pose_model.as_ref());
		Self {
			fps: settings.fps,
			vmc_enabled: settings.vmc_enabled,
			vmc_target_addr: settings.vmc_target_addr.clone(),
			zenoh_enabled: settings.zenoh_enabled,
			zenoh_key_expr: settings.zenoh_key_expr.clone(),
			zenoh_topic_mode: settings.zenoh_topic_mode.clone(),
			zenoh_stream_id: settings.zenoh_stream_id.clone(),
			zenoh_producer: settings.zenoh_producer.clone(),
			engine: settings.engine.clone(),
			device: settings.device.clone(),
			resolution: settings.resolution.clone(),
			media_pipe_running_mode: settings.media_pipe_running_mode.clone(),
			media_pipe_holistic_enabled: settings.media_pipe_holistic_enabled,
			media_pipe_delegate: settings.media_pipe_delegate.clone(),
			media_pipe_num_threads: settings.media_pipe_num_threads,
			media_pipe_holistic_flow_limiter_enabled: settings.media_pipe_holistic_flow_limiter_enabled,
			media_pipe_holistic_flow_limiter_max_in_flight: settings.media_pipe_holistic_flow_limiter_max_in_flight,
			media_pipe_holistic_flow_limiter_max_in_queue: settings.media_pipe_holistic_flow_limiter_max_in_queue,
			vmc_receive_listen_addr: settings.vmc_receive_listen_addr.clone(),
			ifacialmocap_receive_listen_addr: settings.ifacialmocap_receive_listen_addr.clone(),
			modifier_head_enabled: modifier.and_then(|modifier| modifier.head_enabled),
			modifier_face_enabled: modifier.and_then(|modifier| modifier.face_enabled),
			modifier_hands_enabled: modifier.and_then(|modifier| modifier.hands_enabled),
			modifier_arms_ik_enabled: modifier.and_then(|modifier| modifier.arms_ik_enabled),
			modifier_torso_enabled: modifier.and_then(|modifier| modifier.torso_enabled),
			modifier_legs_enabled: modifier.and_then(|modifier| modifier.legs_enabled),
			modifier_feet_enabled: modifier.and_then(|modifier| modifier.feet_enabled),
			modifier_torso_pitch_scale: modifier.and_then(|modifier| modifier.torso_pitch_scale),
			modifier_neutral_calibration_enabled: modifier.and_then(|modifier| modifier.neutral_calibration_enabled),
			modifier_neutral_calibration_sample_count: modifier
				.and_then(|modifier| modifier.neutral_calibration_rotations.as_ref())
				.map(|rotations| rotations.len()),
			modifier_neutral_calibration_pose: modifier.and_then(|modifier| modifier.neutral_calibration_pose.clone()),
			modifier_mirror_mode: modifier.and_then(|modifier| modifier.mirror_mode.clone()),
			modifier_eye_open_bias: modifier.and_then(|modifier| modifier.eye_open_bias),
			modifier_smoothing_preset: modifier.and_then(|modifier| modifier.smoothing_preset.clone()),
			modifier_smoothing_ema_enabled: modifier.and_then(|modifier| modifier.smoothing_ema_enabled),
			modifier_smoothing_ema_alpha: modifier.and_then(|modifier| modifier.smoothing_ema_alpha),
			modifier_smoothing_one_euro_enabled: modifier.and_then(|modifier| modifier.smoothing_one_euro_enabled),
			modifier_smoothing_confidence_adaptive_cutoff: modifier.and_then(|modifier| modifier.smoothing_confidence_adaptive_cutoff),
			modifier_adaptive_min_cutoff_hz: modifier.and_then(|modifier| modifier.adaptive_min_cutoff_hz),
			modifier_adaptive_beta: modifier.and_then(|modifier| modifier.adaptive_beta),
			modifier_adaptive_derivative_cutoff_hz: modifier.and_then(|modifier| modifier.adaptive_derivative_cutoff_hz),
			modifier_face_pose_model_enabled: face_pose_model.and_then(|model| model.enabled),
			modifier_face_pose_model_neutral_nose_drop_eye_mouth: face_pose_model.and_then(|model| model.neutral_nose_drop_eye_mouth),
			modifier_face_pose_model_sample_count: face_pose_model.and_then(|model| model.sample_count),
			modifier_anatomical_constraints: post_process_rules.and_then(|rules| rules.anatomical_constraints),
			modifier_hold_lost_landmarks: post_process_rules.and_then(|rules| rules.hold_lost_landmarks),
			modifier_ease_recovery: post_process_rules.and_then(|rules| rules.ease_recovery),
			modifier_limit_rotation_jumps: post_process_rules.and_then(|rules| rules.limit_rotation_jumps),
			modifier_head_source_switch_blend: post_process_rules.and_then(|rules| rules.head_source_switch_blend),
			modifier_head_from_face_matrix: post_process_rules.and_then(|rules| rules.head_from_face_matrix),
			modifier_lost_signal_behavior: post_process_rules.and_then(|rules| rules.lost_signal_behavior.clone()),
			modifier_lost_signal_rest_pose_blend: post_process_rules.and_then(|rules| rules.lost_signal_rest_pose_blend),
			modifier_lost_signal_hold_seconds: post_process_rules.and_then(|rules| rules.lost_signal_hold_seconds),
			modifier_lost_signal_head_behavior: post_process_rules.and_then(|rules| rules.lost_signal_head_behavior.clone()),
			modifier_lost_signal_head_rest_pose_blend: post_process_rules.and_then(|rules| rules.lost_signal_head_rest_pose_blend),
			modifier_lost_signal_head_hold_seconds: post_process_rules.and_then(|rules| rules.lost_signal_head_hold_seconds),
			modifier_lost_signal_hands_behavior: post_process_rules.and_then(|rules| rules.lost_signal_hands_behavior.clone()),
			modifier_lost_signal_hands_rest_pose_blend: post_process_rules.and_then(|rules| rules.lost_signal_hands_rest_pose_blend),
			modifier_lost_signal_hands_hold_seconds: post_process_rules.and_then(|rules| rules.lost_signal_hands_hold_seconds),
			modifier_lost_signal_arms_behavior: post_process_rules.and_then(|rules| rules.lost_signal_arms_behavior.clone()),
			modifier_lost_signal_arms_rest_pose_blend: post_process_rules.and_then(|rules| rules.lost_signal_arms_rest_pose_blend),
			modifier_lost_signal_arms_hold_seconds: post_process_rules.and_then(|rules| rules.lost_signal_arms_hold_seconds),
			modifier_lost_signal_recovery_seconds: post_process_rules.and_then(|rules| rules.lost_signal_recovery_seconds),
		}
	}
}

impl From<&ProfilePipelineComponents> for ProfilePipelineView {
	fn from(components: &ProfilePipelineComponents) -> Self {
		let input_denoise_mode = profile_view_input_denoise_mode(components.input_denoise_mode.as_deref());
		let input_denoise_temporal_iir_hz = components
			.input_denoise_temporal_iir_hz
			.or_else(|| profile_view_input_denoise_legacy_hz(components.input_denoise_mode.as_deref()));
		Self {
			engine: components.engine.clone(),
			input: components.input.clone(),
			post_process: components.post_process.clone(),
			input_path: components.input_path.clone(),
			input_fps: components.input_fps,
			input_width: components.input_width,
			input_height: components.input_height,
			input_pixel_format: components.input_pixel_format.clone(),
			input_repeat: components.input_repeat,
			input_ffmpeg_path: components.input_ffmpeg_path.clone(),
			input_denoise_mode,
			input_denoise_temporal_iir_hz,
			input_resize_enabled: components.input_resize_enabled,
			input_resize_axis: components.input_resize_axis.clone(),
			input_resize_reference: components.input_resize_reference,
			input_resize_width: components.input_resize_width,
			input_resize_height: components.input_resize_height,
			input_resize_preserve_aspect: components.input_resize_preserve_aspect,
			input_resize_pad_color: components.input_resize_pad_color.clone(),
		}
	}
}

fn profile_view_input_denoise_mode(value: Option<&str>) -> Option<String> {
	match value.unwrap_or("off").trim().to_ascii_lowercase().as_str() {
		"on" | "true" => Some("temporal-iir".to_string()),
		"temporal-iir" | "temporal_iir" | "temporal-iir-10hz" | "temporal_iir_10hz" | "temporal-iir-8hz" | "temporal_iir_8hz"
		| "temporal-iir-6hz" | "temporal_iir_6hz" | "temporal-iir-4hz" | "temporal_iir_4hz" | "temporal-iir-2hz" | "temporal_iir_2hz" => {
			Some("temporal-iir".to_string())
		}
		"off" | "" => Some("off".to_string()),
		_ => Some("off".to_string()),
	}
}

fn profile_view_input_denoise_legacy_hz(value: Option<&str>) -> Option<f32> {
	match value.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
		"temporal-iir-8hz" | "temporal_iir_8hz" => Some(8.0),
		"temporal-iir-6hz" | "temporal_iir_6hz" => Some(6.0),
		"temporal-iir-4hz" | "temporal_iir_4hz" => Some(4.0),
		"temporal-iir-2hz" | "temporal_iir_2hz" => Some(2.0),
		"temporal-iir-10hz" | "temporal_iir_10hz" => Some(10.0),
		_ => None,
	}
}

/// Profile 表示名から ASCII slug を作って一意な id を生成する。空 / 衝突時は
/// 連番サフィックスを付与。
fn next_profile_id(doc: &CoreProfileDocument, name: &str) -> String {
	let slug: String = name
		.chars()
		.flat_map(|ch| {
			if ch.is_ascii_alphanumeric() {
				vec![ch.to_ascii_lowercase()]
			} else if ch == ' ' || ch == '-' || ch == '_' {
				vec!['-']
			} else {
				vec![]
			}
		})
		.collect();
	let trimmed = slug.trim_matches('-').to_string();
	let base = if trimmed.is_empty() { "profile".to_string() } else { trimmed };
	let mut candidate = base.clone();
	let mut index = 2_u32;
	while doc.profiles.iter().any(|profile| profile.id == candidate) {
		candidate = format!("{base}-{index}");
		index += 1;
	}
	candidate
}

fn next_profile_name(doc: &CoreProfileDocument, base: &str) -> String {
	let trimmed = base.trim();
	let base = if trimmed.is_empty() {
		"Profile".to_string()
	} else {
		trimmed.to_string()
	};
	let mut candidate = base.clone();
	let mut index = 2_u32;
	while doc.profiles.iter().any(|profile| profile.name == candidate) {
		candidate = format!("{base} {index}");
		index += 1;
	}
	candidate
}

/// Profile の個別フィールドを更新する。受け取った `value` が `null` の場合は
/// `Option::None` を入れて runtime 既定値に戻す合図とする。
fn apply_profile_field(profile: &mut CoreProfileDocumentProfile, field: &str, value: serde_json::Value) -> Result<(), String> {
	match field {
		"name" => {
			profile.name = expect_string(&value, field)?.trim().to_string();
		}
		"note" => {
			profile.note = expect_string(&value, field)?;
		}
		"icon_path" => {
			profile.icon_path = opt_string(&value, field)?;
		}
		"group" => {
			profile.group = expect_string(&value, field)?.trim().to_string();
		}
		path if path.starts_with("runtime_selection.") => {
			let runtime = profile.runtime_selection.get_or_insert_with(ProfileRuntimeSettings::default);
			apply_runtime_field(runtime, &path["runtime_selection.".len()..], value)?;
			if runtime_is_empty(runtime) {
				profile.runtime_selection = None;
			}
		}
		path if path.starts_with("pipeline_components.") => {
			let pipeline = profile.pipeline_components.get_or_insert_with(ProfilePipelineComponents::default);
			apply_pipeline_field(pipeline, &path["pipeline_components.".len()..], value)?;
			if pipeline_is_empty(pipeline) {
				profile.pipeline_components = None;
			}
		}
		_ => return Err(format!("unknown profile field: {field}")),
	}
	Ok(())
}

fn apply_runtime_field(runtime: &mut ProfileRuntimeSettings, field: &str, value: serde_json::Value) -> Result<(), String> {
	match field {
		"fps" => runtime.fps = opt_u32(&value, field)?,
		"vmc_enabled" => runtime.vmc_enabled = opt_bool(&value, field)?,
		"vmc_target_addr" => runtime.vmc_target_addr = opt_string(&value, field)?,
		"zenoh_enabled" => runtime.zenoh_enabled = opt_bool(&value, field)?,
		"zenoh_key_expr" => runtime.zenoh_key_expr = opt_string(&value, field)?,
		"zenoh_topic_mode" => runtime.zenoh_topic_mode = opt_string(&value, field)?,
		"zenoh_stream_id" => runtime.zenoh_stream_id = opt_string(&value, field)?,
		"zenoh_producer" => runtime.zenoh_producer = opt_string(&value, field)?,
		"engine" => runtime.engine = opt_string(&value, field)?,
		"device" => runtime.device = opt_string(&value, field)?,
		"resolution" => runtime.resolution = opt_string(&value, field)?,
		"media_pipe_running_mode" => runtime.media_pipe_running_mode = opt_string(&value, field)?,
		"media_pipe_holistic_enabled" => runtime.media_pipe_holistic_enabled = opt_bool(&value, field)?,
		"media_pipe_delegate" => runtime.media_pipe_delegate = opt_string(&value, field)?,
		"media_pipe_num_threads" => runtime.media_pipe_num_threads = opt_u32(&value, field)?.map(|value| value.max(1)),
		"media_pipe_holistic_flow_limiter_enabled" => runtime.media_pipe_holistic_flow_limiter_enabled = opt_bool(&value, field)?,
		"media_pipe_holistic_flow_limiter_max_in_flight" => {
			runtime.media_pipe_holistic_flow_limiter_max_in_flight = opt_u32(&value, field)?.map(|value| value.max(1))
		}
		"media_pipe_holistic_flow_limiter_max_in_queue" => runtime.media_pipe_holistic_flow_limiter_max_in_queue = opt_u32(&value, field)?,
		"vmc_receive_listen_addr" => runtime.vmc_receive_listen_addr = opt_string(&value, field)?,
		"ifacialmocap_receive_listen_addr" => runtime.ifacialmocap_receive_listen_addr = opt_string(&value, field)?,
		path if path.starts_with("modifier.") => {
			let modifier = runtime.modifier.get_or_insert_with(ProfileModifierSettings::default);
			apply_modifier_field(modifier, &path["modifier.".len()..], value)?;
			if modifier_is_empty(modifier) {
				runtime.modifier = None;
			}
		}
		_ => return Err(format!("unknown runtime_selection field: {field}")),
	}
	Ok(())
}

fn apply_modifier_field(modifier: &mut ProfileModifierSettings, field: &str, value: serde_json::Value) -> Result<(), String> {
	match field {
		"head_enabled" => modifier.head_enabled = opt_bool(&value, field)?,
		"face_enabled" => modifier.face_enabled = opt_bool(&value, field)?,
		"hands_enabled" => modifier.hands_enabled = opt_bool(&value, field)?,
		"arms_ik_enabled" => modifier.arms_ik_enabled = opt_bool(&value, field)?,
		"torso_enabled" => modifier.torso_enabled = opt_bool(&value, field)?,
		"legs_enabled" => modifier.legs_enabled = opt_bool(&value, field)?,
		"feet_enabled" => modifier.feet_enabled = opt_bool(&value, field)?,
		"torso_pitch_scale" => modifier.torso_pitch_scale = opt_f32(&value, field)?.map(|value| value.clamp(0.0, 1.0)),
		"mirror_mode" => modifier.mirror_mode = opt_string(&value, field)?,
		"eye_open_bias" => modifier.eye_open_bias = opt_f32(&value, field)?.map(|value| value.clamp(0.0, 1.0)),
		// Phase E-α-8: Modifier の Smoothing preset ("off"/"low"/"medium"/"high"/"adaptive").
		// 空文字列 (`""`) は GUI dropdown の「(default: off)」選択肢から来るので
		// `opt_string` が `None` に正規化する → modifier から削除される (default 復帰)。
		"smoothing_preset" => modifier.smoothing_preset = opt_string(&value, field)?,
		"smoothing_ema_enabled" => modifier.smoothing_ema_enabled = opt_bool(&value, field)?,
		"smoothing_ema_alpha" => modifier.smoothing_ema_alpha = opt_f32(&value, field)?,
		"smoothing_one_euro_enabled" => modifier.smoothing_one_euro_enabled = opt_bool(&value, field)?,
		"smoothing_confidence_adaptive_cutoff" => modifier.smoothing_confidence_adaptive_cutoff = opt_bool(&value, field)?,
		"adaptive_min_cutoff_hz" => modifier.adaptive_min_cutoff_hz = opt_f32(&value, field)?,
		"adaptive_beta" => modifier.adaptive_beta = opt_f32(&value, field)?,
		"adaptive_derivative_cutoff_hz" => modifier.adaptive_derivative_cutoff_hz = opt_f32(&value, field)?,
		path if path.starts_with("face_pose_model.") => {
			if path == "face_pose_model.enabled" && value.as_bool() == Some(false) {
				modifier.face_pose_model = None;
				return Ok(());
			}
			let model = modifier.face_pose_model.get_or_insert_with(Default::default);
			apply_face_pose_model_field(model, &path["face_pose_model.".len()..], value)?;
			if *model == Default::default() {
				modifier.face_pose_model = None;
			}
		}
		path if path.starts_with("post_process_rules.") => {
			let rules = modifier.post_process_rules.get_or_insert_with(Default::default);
			apply_post_process_rules_field(rules, &path["post_process_rules.".len()..], value)?;
			if *rules == Default::default() {
				modifier.post_process_rules = None;
			}
		}
		_ => return Err(format!("unknown modifier field: {field}")),
	}
	Ok(())
}

fn apply_face_pose_model_field(
	model: &mut un_motion_profile_schema::profile_settings::ProfileFacePoseModelSettings,
	field: &str,
	value: serde_json::Value,
) -> Result<(), String> {
	match field {
		"enabled" => model.enabled = opt_bool(&value, field)?,
		"neutral_nose_drop_eye_mouth" => model.neutral_nose_drop_eye_mouth = opt_f32(&value, field)?,
		"sample_count" => model.sample_count = opt_u32(&value, field)?,
		"median_abs_yaw" => model.median_abs_yaw = opt_f32(&value, field)?,
		"median_abs_roll" => model.median_abs_roll = opt_f32(&value, field)?,
		_ => return Err(format!("unknown face_pose_model field: {field}")),
	}
	Ok(())
}

fn apply_post_process_rules_field(
	rules: &mut un_motion_profile_schema::ProfileMediaPipeAdvancedSettings,
	field: &str,
	value: serde_json::Value,
) -> Result<(), String> {
	match field {
		"anatomical_constraints" => rules.anatomical_constraints = opt_bool(&value, field)?,
		"hold_lost_landmarks" => rules.hold_lost_landmarks = opt_bool(&value, field)?,
		"ease_recovery" => rules.ease_recovery = opt_bool(&value, field)?,
		"limit_rotation_jumps" => rules.limit_rotation_jumps = opt_bool(&value, field)?,
		"head_source_switch_blend" => rules.head_source_switch_blend = opt_bool(&value, field)?,
		"head_from_face_matrix" => rules.head_from_face_matrix = opt_bool(&value, field)?,
		"lost_signal_behavior" => rules.lost_signal_behavior = opt_string(&value, field)?,
		"lost_signal_rest_pose_blend" => rules.lost_signal_rest_pose_blend = opt_f32(&value, field)?,
		"lost_signal_hold_seconds" => rules.lost_signal_hold_seconds = opt_f32(&value, field)?,
		"lost_signal_head_behavior" => rules.lost_signal_head_behavior = opt_string(&value, field)?,
		"lost_signal_head_rest_pose_blend" => rules.lost_signal_head_rest_pose_blend = opt_f32(&value, field)?,
		"lost_signal_head_hold_seconds" => rules.lost_signal_head_hold_seconds = opt_f32(&value, field)?,
		"lost_signal_hands_behavior" => rules.lost_signal_hands_behavior = opt_string(&value, field)?,
		"lost_signal_hands_rest_pose_blend" => rules.lost_signal_hands_rest_pose_blend = opt_f32(&value, field)?,
		"lost_signal_hands_hold_seconds" => rules.lost_signal_hands_hold_seconds = opt_f32(&value, field)?,
		"lost_signal_arms_behavior" => rules.lost_signal_arms_behavior = opt_string(&value, field)?,
		"lost_signal_arms_rest_pose_blend" => rules.lost_signal_arms_rest_pose_blend = opt_f32(&value, field)?,
		"lost_signal_arms_hold_seconds" => rules.lost_signal_arms_hold_seconds = opt_f32(&value, field)?,
		"lost_signal_recovery_seconds" => rules.lost_signal_recovery_seconds = opt_f32(&value, field)?,
		_ => return Err(format!("unknown post_process_rules field: {field}")),
	}
	Ok(())
}

fn apply_pipeline_field(pipeline: &mut ProfilePipelineComponents, field: &str, value: serde_json::Value) -> Result<(), String> {
	match field {
		"input" => pipeline.input = opt_string(&value, field)?,
		"post_process" => pipeline.post_process = opt_string(&value, field)?,
		"input_path" => pipeline.input_path = opt_string(&value, field)?,
		"input_fps" => pipeline.input_fps = opt_u32(&value, field)?,
		"input_width" => pipeline.input_width = opt_u32(&value, field)?,
		"input_height" => pipeline.input_height = opt_u32(&value, field)?,
		"input_pixel_format" => pipeline.input_pixel_format = opt_string(&value, field)?,
		"input_repeat" => pipeline.input_repeat = opt_bool(&value, field)?,
		"input_ffmpeg_path" => pipeline.input_ffmpeg_path = opt_string(&value, field)?,
		"input_denoise_mode" => pipeline.input_denoise_mode = opt_string(&value, field)?,
		"input_denoise_temporal_iir_hz" => pipeline.input_denoise_temporal_iir_hz = opt_f32(&value, field)?,
		"input_resize_enabled" => pipeline.input_resize_enabled = opt_bool(&value, field)?,
		"input_resize_axis" => pipeline.input_resize_axis = opt_string(&value, field)?,
		"input_resize_reference" => pipeline.input_resize_reference = opt_u32(&value, field)?,
		"input_resize_width" => pipeline.input_resize_width = opt_u32(&value, field)?,
		"input_resize_height" => pipeline.input_resize_height = opt_u32(&value, field)?,
		"input_resize_preserve_aspect" => pipeline.input_resize_preserve_aspect = opt_bool(&value, field)?,
		"input_resize_pad_color" => pipeline.input_resize_pad_color = opt_string(&value, field)?,
		_ => return Err(format!("unknown pipeline_components field: {field}")),
	}
	Ok(())
}

fn runtime_is_empty(runtime: &ProfileRuntimeSettings) -> bool {
	*runtime == ProfileRuntimeSettings::default()
}

fn pipeline_is_empty(pipeline: &ProfilePipelineComponents) -> bool {
	*pipeline == ProfilePipelineComponents::default()
}

fn modifier_is_empty(modifier: &ProfileModifierSettings) -> bool {
	*modifier == ProfileModifierSettings::default()
}

fn expect_string(value: &serde_json::Value, field: &str) -> Result<String, String> {
	value
		.as_str()
		.map(|s| s.to_string())
		.ok_or_else(|| format!("field `{field}` must be a string"))
}

fn opt_string(value: &serde_json::Value, field: &str) -> Result<Option<String>, String> {
	if value.is_null() {
		Ok(None)
	} else if let Some(text) = value.as_str() {
		let trimmed = text.trim();
		Ok(if trimmed.is_empty() { None } else { Some(trimmed.to_string()) })
	} else {
		Err(format!("field `{field}` must be a string or null"))
	}
}

fn opt_bool(value: &serde_json::Value, field: &str) -> Result<Option<bool>, String> {
	if value.is_null() {
		Ok(None)
	} else {
		value
			.as_bool()
			.map(Some)
			.ok_or_else(|| format!("field `{field}` must be a bool or null"))
	}
}

fn opt_u32(value: &serde_json::Value, field: &str) -> Result<Option<u32>, String> {
	if value.is_null() {
		return Ok(None);
	}
	if let Some(n) = value.as_u64() {
		if n > u32::MAX as u64 {
			return Err(format!("field `{field}` exceeds u32 range"));
		}
		return Ok(Some(n as u32));
	}
	Err(format!("field `{field}` must be a non-negative integer or null"))
}

fn opt_f32(value: &serde_json::Value, field: &str) -> Result<Option<f32>, String> {
	if value.is_null() {
		return Ok(None);
	}
	if let Some(n) = value.as_f64() {
		if n.is_finite() && n >= 0.0 && n <= f32::MAX as f64 {
			return Ok(Some(n as f32));
		}
	}
	Err(format!("field `{field}` must be a non-negative number or null"))
}

/// 保存済み document を対象 profile で稼働中の Capturer にライブ同期する。
///
/// `/api/profiles/document/sync` は Capturer の現在 active profile を保持したまま
/// profile 内容だけ差し替える。Supervisor は Capturer 群を統合管理しないため、
/// 編集された profile を実行しているプロセスだけに同期する。
///
/// 失敗は警告のみで握りつぶす (再起動すれば user profile store から復元できるため致命的でない)。
fn push_document_to_matching_capturers(state: &SupervisorState, doc: &CoreProfileDocument, profile_id: &str) {
	let body = serde_json::json!({ "selection": doc });
	for capturer in state.capturers.values() {
		if !matches!(capturer.info.state, CapturerState::Running) {
			continue;
		}
		if capturer.info.profile_id.as_deref() != Some(profile_id) {
			continue;
		}
		let url = format!("http://{}/api/profiles/document/sync", capturer.bind_addr);
		if let Err(error) = state.http_client.put(&url).json(&body).send() {
			tracing::warn!(id = capturer.info.id, %error, "sync profile document via capturer failed");
		}
	}
}

/// 対象 Capturer から HTTP 経由で active_profile_id を取得する。
fn fetch_capturer_active_profile_id(http_client: &reqwest::blocking::Client, bind_addr: SocketAddr) -> Option<String> {
	let url = format!("http://{bind_addr}/api/profiles/active");
	let response = http_client.get(&url).send().ok()?;
	if !response.status().is_success() {
		return None;
	}
	#[derive(serde::Deserialize)]
	#[serde(rename_all = "camelCase")]
	struct StatusEnvelope {
		status: StatusPayload,
	}
	#[derive(serde::Deserialize)]
	#[serde(rename_all = "camelCase")]
	struct StatusPayload {
		active_profile_id: String,
	}
	let envelope: StatusEnvelope = response.json().ok()?;
	Some(envelope.status.active_profile_id)
}

/// `CoreProfileDocument` の `profiles: Vec<CoreProfileDocumentProfile>` から
/// GUI 用の一覧へ縮約する。Capturers 一覧では source 表示や F モデル警告に
/// profile の runtime / pipeline summary が必要なので、設定本体も浅く渡す。
trait CoreProfileDocumentExt {
	fn profiles_summary(&self) -> Vec<ProfileSummary>;
}

impl CoreProfileDocumentExt for un_motion_profile_schema::CoreProfileDocument {
	fn profiles_summary(&self) -> Vec<ProfileSummary> {
		self.profiles
			.iter()
			.map(|profile| ProfileSummary {
				id: profile.id.clone(),
				name: profile.name.clone(),
				note: profile.note.clone(),
				icon_path: profile.icon_path.clone(),
				group: profile.group.clone(),
				engine: profile_engine_summary(profile),
				runtime_selection: profile.runtime_selection.clone(),
				pipeline_components: profile.pipeline_components.clone(),
			})
			.collect()
	}
}

// ---------- internal helpers (UN Avatar Supervisor からの翻訳) ----------

/// Capturer プロセスの try_wait / stderr buffer / uptime を更新する。
/// `list_capturers` / `capturer_runtime_status` の前に必ず呼ぶことで、GUI が
/// 古い状態を見ないようにする。
fn refresh_capturers(state: &mut SupervisorState) {
	for capturer in state.capturers.values_mut() {
		if matches!(capturer.info.state, CapturerState::Exited | CapturerState::Crashed) {
			refresh_capturer_stderr(capturer);
			continue;
		}
		let exit_status = match capturer.child.try_wait() {
			Ok(status) => status,
			Err(error) => {
				tracing::warn!(id = capturer.info.id, %error, "try_wait failed");
				None
			}
		};
		if let Some(status) = exit_status {
			capturer.info.state = if matches!(capturer.info.state, CapturerState::Stopping) {
				CapturerState::Exited
			} else if status.success() {
				CapturerState::Exited
			} else {
				CapturerState::Crashed
			};
			capturer.info.pid = None;
			capturer.info.exit_code = status.code();
		} else if matches!(capturer.info.state, CapturerState::Starting) {
			capturer.info.state = CapturerState::Running;
		}
		capturer.info.uptime_secs = capturer.started_at.elapsed().as_secs();
		if let Ok(tail) = capturer.stderr_tail.lock() {
			capturer.info.stderr_tail = tail.clone();
			capturer.info.last_stderr = tail.last().cloned();
		}
	}
	prune_stopped_capturer_history(state);
}

fn prune_stopped_capturer_history(state: &mut SupervisorState) {
	let stopped: Vec<u32> = state
		.capturers
		.iter()
		.filter_map(|(id, capturer)| matches!(capturer.info.state, CapturerState::Exited | CapturerState::Crashed).then_some(*id))
		.collect();
	let overflow = stopped.len().saturating_sub(MAX_STOPPED_CAPTURER_HISTORY);
	for id in stopped.into_iter().take(overflow) {
		state.capturers.remove(&id);
	}
}

/// Windows では `Child::kill()` が **直接子プロセスだけ** を終了させ、Capturer が
/// 起動したカメラ／MediaPipe／トレイ用の子や、`cargo run` 経由時の孫プロセスが
/// 残留することがある。`taskkill /T` で **プロセス木ごと** 止める。
#[cfg(windows)]
fn force_stop_capturer_child(child: &mut Child, context: &str) -> Result<(), String> {
	let pid = child.id();
	let status = Command::new("taskkill")
		.args(["/PID", &pid.to_string(), "/T", "/F"])
		.creation_flags(CREATE_NO_WINDOW)
		.status()
		.map_err(|e| format!("{context}: taskkill failed to spawn: {e}"))?;
	// 既に死んでいると 128 などで失敗することがあるが、`wait` で実状態を確定する。
	if !status.success() {
		tracing::debug!(target: "un_motion_supervisor", pid, code = ?status.code(), "{context}: taskkill exited non-zero (process may already be gone)");
	}
	child.wait().map_err(|e| format!("{context}: wait after taskkill: {e}"))?;
	Ok(())
}

#[cfg(not(windows))]
fn force_stop_capturer_child(child: &mut Child, context: &str) -> Result<(), String> {
	child.kill().map_err(|e| format!("{context}: kill: {e}"))?;
	child.wait().map_err(|e| format!("{context}: wait: {e}"))?;
	Ok(())
}

fn wait_capturer_exit(child: &mut Child, timeout: Duration) -> Result<bool, String> {
	let started = Instant::now();
	loop {
		match child.try_wait() {
			Ok(Some(_)) => return Ok(true),
			Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
			Ok(None) => return Ok(false),
			Err(e) => return Err(format!("wait capturer exit: {e}")),
		}
	}
}

/// UN Avatar の `stop_managed_renderer` の HTTP 版。`POST /api/core/exit` で
/// graceful 終了を試み、`wait_capturer_exit` で待つ。失敗時は `force_stop_capturer_child`
/// でプロセス木ごと強制終了する。
fn stop_managed_capturer(id: u32, capturer: &mut ManagedCapturer, http_client: &reqwest::blocking::Client) -> Result<(), String> {
	capturer.info.state = CapturerState::Stopping;
	let url = format!("http://{}/api/core/exit", capturer.bind_addr);
	let graceful_requested = http_client.post(&url).send().ok().is_some_and(|resp| resp.status().is_success());
	if graceful_requested && wait_capturer_exit(&mut capturer.child, Duration::from_millis(1500))? {
		capturer.info.state = CapturerState::Exited;
		capturer.info.pid = None;
		refresh_capturer_stderr(capturer);
		return Ok(());
	}
	force_stop_capturer_child(&mut capturer.child, &format!("stop capturer {id}"))?;
	capturer.info.state = CapturerState::Exited;
	capturer.info.pid = None;
	refresh_capturer_stderr(capturer);
	Ok(())
}

fn refresh_capturer_stderr(capturer: &mut ManagedCapturer) {
	let Ok(tail) = capturer.stderr_tail.lock() else {
		return;
	};
	capturer.info.stderr_tail = tail.clone();
	capturer.info.last_stderr = tail.last().cloned();
}

/// `runtime_snapshot.output_telemetry` JSON から `TelemetrySample` を切り出す。
/// 旧 Capturer の snapshot で `output_telemetry` 自体が無い / `sources` が無い場合は
/// 該当フィールドが 0 / 空のままになる (serde で `default` 扱い)。
fn extract_telemetry_sample(snapshot: &serde_json::Value) -> Option<TelemetrySample> {
	let telemetry = snapshot.get("output_telemetry")?;
	let mut sample = TelemetrySample {
		sampled_at: Instant::now(),
		vmc_datagrams: 0,
		vmc_packets: 0,
		zenoh_frames: 0,
		sources: std::collections::BTreeMap::new(),
	};
	if let Some(vmc) = telemetry.get("vmc").and_then(|v| v.as_object()) {
		sample.vmc_datagrams = vmc.get("sent_datagrams").and_then(|v| v.as_u64()).unwrap_or(0);
		sample.vmc_packets = vmc.get("sent_packets").and_then(|v| v.as_u64()).unwrap_or(0);
	}
	if let Some(zenoh) = telemetry.get("zenoh").and_then(|v| v.as_object()) {
		sample.zenoh_frames = zenoh.get("sent_frames").and_then(|v| v.as_u64()).unwrap_or(0);
	}
	if let Some(sources) = telemetry.get("sources").and_then(|v| v.as_array()) {
		for source in sources {
			let Some(stream_id) = source.get("stream_id").and_then(|v| v.as_str()) else {
				continue;
			};
			let entry = SourceSampleEntry {
				kind: source.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string(),
				source_id: source.get("source_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
				raw_received: source.get("raw_received").and_then(|v| v.as_u64()).unwrap_or(0),
				frames_emitted: source.get("frames_emitted").and_then(|v| v.as_u64()).unwrap_or(0),
				observed_source_fps_milli: source.get("observed_source_fps_milli").and_then(|v| v.as_u64()).unwrap_or(0),
			};
			sample.sources.insert(stream_id.to_string(), entry);
		}
	}
	Some(sample)
}

/// 直前サンプル `prev` と現サンプル `cur` の差分から FPS を計算する。Δt が小さすぎる
/// (< 50 ms) 場合は計算誤差が大きいので `None` を返す: 次の poll で再計算する。
fn compute_fps_from_samples(prev: &TelemetrySample, cur: &TelemetrySample) -> Option<CapturerOutputFps> {
	let delta = cur.sampled_at.saturating_duration_since(prev.sampled_at);
	let dt = delta.as_secs_f64();
	if dt < 0.05 {
		return None;
	}
	let per_sec = |before: u64, after: u64| -> f32 { ((after.saturating_sub(before)) as f64 / dt) as f32 };
	let mut sources = Vec::new();
	for (stream_id, entry) in &cur.sources {
		let prev_entry = prev.sources.get(stream_id);
		let raw_per_sec = match prev_entry {
			Some(p) => per_sec(p.raw_received, entry.raw_received),
			None => 0.0,
		};
		let frames_per_sec = match prev_entry {
			Some(p) => per_sec(p.frames_emitted, entry.frames_emitted),
			None => 0.0,
		};
		sources.push(CapturerSourceFps {
			kind: entry.kind.clone(),
			stream_id: stream_id.clone(),
			source_id: entry.source_id.clone(),
			raw_per_sec,
			frames_per_sec,
			observed_source_fps: (entry.observed_source_fps_milli > 0).then_some(entry.observed_source_fps_milli as f32 / 1000.0),
		});
	}
	Some(CapturerOutputFps {
		interval_secs: dt as f32,
		vmc_datagrams_per_sec: per_sec(prev.vmc_datagrams, cur.vmc_datagrams),
		vmc_packets_per_sec: per_sec(prev.vmc_packets, cur.vmc_packets),
		zenoh_frames_per_sec: per_sec(prev.zenoh_frames, cur.zenoh_frames),
		sources,
	})
}

/// `/api/runtime/snapshot` から CoreSnapshot を取得。Svelte 側で柔軟にレンダリングする
/// ため `serde_json::Value` のまま返す。
fn fetch_runtime_snapshot(http_client: &reqwest::blocking::Client, bind_addr: SocketAddr) -> Result<serde_json::Value, String> {
	let url = format!("http://{}/api/runtime/snapshot", bind_addr);
	let response = http_client.get(&url).send().map_err(|e| format!("snapshot request failed: {e}"))?;
	if !response.status().is_success() {
		return Err(format!("snapshot HTTP {}", response.status()));
	}
	let body = response
		.json::<serde_json::Value>()
		.map_err(|e| format!("snapshot parse error: {e}"))?;
	body.get("snapshot")
		.and_then(|s| s.get("runtime"))
		.filter(|v| !v.is_null())
		.cloned()
		.ok_or_else(|| "snapshot.runtime is null or missing".to_string())
}

/// Capturer プロセスが `/healthz` で ok を返すまで polling する。途中で child が
/// 死んだ場合は stderr tail を添えて Err にする。
fn wait_for_healthz(
	http_client: &reqwest::blocking::Client,
	bind_addr: SocketAddr,
	child: &mut Child,
	stderr_tail: &Arc<Mutex<Vec<String>>>,
) -> Result<(), String> {
	let url = format!("http://{}/healthz", bind_addr);
	let started = Instant::now();
	loop {
		match child.try_wait() {
			Ok(Some(status)) => {
				let tail_text = stderr_tail
					.lock()
					.ok()
					.map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
					.unwrap_or_default();
				return Err(format!(
					"capturer exited before healthz (exit={:?})\n--- stderr tail ---\n{tail_text}",
					status.code()
				));
			}
			Ok(None) => {}
			Err(e) => return Err(format!("try_wait capturer: {e}")),
		}
		if http_client
			.get(&url)
			.send()
			.ok()
			.is_some_and(|response| response.status().is_success())
		{
			return Ok(());
		}
		if started.elapsed() >= HEALTHZ_WAIT_TIMEOUT {
			let tail_text = stderr_tail
				.lock()
				.ok()
				.map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
				.unwrap_or_default();
			return Err(format!(
				"capturer healthz timeout after {:.1}s\n--- stderr tail ---\n{tail_text}",
				started.elapsed().as_secs_f32()
			));
		}
		std::thread::sleep(HEALTHZ_POLL_INTERVAL);
	}
}

fn reserve_loopback_address() -> Result<SocketAddr, String> {
	let listener =
		TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).map_err(|e| format!("reserve loopback port: {e}"))?;
	listener.local_addr().map_err(|e| format!("loopback local addr: {e}"))
}

fn spawn_stderr_tail(stderr: Option<ChildStderr>) -> Arc<Mutex<Vec<String>>> {
	let tail = Arc::new(Mutex::new(Vec::new()));
	let Some(stderr) = stderr else {
		return tail;
	};
	let thread_tail = Arc::clone(&tail);
	std::thread::spawn(move || {
		for line in BufReader::new(stderr).lines().map_while(Result::ok) {
			let Ok(mut tail) = thread_tail.lock() else {
				return;
			};
			tail.push(line);
			let overflow = tail.len().saturating_sub(MAX_CAPTURER_LOG_LINES);
			if overflow > 0 {
				tail.drain(0..overflow);
			}
		}
	});
	tail
}

fn configure_hidden_child(_command: &mut Command) {
	#[cfg(windows)]
	{
		_command.creation_flags(CREATE_NO_WINDOW);
	}
}

/// `un-motion-capturer` を起動する Command を構築する。
/// release 配布構成 (隣接 exe) と開発 (`cargo run`) の両方を解決する。
///
/// Phase E core fix: cwd を明示的に **Capturer exe が解決できる最寄りの workspace
/// root** に設定して、MediaPipe Native の `models/*.task` 探索 (`unmotion_source::
/// configure_native_mediapipe_env_from_workspace`) が安定して当たるようにする。
/// Supervisor を Tauri 経由で起動した場合、cwd が想定外 (例えば `apps/un-motion-
/// supervisor/src-tauri/`) になり、Capturer プロセスがその cwd を継承して models を
/// 見失うという沈黙故障を防ぐ。
fn capturer_command(
	bind_addr: SocketAddr,
	allow_non_loopback: bool,
	active_profile: Option<&str>,
	api_worker_threads: usize,
) -> Result<Command, String> {
	let exe = capturer_executable_path();
	let cwd = capturer_working_directory(&exe);
	let api_worker_threads = normalize_api_worker_threads(api_worker_threads).to_string();
	if exe.is_file() {
		tracing::info!(
			exe = %exe.display(),
			cwd = %cwd.display(),
			active_profile = active_profile.unwrap_or("-"),
			"launching un-motion-capturer executable"
		);
		let mut command = Command::new(exe);
		command.current_dir(cwd).arg("--bind").arg(bind_addr.to_string());
		command.arg("--api-workers").arg(&api_worker_threads);
		if allow_non_loopback {
			command.arg("--allow-non-loopback");
		}
		if let Some(profile_id) = active_profile.filter(|value| !value.trim().is_empty()) {
			command.arg("--active-profile").arg(profile_id);
		}
		// Supervisor から spawn する場合は tray を有効化し、ユーザーが Console を閉じても
		// Capturer の存在を視認 / 終了できるようにする (要望: 「Capturer にシステムトレイ
		// 常駐能力を持たせ、動作中はシステムトレイにアイコンが出てホーバーやコンテキスト
		// メニューでプロセス ID も見えるよう工夫する」)。Capturer 自身が
		// `--supervisor-exe` を未指定なら隣接 exe を自動検出するので、ここでは
		// `--with-tray=true` のみ渡せば十分。
		command.arg("--with-tray=true");
		return Ok(command);
	}
	let repo = repo_root();
	tracing::info!(
		cwd = %repo.display(),
		active_profile = active_profile.unwrap_or("-"),
		"launching un-motion-capturer through cargo run fallback"
	);
	let mut command = Command::new("cargo");
	command
		.current_dir(repo)
		.args(["run", "-q", "-p", "un-motion-capturer", "--bin", "un-motion-capturer", "--"])
		.arg("--bind")
		.arg(bind_addr.to_string());
	command.arg("--api-workers").arg(&api_worker_threads);
	if allow_non_loopback {
		command.arg("--allow-non-loopback");
	}
	if let Some(profile_id) = active_profile.filter(|value| !value.trim().is_empty()) {
		command.arg("--active-profile").arg(profile_id);
	}
	command.arg("--with-tray=true");
	Ok(command)
}

/// Capturer プロセスを spawn するときの cwd を決める。
/// 隣接 exe の親ディレクトリに `models/` があればそれ (release zip 配布レイアウト)、
/// なければ `repo_root()` (cargo run 開発レイアウト) を返す。
fn capturer_working_directory(capturer_exe: &Path) -> PathBuf {
	if let Some(dir) = capturer_exe.parent() {
		if dir.join("models").is_dir() {
			return dir.to_path_buf();
		}
	}
	repo_root()
}

fn capturer_executable_path() -> PathBuf {
	capturer_executable_candidates()
		.into_iter()
		.find(|path| path.is_file())
		.unwrap_or_else(|| repo_root().join("target").join("debug").join(exe_name("un-motion-capturer")))
}

fn capturer_executable_candidates() -> Vec<PathBuf> {
	let current = std::env::current_exe().ok();
	let mut candidates = Vec::new();
	if let Some(dir) = current.as_ref().and_then(|path| path.parent()) {
		candidates.extend(capturer_candidates_from_dir(dir));
	}
	let repo = repo_root();
	if cfg!(debug_assertions) {
		candidates.extend(capturer_candidates_from_dir(&repo.join("target").join("debug")));
		candidates.extend(capturer_candidates_from_dir(&repo.join("target").join("release")));
	} else {
		candidates.extend(capturer_candidates_from_dir(&repo.join("target").join("release")));
		candidates.extend(capturer_candidates_from_dir(&repo.join("target").join("debug")));
	}
	candidates
}

fn capturer_candidates_from_dir(dir: &Path) -> Vec<PathBuf> {
	let mut existing = Vec::new();
	let mut missing = Vec::new();
	for path in [
		dir.join(exe_name("un-motion-capturer")),
		dir.join("runtimes").join(exe_name("un-motion-capturer")),
	] {
		push_capturer_candidate(path, &mut existing, &mut missing);
	}
	for path in versioned_capturer_sidecars(&dir.join("runtimes")) {
		push_capturer_candidate(path, &mut existing, &mut missing);
	}
	existing.sort_by(|left, right| right.0.cmp(&left.0));
	existing.into_iter().map(|(_, path)| path).chain(missing).collect()
}

fn push_capturer_candidate(path: PathBuf, existing: &mut Vec<(std::time::SystemTime, PathBuf)>, missing: &mut Vec<PathBuf>) {
	match fs::metadata(&path).and_then(|metadata| metadata.modified()) {
		Ok(modified) => existing.push((modified, path)),
		Err(_) => missing.push(path),
	}
}

fn versioned_capturer_sidecars(dir: &Path) -> Vec<PathBuf> {
	let Ok(entries) = fs::read_dir(dir) else {
		return Vec::new();
	};
	let prefix = "un-motion-capturer-";
	let suffix = exe_suffix();
	let mut paths: Vec<_> = entries
		.filter_map(Result::ok)
		.map(|entry| entry.path())
		.filter(|path| {
			path.file_name()
				.and_then(|name| name.to_str())
				.is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
		})
		.filter_map(|path| {
			let modified = fs::metadata(&path).and_then(|metadata| metadata.modified()).ok()?;
			Some((modified, path))
		})
		.collect();
	paths.sort_by(|left, right| right.0.cmp(&left.0));
	paths.into_iter().map(|(_, path)| path).collect()
}

fn exe_suffix() -> &'static str {
	if cfg!(windows) { ".exe" } else { "" }
}

fn supervisor_logs_dir() -> PathBuf {
	repo_root().join("target").join("tmp").join("supervisor-logs")
}

#[tauri::command]
fn save_supervisor_logs(content: String, file_prefix: String) -> Result<String, String> {
	let dir = supervisor_logs_dir();
	fs::create_dir_all(&dir).map_err(|e| format!("create logs dir: {e}"))?;
	let ts = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);
	let prefix: String = file_prefix.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
	let prefix = if prefix.is_empty() { "supervisor".to_string() } else { prefix };
	let path = dir.join(format!("{prefix}-{ts}.txt"));
	fs::write(&path, content.as_bytes()).map_err(|e| format!("write logs: {e}"))?;
	Ok(path.display().to_string())
}

#[tauri::command]
fn pick_file_path(kind: String) -> Result<Option<String>, String> {
	let mut dialog = rfd::FileDialog::new().set_directory(repo_root());
	dialog = match kind.as_str() {
		"icon" => dialog
			.add_filter("Image", &["png", "jpg", "jpeg", "ico", "webp"])
			.add_filter("All files", &["*"]),
		"image" | "file-image" => dialog
			.add_filter("Image", &["png", "jpg", "jpeg", "webp", "avif", "bmp"])
			.add_filter("All files", &["*"]),
		"video" | "file-video" => dialog
			.add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v"])
			.add_filter("All files", &["*"]),
		"ffmpeg" => dialog
			.add_filter("Executable", if cfg!(windows) { &["exe"] } else { &["*"] })
			.add_filter("All files", &["*"]),
		"sound" => dialog.add_filter("Sound", &["wav", "flac", "ogg"]).add_filter("All files", &["*"]),
		_ => dialog.add_filter("All files", &["*"]),
	};
	Ok(dialog.pick_file().map(|path| {
		if kind == "icon" {
			path_for_profile(&path)
		} else {
			path.display().to_string()
		}
	}))
}

fn path_for_profile(path: &Path) -> String {
	let repo = repo_root();
	let relative = path.strip_prefix(&repo).unwrap_or(path);
	relative.to_string_lossy().replace('\\', "/")
}

#[tauri::command]
fn reveal_supervisor_logs_dir() -> Result<(), String> {
	let dir = supervisor_logs_dir();
	fs::create_dir_all(&dir).map_err(|e| format!("create logs dir: {e}"))?;
	open_path_in_file_manager(&dir)
}

#[tauri::command]
fn reveal_profiles_dir() -> Result<(), String> {
	let store = user_profile_store();
	let dir = store.profiles_dir();
	fs::create_dir_all(dir).map_err(|e| format!("create profiles dir: {e}"))?;
	open_path_in_file_manager(dir)
}

fn open_path_in_file_manager(path: &Path) -> Result<(), String> {
	#[cfg(windows)]
	{
		let mut command = Command::new("explorer.exe");
		if path.is_file() {
			command.arg(format!("/select,{}", path.display()));
		} else {
			command.arg(path);
		}
		command.spawn().map_err(|e| format!("open explorer: {e}"))?;
		return Ok(());
	}

	#[cfg(target_os = "macos")]
	{
		let mut command = Command::new("open");
		if path.is_file() {
			command.arg("-R").arg(path);
		} else {
			command.arg(path);
		}
		command.spawn().map_err(|e| format!("open finder: {e}"))?;
		return Ok(());
	}

	#[cfg(all(unix, not(target_os = "macos")))]
	{
		let target = if path.is_file() { path.parent().unwrap_or(path) } else { path };
		Command::new("xdg-open")
			.arg(target)
			.spawn()
			.map_err(|e| format!("open file manager: {e}"))?;
		return Ok(());
	}
}

fn repo_root() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.parent()
		.and_then(Path::parent)
		.and_then(Path::parent)
		.expect("src-tauri is under apps/un-motion-supervisor/src-tauri")
		.to_path_buf()
}

fn exe_name(name: &str) -> String {
	if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn make_profile(id: &str, name: &str) -> CoreProfileDocumentProfile {
		CoreProfileDocumentProfile {
			id: id.to_string(),
			name: name.to_string(),
			created_at: String::new(),
			note: String::new(),
			icon_path: None,
			group: String::new(),
			default_source_enabled: true,
			default_source_label: "Default".to_string(),
			runtime_selection: None,
			pipeline_components: None,
		}
	}

	#[test]
	fn next_profile_id_slugifies_and_avoids_duplicates() {
		let mut doc = CoreProfileDocument::default();
		doc.profiles = vec![make_profile("alpha", "Alpha"), make_profile("alpha-2", "Alpha 2")];
		let id = next_profile_id(&doc, "Alpha");
		assert_eq!(id, "alpha-3");
	}

	#[test]
	fn next_profile_id_falls_back_when_name_lacks_ascii() {
		let mut doc = CoreProfileDocument::default();
		doc.profiles = vec![make_profile("profile", "_")];
		let id = next_profile_id(&doc, "！？");
		assert_eq!(id, "profile-2");
	}

	#[test]
	fn new_profile_runtime_defaults_match_release_profile_baseline() {
		let runtime = new_profile_runtime_defaults();
		assert_eq!(runtime.zenoh_enabled, Some(true));
		assert_eq!(runtime.zenoh_key_expr.as_deref(), Some("un-motion/frame"));
		assert_eq!(runtime.zenoh_topic_mode.as_deref(), Some("frame"));
		assert_eq!(runtime.vmc_enabled, Some(false));
		assert_eq!(runtime.media_pipe_delegate.as_deref(), Some("xnnpack"));
		assert_eq!(runtime.media_pipe_num_threads, Some(2));
		assert_eq!(runtime.media_pipe_holistic_flow_limiter_enabled, Some(true));
		assert_eq!(runtime.media_pipe_holistic_flow_limiter_max_in_flight, Some(1));
		assert_eq!(runtime.media_pipe_holistic_flow_limiter_max_in_queue, Some(1));

		let modifier = runtime.modifier.expect("modifier defaults");
		assert_eq!(modifier.head_enabled, Some(true));
		assert_eq!(modifier.face_enabled, Some(true));
		assert_eq!(modifier.hands_enabled, Some(true));
		assert_eq!(modifier.arms_ik_enabled, Some(true));
		assert_eq!(modifier.torso_enabled, Some(true));
		assert_eq!(modifier.legs_enabled, Some(false));
		assert_eq!(modifier.feet_enabled, Some(false));
		assert_eq!(modifier.torso_pitch_scale, Some(1.0));
		assert_eq!(modifier.smoothing_ema_enabled, Some(false));
		assert_eq!(modifier.smoothing_ema_alpha, Some(0.7));
		assert_eq!(modifier.smoothing_one_euro_enabled, Some(true));
		assert_eq!(modifier.smoothing_confidence_adaptive_cutoff, Some(true));
		assert_eq!(modifier.adaptive_min_cutoff_hz, Some(1.0));
		assert_eq!(modifier.adaptive_beta, Some(0.12));
		assert_eq!(modifier.adaptive_derivative_cutoff_hz, Some(1.0));
	}

	#[test]
	fn merge_profile_from_capturer_selection_preserves_unrelated_profiles() {
		let mut user_document = CoreProfileDocument {
			selected_profile_id: "global".to_string(),
			profiles: vec![make_profile("global", "Global Shutter Cam"), make_profile("waidayo", "Waidayo")],
			profile_sources: Vec::new(),
			next_profile_index: 3,
			next_source_index: 2,
		};
		user_document.profiles[1].note = "current waidayo".to_string();

		let mut capturer_document = user_document.clone();
		capturer_document.profiles[0].note = "updated global".to_string();
		capturer_document.profiles[1].note = "stale waidayo from capturer".to_string();

		let merged = merge_profile_from_capturer_selection("global", user_document, capturer_document).expect("merge");

		assert_eq!(merged.profiles[0].note, "updated global");
		assert_eq!(merged.profiles[1].note, "current waidayo");
	}

	#[test]
	fn apply_profile_field_updates_name_and_note() {
		let mut profile = make_profile("alpha", "Alpha");
		apply_profile_field(&mut profile, "name", json!(" Renamed ")).unwrap();
		apply_profile_field(&mut profile, "note", json!("hello")).unwrap();
		assert_eq!(profile.name, "Renamed");
		assert_eq!(profile.note, "hello");
	}

	#[test]
	fn apply_profile_field_round_trips_optional_runtime_fields() {
		let mut profile = make_profile("alpha", "Alpha");
		apply_profile_field(&mut profile, "runtime_selection.fps", json!(60)).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.zenoh_enabled", json!(true)).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.zenoh_key_expr", json!("un-motion/frame")).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.media_pipe_delegate", json!("cpu")).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.media_pipe_num_threads", json!(2)).unwrap();
		apply_profile_field(
			&mut profile,
			"runtime_selection.media_pipe_holistic_flow_limiter_enabled",
			json!(false),
		)
		.unwrap();
		apply_profile_field(
			&mut profile,
			"runtime_selection.media_pipe_holistic_flow_limiter_max_in_flight",
			json!(2),
		)
		.unwrap();
		apply_profile_field(
			&mut profile,
			"runtime_selection.media_pipe_holistic_flow_limiter_max_in_queue",
			json!(0),
		)
		.unwrap();
		let runtime = profile.runtime_selection.as_ref().expect("runtime allocated");
		assert_eq!(runtime.fps, Some(60));
		assert_eq!(runtime.zenoh_enabled, Some(true));
		assert_eq!(runtime.zenoh_key_expr.as_deref(), Some("un-motion/frame"));
		assert_eq!(runtime.media_pipe_delegate.as_deref(), Some("cpu"));
		assert_eq!(runtime.media_pipe_num_threads, Some(2));
		assert_eq!(runtime.media_pipe_holistic_flow_limiter_enabled, Some(false));
		assert_eq!(runtime.media_pipe_holistic_flow_limiter_max_in_flight, Some(2));
		assert_eq!(runtime.media_pipe_holistic_flow_limiter_max_in_queue, Some(0));

		// null で None に戻し、最終的に runtime_selection 全体が None に縮退することを確認。
		apply_profile_field(&mut profile, "runtime_selection.fps", json!(null)).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.zenoh_enabled", json!(null)).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.zenoh_key_expr", json!(null)).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.media_pipe_delegate", json!(null)).unwrap();
		apply_profile_field(&mut profile, "runtime_selection.media_pipe_num_threads", json!(null)).unwrap();
		apply_profile_field(
			&mut profile,
			"runtime_selection.media_pipe_holistic_flow_limiter_enabled",
			json!(null),
		)
		.unwrap();
		apply_profile_field(
			&mut profile,
			"runtime_selection.media_pipe_holistic_flow_limiter_max_in_flight",
			json!(null),
		)
		.unwrap();
		apply_profile_field(
			&mut profile,
			"runtime_selection.media_pipe_holistic_flow_limiter_max_in_queue",
			json!(null),
		)
		.unwrap();
		assert!(profile.runtime_selection.is_none());
	}

	#[test]
	fn apply_profile_field_updates_modifier_subfield() {
		let mut profile = make_profile("alpha", "Alpha");
		apply_profile_field(&mut profile, "runtime_selection.modifier.hands_enabled", json!(true)).unwrap();
		let modifier = profile
			.runtime_selection
			.as_ref()
			.and_then(|runtime| runtime.modifier.as_ref())
			.expect("tracking allocated");
		assert_eq!(modifier.hands_enabled, Some(true));

		apply_profile_field(&mut profile, "runtime_selection.modifier.hands_enabled", json!(null)).unwrap();
		assert!(profile.runtime_selection.is_none());
	}

	#[test]
	fn apply_profile_field_updates_pipeline_input_source_fields() {
		let mut profile = make_profile("alpha", "Alpha");
		apply_profile_field(&mut profile, "pipeline_components.input", json!("file-video")).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_path", json!("C:/tmp/sample.mp4")).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_fps", json!(24)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_width", json!(1280)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_height", json!(720)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_pixel_format", json!("YUY2")).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_repeat", json!(true)).unwrap();

		let pipeline = profile.pipeline_components.as_ref().expect("pipeline persisted");
		assert_eq!(pipeline.input.as_deref(), Some("file-video"));
		assert_eq!(pipeline.input_path.as_deref(), Some("C:/tmp/sample.mp4"));
		assert_eq!(pipeline.input_fps, Some(24));
		assert_eq!(pipeline.input_width, Some(1280));
		assert_eq!(pipeline.input_height, Some(720));
		assert_eq!(pipeline.input_pixel_format.as_deref(), Some("YUY2"));
		assert_eq!(pipeline.input_repeat, Some(true));

		apply_profile_field(&mut profile, "pipeline_components.input_path", json!(null)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_fps", json!(null)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_width", json!(null)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_height", json!(null)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_pixel_format", json!(null)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input_repeat", json!(null)).unwrap();
		apply_profile_field(&mut profile, "pipeline_components.input", json!(null)).unwrap();
		assert!(profile.pipeline_components.is_none());
	}

	#[test]
	fn apply_profile_field_rejects_unknown_path() {
		let mut profile = make_profile("alpha", "Alpha");
		let err = apply_profile_field(&mut profile, "runtime_selection.unknown_knob", json!(true)).unwrap_err();
		assert!(err.contains("unknown runtime_selection field"));
	}

	#[test]
	fn opt_string_trims_and_returns_none_for_empty() {
		assert_eq!(opt_string(&json!(null), "field").unwrap(), None);
		assert_eq!(opt_string(&json!("   "), "field").unwrap(), None);
		assert_eq!(opt_string(&json!("  hello "), "field").unwrap(), Some("hello".to_string()));
	}
}
