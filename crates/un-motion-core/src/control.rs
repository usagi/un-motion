use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use arc_swap::ArcSwapOption;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, RwLock, broadcast};
use un_motion_frame::UNMotionFrame;
use un_motion_runtime::RuntimeSnapshot;

use un_motion_profile_schema::{
	CoreProfile, CoreProfileDocument, CoreProfileDocumentStore, document_from_profiles, document_profiles, normalize_profile_document,
};

use crate::runtime_host::{
	CoreRuntimeHandle, CoreRuntimeHost, CoreRuntimeStatusUpdate, NeutralCalibrationPose, core_runtime_config_from_document,
};

pub const DEFAULT_API_WORKER_THREADS: usize = 2;

#[derive(Debug)]
pub(crate) struct ProfileSyncTargetMissing(pub String);

impl std::fmt::Display for ProfileSyncTargetMissing {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(formatter, "profile sync rejected: active profile '{}' is missing; select a different profile explicitly before replacing it", self.0)
	}
}

impl std::error::Error for ProfileSyncTargetMissing {}

#[derive(Debug)]
pub(crate) struct ProfileSyncTargetChanged;

impl std::fmt::Display for ProfileSyncTargetChanged {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(formatter, "profile sync rejected: the active profile no longer matches the edited target")
	}
}

impl std::error::Error for ProfileSyncTargetChanged {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreApiConfig {
	pub bind_addr: SocketAddr,
	pub allow_non_loopback: bool,
	pub tray_enabled: bool,
	/// Actix Web API worker threads for this process.
	///
	/// Actix defaults this to system logical cores per process, which is too
	/// expensive when Supervisor launches multiple Capturer processes.
	pub api_worker_threads: usize,
	/// HTTP API listener が listen を開始する前に `start_runtime` を一度呼び、
	/// `selected_profile_id` で MediaPipe / Zenoh / VMC ワーカーを起動するか。
	///
	/// HTTP API listener が listen を開始する前に `start_runtime` を一度呼び、
	/// `selected_profile_id` で MediaPipe / Zenoh / VMC ワーカーを起動するか。
	/// `un-motion-capturer` は既定で `true` を渡し、起動 = 推論開始にする。
	pub auto_start_runtime: bool,
}

impl Default for CoreApiConfig {
	fn default() -> Self {
		Self {
			bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 39580),
			allow_non_loopback: false,
			tray_enabled: false,
			api_worker_threads: default_api_worker_threads(),
			auto_start_runtime: false,
		}
	}
}

pub fn logical_core_count() -> usize {
	std::thread::available_parallelism()
		.map_or(DEFAULT_API_WORKER_THREADS, NonZeroUsize::get)
		.max(1)
}

pub fn normalize_api_worker_threads(value: usize) -> usize {
	value.clamp(1, logical_core_count())
}

pub fn default_api_worker_threads() -> usize {
	normalize_api_worker_threads(DEFAULT_API_WORKER_THREADS)
}

impl CoreApiConfig {
	pub fn from_args<I, S>(args: I) -> anyhow::Result<Self>
	where
		I: IntoIterator<Item = S>,
		S: AsRef<str>,
	{
		let mut config = Self::default();
		let mut args = args.into_iter();
		while let Some(arg) = args.next() {
			let arg = arg.as_ref();
			if let Some(value) = arg.strip_prefix("--bind=") {
				config.bind_addr = parse_bind_addr(value)?;
			} else if arg == "--bind" {
				let Some(value) = args.next() else {
					bail!("--bind requires an address, for example --bind 127.0.0.1:39580");
				};
				config.bind_addr = parse_bind_addr(value.as_ref())?;
			} else if arg == "--allow-non-loopback" {
				config.allow_non_loopback = true;
			} else if arg == "--tray" {
				config.tray_enabled = true;
			} else if arg == "--no-tray" {
				config.tray_enabled = false;
			} else if let Some(value) = arg.strip_prefix("--api-workers=") {
				config.api_worker_threads = parse_api_worker_threads(value)?;
			} else if arg == "--api-workers" {
				let Some(value) = args.next() else {
					bail!("--api-workers requires a positive integer");
				};
				config.api_worker_threads = parse_api_worker_threads(value.as_ref())?;
			} else if arg == "--help" || arg == "-h" {
				bail!("usage: unmotion-core [--bind 127.0.0.1:39580] [--allow-non-loopback] [--tray|--no-tray] [--api-workers N]");
			} else {
				bail!("unknown argument: {arg}");
			}
		}
		config.validate()?;
		Ok(config)
	}

	pub fn validate(&self) -> anyhow::Result<()> {
		if !self.allow_non_loopback && !self.bind_addr.ip().is_loopback() {
			bail!(
				"refusing non-loopback API bind address {}; pass --allow-non-loopback to override",
				self.bind_addr
			);
		}
		Ok(())
	}

	pub fn normalized_api_worker_threads(&self) -> usize {
		normalize_api_worker_threads(self.api_worker_threads)
	}
}

fn parse_bind_addr(value: &str) -> anyhow::Result<SocketAddr> {
	value.parse().with_context(|| format!("invalid bind address: {value}"))
}

fn parse_api_worker_threads(value: &str) -> anyhow::Result<usize> {
	let parsed = value
		.parse::<usize>()
		.with_context(|| format!("invalid --api-workers value: {value}"))?;
	Ok(normalize_api_worker_threads(parsed))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
	pub running: bool,
	pub health: String,
	pub active_profile_id: String,
	pub frame_count: u64,
	pub packet_count: u64,
	pub updated_at_ms: u64,
}

impl RuntimeStatus {
	fn stopped(active_profile_id: String) -> Self {
		Self {
			running: false,
			health: "stopped".to_string(),
			active_profile_id,
			frame_count: 0,
			packet_count: 0,
			updated_at_ms: now_unix_ms(),
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreSnapshot {
	pub status: RuntimeStatus,
	pub profiles: Vec<CoreProfile>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub runtime: Option<RuntimeSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CoreEventKind {
	RuntimeStarted,
	RuntimeStopped,
	ActiveProfileChanged,
	Snapshot,
}

impl CoreEventKind {
	pub const fn sse_name(&self) -> &'static str {
		match self {
			Self::RuntimeStarted => "runtime-started",
			Self::RuntimeStopped => "runtime-stopped",
			Self::ActiveProfileChanged => "active-profile-changed",
			Self::Snapshot => "snapshot",
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreEvent {
	pub sequence: u64,
	pub kind: CoreEventKind,
	pub timestamp_ms: u64,
	pub snapshot: CoreSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveProfileRequest {
	pub profile_id: String,
}

#[derive(Debug)]
struct CoreMutableState {
	status: RuntimeStatus,
	profiles: Vec<CoreProfile>,
	profile_document: CoreProfileDocument,
	runtime_snapshot: Option<RuntimeSnapshot>,
}

#[derive(Debug)]
struct PendingRuntimeStatus {
	generation: u64,
	update: CoreRuntimeStatusUpdate,
}

#[derive(Debug)]
struct CoreControlInner {
	state: RwLock<CoreMutableState>,
	mutation: AsyncMutex<()>,
	profile_store: Option<CoreProfileDocumentStore>,
	runtime_host: Mutex<Option<CoreRuntimeHost>>,
	pending_runtime_status: ArcSwapOption<PendingRuntimeStatus>,
	runtime_status_apply: AsyncMutex<()>,
	runtime_generation: AtomicU64,
	#[cfg(test)]
	fail_next_runtime_start: std::sync::atomic::AtomicBool,
	events: broadcast::Sender<CoreEvent>,
	next_event_sequence: AtomicU64,
}

struct RuntimeStopOutcome {
	was_running: bool,
	error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CoreControlState {
	inner: Arc<CoreControlInner>,
}

impl Default for CoreControlState {
	fn default() -> Self {
		Self::new(vec![CoreProfile::default_profile()])
	}
}

impl CoreControlState {
	pub fn new(mut profiles: Vec<CoreProfile>) -> Self {
		if profiles.is_empty() {
			profiles.push(CoreProfile::default_profile());
		}
		let profile_document = document_from_profiles(profiles);
		Self::from_profile_document(profile_document, None)
	}

	pub fn from_workspace() -> anyhow::Result<Self> {
		let store = CoreProfileDocumentStore::from_workspace();
		Ok(Self::from_profile_document(store.load()?, Some(store)))
	}

	pub fn from_profile_document(profile_document: CoreProfileDocument, profile_store: Option<CoreProfileDocumentStore>) -> Self {
		let profile_document = normalize_profile_document(profile_document);
		let profiles = document_profiles(&profile_document);
		let active_profile_id = profile_document.selected_profile_id.clone();
		let (events, _) = broadcast::channel(128);
		Self {
			inner: Arc::new(CoreControlInner {
				state: RwLock::new(CoreMutableState {
					status: RuntimeStatus::stopped(active_profile_id),
					profiles,
					profile_document,
					runtime_snapshot: None,
				}),
				mutation: AsyncMutex::new(()),
				profile_store,
				runtime_host: Mutex::new(None),
				pending_runtime_status: ArcSwapOption::empty(),
				runtime_status_apply: AsyncMutex::new(()),
				runtime_generation: AtomicU64::new(0),
				#[cfg(test)]
				fail_next_runtime_start: std::sync::atomic::AtomicBool::new(false),
				events,
				next_event_sequence: AtomicU64::new(1),
			}),
		}
	}

	pub async fn snapshot(&self) -> CoreSnapshot {
		self.apply_pending_runtime_status().await;
		let state = self.inner.state.read().await;
		CoreSnapshot {
			status: state.status.clone(),
			profiles: state.profiles.clone(),
			runtime: state.runtime_snapshot.clone(),
		}
	}

	pub async fn status(&self) -> RuntimeStatus {
		self.snapshot().await.status
	}

	pub async fn profiles(&self) -> Vec<CoreProfile> {
		self.snapshot().await.profiles
	}

	pub async fn profile_document(&self) -> CoreProfileDocument {
		self.inner.state.read().await.profile_document.clone()
	}

	pub async fn set_profile_document(&self, profile_document: CoreProfileDocument) -> anyhow::Result<CoreProfileDocument> {
		let _mutation = self.inner.mutation.lock().await;
		self.set_profile_document_locked(profile_document).await
	}

	async fn set_profile_document_locked(&self, profile_document: CoreProfileDocument) -> anyhow::Result<CoreProfileDocument> {
		let profile_document = normalize_profile_document(profile_document);
		if self.runtime_host_is_running() {
			core_runtime_config_from_document(&profile_document)?;
		}
		let normalized = if let Some(store) = &self.inner.profile_store {
			store.save(profile_document)?
		} else {
			normalize_profile_document(profile_document)
		};
		self.apply_profile_document_locked(normalized).await
	}

	// Persistence is handled by the caller; do not write a runtime-only active ID back.
	async fn apply_profile_document_locked(&self, normalized: CoreProfileDocument) -> anyhow::Result<CoreProfileDocument> {
		if self.runtime_host_is_running() {
			core_runtime_config_from_document(&normalized)?;
		}
		let stop = self.stop_runtime_host().await;
		let was_running = stop.was_running;
		{
			let mut state = self.inner.state.write().await;
			state.profiles = document_profiles(&normalized);
			state.status.active_profile_id = normalized.selected_profile_id.clone();
			if was_running {
				state.status.running = false;
				state.status.health = "restarting".to_string();
				state.status.frame_count = 0;
				state.status.packet_count = 0;
				state.runtime_snapshot = None;
			}
			state.status.updated_at_ms = now_unix_ms();
			state.profile_document = normalized.clone();
		}
		self.emit(CoreEventKind::Snapshot).await;
		if was_running {
			let started = self.start_runtime_from_document(normalized.clone()).await;
			if !started.running {
				bail!("profile settings retained, but runtime restart failed: {}", started.health);
			}
		}
		Ok(normalized)
	}

	pub async fn sync_profile_document_preserving_active(
		&self,
		profile_document: CoreProfileDocument,
	) -> anyhow::Result<CoreProfileDocument> {
		self.sync_profile_document_for_target(profile_document, None).await
	}

	pub async fn sync_profile_document_for_target(
		&self,
		profile_document: CoreProfileDocument,
		expected_profile_id: Option<&str>,
	) -> anyhow::Result<CoreProfileDocument> {
		let _mutation = self.inner.mutation.lock().await;
		let (current_active_profile_id, old_active_profile) = {
			let state = self.inner.state.read().await;
			let active_profile_id = state.status.active_profile_id.clone();
			let old_active_profile = state
				.profile_document
				.profiles
				.iter()
				.find(|profile| profile.id == active_profile_id)
				.cloned();
			(active_profile_id, old_active_profile)
		};
		if expected_profile_id.is_some_and(|expected| expected != current_active_profile_id) {
			return Err(ProfileSyncTargetChanged.into());
		}
		let mut normalized = normalize_profile_document(profile_document);
		if !normalized.profiles.iter().any(|profile| profile.id == current_active_profile_id) {
			return Err(ProfileSyncTargetMissing(current_active_profile_id).into());
		}
		normalized.selected_profile_id = current_active_profile_id.clone();
		let new_active_profile = normalized
			.profiles
			.iter()
			.find(|profile| profile.id == normalized.selected_profile_id)
			.cloned();
		let was_running = self.runtime_host_is_running();
		let should_restart =
			was_running && (normalized.selected_profile_id != current_active_profile_id || old_active_profile != new_active_profile);
		if should_restart {
			// Validate output configuration while the previous runtime is still intact.
			core_runtime_config_from_document(&normalized)?;
			let _ = self.stop_runtime_host().await;
		}
		{
			let mut state = self.inner.state.write().await;
			state.profiles = document_profiles(&normalized);
			state.status.active_profile_id = normalized.selected_profile_id.clone();
			if should_restart {
				state.status.running = false;
				state.status.health = "restarting".to_string();
				state.status.frame_count = 0;
				state.status.packet_count = 0;
				state.runtime_snapshot = None;
			}
			state.status.updated_at_ms = now_unix_ms();
			state.profile_document = normalized.clone();
		}
		self.emit(CoreEventKind::Snapshot).await;
		if should_restart {
			let started = self.start_runtime_from_document(normalized.clone()).await;
			if !started.running {
				bail!("profile settings retained, but runtime restart failed: {}", started.health);
			}
		}
		Ok(normalized)
	}

	pub async fn start_runtime(&self) -> RuntimeStatus {
		let _mutation = self.inner.mutation.lock().await;
		if self.runtime_host_is_running() {
			return self.status().await;
		}
		let document = self.profile_document().await;
		self.start_runtime_from_document(document).await
	}

	async fn start_runtime_from_document(&self, document: CoreProfileDocument) -> RuntimeStatus {
		if self.runtime_host_is_running() {
			return self.status().await;
		}
		self.clear_pending_runtime_status().await;
		self.inner.runtime_generation.fetch_add(1, Ordering::AcqRel);
		let status_callback = self.runtime_status_callback();
		let host = match core_runtime_config_from_document(&document) {
			Ok(config) => {
				#[cfg(test)]
				let fail_start = self.inner.fail_next_runtime_start.swap(false, Ordering::AcqRel);
				#[cfg(not(test))]
				let fail_start = false;
				if fail_start {
					Err(anyhow!("injected runtime startup failure"))
				} else {
					run_blocking_runtime_operation(move || CoreRuntimeHost::spawn(config, status_callback)).await
				}
			},
			Err(error) => Err(error),
		};
		let host = match host {
			Ok(host) => host,
			Err(error) => {
				self.inner.runtime_generation.fetch_add(1, Ordering::AcqRel);
				let status = {
					let mut state = self.inner.state.write().await;
					state.status.running = false;
					state.status.health = format!("failed: {error}");
					state.runtime_snapshot = None;
					state.status.updated_at_ms = now_unix_ms();
					state.status.clone()
				};
				self.emit(CoreEventKind::RuntimeStopped).await;
				return status;
			}
		};
		let install_result = {
			let mut current = self.runtime_host_lock();
			if current.is_some() {
				Err(host)
			} else {
				*current = Some(host);
				Ok(())
			}
		};
		if let Err(rejected_host) = install_result {
			if let Some(error) = join_runtime_host(rejected_host).await {
				tracing::error!(error = %error, "rejected runtime host failed to stop");
			}
			return self.status().await;
		}
		let status = {
			let mut state = self.inner.state.write().await;
			state.status.running = true;
			state.status.health = "running".to_string();
			state.status.frame_count = 0;
			state.status.packet_count = 0;
			state.status.updated_at_ms = now_unix_ms();
			state.status.clone()
		};
		self.emit(CoreEventKind::RuntimeStarted).await;
		status
	}

	pub async fn stop_runtime(&self) -> RuntimeStatus {
		let _mutation = self.inner.mutation.lock().await;
		let stop = self.stop_runtime_host().await;
		let status = {
			let mut state = self.inner.state.write().await;
			state.status.running = false;
			state.status.health = stop.error.map_or_else(|| "stopped".to_string(), |error| format!("failed: {error}"));
			state.runtime_snapshot = None;
			state.status.updated_at_ms = now_unix_ms();
			state.status.clone()
		};
		self.emit(CoreEventKind::RuntimeStopped).await;
		status
	}

	async fn save_calibration_document_locked(&self, document: CoreProfileDocument, face_model: bool) -> anyhow::Result<CoreProfileDocument> {
		let active_id = document.selected_profile_id.clone();
		let normalized = if let Some(store) = &self.inner.profile_store {
			let incoming = document.profiles.iter().find(|p| p.id == active_id)
				.and_then(|p| p.runtime_selection.as_ref()).and_then(|r| r.modifier.as_ref())
				.context("calibration result has no modifier settings")?;
			let mut saved = store.update(|latest| {
				let profile = latest.profiles.iter_mut().find(|p| p.id == active_id)
					.context("calibration profile no longer exists")?;
				let modifier = profile.runtime_selection.get_or_insert_with(Default::default).modifier.get_or_insert_with(Default::default);
				if face_model {
					modifier.face_pose_model = incoming.face_pose_model.clone();
				} else {
					modifier.neutral_calibration_enabled = incoming.neutral_calibration_enabled;
					modifier.neutral_calibration_rotations = incoming.neutral_calibration_rotations.clone();
					modifier.neutral_calibration_pose = incoming.neutral_calibration_pose.clone();
				}
				Ok(())
			})?;
			// Keep this Capturer on its own profile, while preserving the shared default on disk.
			saved.selected_profile_id = active_id;
			saved
		} else { normalize_profile_document(document) };
		self.apply_profile_document_locked(normalized).await
	}

	pub async fn calibrate_neutral(&self, valid_sample_count: usize, pose: Option<&str>) -> anyhow::Result<CoreProfileDocument> {
		self.calibrate_neutral_for_profile(valid_sample_count, pose, None).await
	}

	pub async fn calibrate_neutral_for_profile(&self, valid_sample_count: usize, pose: Option<&str>, expected_profile_id: Option<&str>) -> anyhow::Result<CoreProfileDocument> {
		let _mutation = self.inner.mutation.lock().await;
		if let Some(expected) = expected_profile_id {
			let state = self.inner.state.read().await;
			anyhow::ensure!(state.status.active_profile_id == expected, "calibration_target_conflict: expected profile {expected}, active profile {}", state.status.active_profile_id);
		}
		let pose = NeutralCalibrationPose::parse(pose)?;
		let handle = self.runtime_handle()?;
		let result = run_blocking_runtime_operation(move || handle.calibrate_neutral(valid_sample_count, pose)).await?;
		let document = {
			let state = self.inner.state.read().await;
			let mut document = state.profile_document.clone();
			let active_profile_id = state.status.active_profile_id.clone();
			let profile = document
				.profiles
				.iter_mut()
				.find(|profile| profile.id == active_profile_id)
				.context("active profile not found")?;
			let runtime = profile.runtime_selection.get_or_insert_with(Default::default);
			let modifier = runtime.modifier.get_or_insert_with(Default::default);
			modifier.neutral_calibration_enabled = Some(true);
			modifier.neutral_calibration_rotations = Some(result.rotations);
			modifier.neutral_calibration_pose = Some(pose.as_profile_value().to_string());
			document
		};
		self.save_calibration_document_locked(document, false).await
	}

	pub async fn clear_neutral_calibration(&self) -> anyhow::Result<CoreProfileDocument> {
		self.clear_neutral_calibration_for_profile(None).await
	}

	pub async fn clear_neutral_calibration_for_profile(&self, expected_profile_id: Option<&str>) -> anyhow::Result<CoreProfileDocument> {
		let _mutation = self.inner.mutation.lock().await;
		if let Some(expected) = expected_profile_id {
			let state = self.inner.state.read().await;
			anyhow::ensure!(state.status.active_profile_id == expected, "calibration_target_conflict: expected profile {expected}, active profile {}", state.status.active_profile_id);
		}
		let document = {
			let state = self.inner.state.read().await;
			let mut document = state.profile_document.clone();
			let active_profile_id = state.status.active_profile_id.clone();
			let profile = document
				.profiles
				.iter_mut()
				.find(|profile| profile.id == active_profile_id)
				.context("active profile not found")?;
			let runtime = profile.runtime_selection.get_or_insert_with(Default::default);
			let modifier = runtime.modifier.get_or_insert_with(Default::default);
			modifier.neutral_calibration_enabled = Some(false);
			modifier.neutral_calibration_rotations = None;
			modifier.neutral_calibration_pose = None;
			document
		};
		self.save_calibration_document_locked(document, false).await
	}

	pub async fn build_face_pose_model(&self, valid_sample_count: usize) -> anyhow::Result<CoreProfileDocument> {
		self.build_face_pose_model_for_profile(valid_sample_count, None).await
	}

	pub async fn build_face_pose_model_for_profile(&self, valid_sample_count: usize, expected_profile_id: Option<&str>) -> anyhow::Result<CoreProfileDocument> {
		let _mutation = self.inner.mutation.lock().await;
		if let Some(expected) = expected_profile_id {
			let state = self.inner.state.read().await;
			anyhow::ensure!(state.status.active_profile_id == expected, "calibration_target_conflict: expected profile {expected}, active profile {}", state.status.active_profile_id);
		}
		let handle = self.runtime_handle()?;
		let result = run_blocking_runtime_operation(move || handle.build_face_pose_model(valid_sample_count)).await?;
		let document = {
			let state = self.inner.state.read().await;
			let mut document = state.profile_document.clone();
			let active_profile_id = state.status.active_profile_id.clone();
			let profile = document
				.profiles
				.iter_mut()
				.find(|profile| profile.id == active_profile_id)
				.context("active profile not found")?;
			let runtime = profile.runtime_selection.get_or_insert_with(Default::default);
			let modifier = runtime.modifier.get_or_insert_with(Default::default);
			modifier.face_pose_model = Some(un_motion_profile_schema::profile_settings::ProfileFacePoseModelSettings {
				enabled: Some(true),
				neutral_nose_drop_eye_mouth: Some(result.neutral_nose_drop_eye_mouth),
				sample_count: Some(result.valid_samples as u32),
				median_abs_yaw: Some(result.median_abs_yaw),
				median_abs_roll: Some(result.median_abs_roll),
			});
			document
		};
		self.save_calibration_document_locked(document, true).await
	}

	pub async fn capture_unmotion_frame(&self) -> anyhow::Result<UNMotionFrame> {
		let handle = self.runtime_handle()?;
		run_blocking_runtime_operation(move || handle.capture_unmotion_frame()).await
	}

	pub async fn capture_analysis_extras(
		&self,
		output_dir: std::path::PathBuf,
		duration_ms: u64,
	) -> anyhow::Result<Vec<serde_json::Value>> {
		let duration = std::time::Duration::from_millis(duration_ms.clamp(100, 3000));
		let handle = self.runtime_handle()?;
		run_blocking_runtime_operation(move || handle.capture_analysis_extras(output_dir, duration)).await
	}

	pub async fn set_active_profile(&self, profile_id: &str) -> anyhow::Result<RuntimeStatus> {
		let _mutation = self.inner.mutation.lock().await;
		// Idempotency: 既に同じ profile が active で runtime が running なら
		// 何もせず現在の status を返す。これは Supervisor が `launch_capturer` の
		// healthz 後に unconditional で `PUT /api/profiles/active` を呼ぶ運用
		// (auto_start_runtime と同一 profile を要求するケース) で
		// 「auto_start 直後に runtime が一度 stop されて restart される」
		// 沈黙故障を防ぐ。auto_start から数百 ms で stop+restart していた
		// バグの原因 (Capturer stderr で確認済み)。
		{
			let state = self.inner.state.read().await;
			if state.status.active_profile_id == profile_id && state.status.running && self.runtime_host_is_running() {
				return Ok(state.status.clone());
			}
		}
		let validate_runtime = self.runtime_host_is_running();
		let normalized = if let Some(store) = &self.inner.profile_store {
			store.update(|latest| {
				if !latest.profiles.iter().any(|profile| profile.id == profile_id) {
					bail!("profile not found: {profile_id}");
				}
				latest.selected_profile_id = profile_id.to_string();
				if validate_runtime {
					core_runtime_config_from_document(latest)?;
				}
				Ok(())
			})?
		} else {
			let state = self.inner.state.read().await;
			if !state.profiles.iter().any(|profile| profile.id == profile_id) {
				bail!("profile not found: {profile_id}");
			}
			let mut document = state.profile_document.clone();
			document.selected_profile_id = profile_id.to_string();
			normalize_profile_document(document)
		};
		if validate_runtime {
			core_runtime_config_from_document(&normalized)?;
		}
		let stop = self.stop_runtime_host().await;
		let was_running = stop.was_running;
		let status = {
			let mut state = self.inner.state.write().await;
			state.profiles = document_profiles(&normalized);
			state.status.active_profile_id = normalized.selected_profile_id.clone();
			state.profile_document = normalized.clone();
			if was_running {
				state.status.running = false;
				state.status.health = "restarting".to_string();
				state.status.frame_count = 0;
				state.status.packet_count = 0;
				state.runtime_snapshot = None;
			}
			state.status.updated_at_ms = now_unix_ms();
			state.status.clone()
		};
		self.emit(CoreEventKind::ActiveProfileChanged).await;
		if was_running {
			Ok(self.start_runtime_from_document(normalized).await)
		} else {
			Ok(status)
		}
	}

	pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
		self.inner.events.subscribe()
	}

	pub async fn snapshot_event(&self) -> CoreEvent {
		self.build_event(CoreEventKind::Snapshot).await
	}

	async fn emit(&self, kind: CoreEventKind) {
		let _ = self.inner.events.send(self.build_event(kind).await);
	}

	async fn build_event(&self, kind: CoreEventKind) -> CoreEvent {
		CoreEvent {
			sequence: self.inner.next_event_sequence.fetch_add(1, Ordering::Relaxed),
			kind,
			timestamp_ms: now_unix_ms(),
			snapshot: self.snapshot().await,
		}
	}

	fn runtime_host_is_running(&self) -> bool {
		self.runtime_host_lock().is_some()
	}

	fn runtime_handle(&self) -> anyhow::Result<CoreRuntimeHandle> {
		let host = self.runtime_host_lock();
		Ok(host.as_ref().context("runtime is not running")?.handle())
	}

	fn runtime_host_lock(&self) -> std::sync::MutexGuard<'_, Option<CoreRuntimeHost>> {
		self.inner.runtime_host.lock().unwrap_or_else(|poisoned| {
			tracing::error!("recovering poisoned runtime host lock");
			poisoned.into_inner()
		})
	}

	async fn stop_runtime_host(&self) -> RuntimeStopOutcome {
		self.inner.runtime_generation.fetch_add(1, Ordering::AcqRel);
		let _apply = self.inner.runtime_status_apply.lock().await;
		let host = self.runtime_host_lock().take();
		if let Some(host) = host {
			let error = join_runtime_host(host).await;
			if let Some(error) = &error {
				tracing::error!(error = %error, "runtime host failed to stop");
			}
			self.inner.pending_runtime_status.store(None);
			RuntimeStopOutcome { was_running: true, error }
		} else {
			RuntimeStopOutcome {
				was_running: false,
				error: None,
			}
		}
	}

	fn runtime_status_callback(&self) -> std::sync::Arc<dyn Fn(CoreRuntimeStatusUpdate) + Send + Sync + 'static> {
		let inner = Arc::downgrade(&self.inner);
		let generation = self.inner.runtime_generation.load(Ordering::Acquire);
		Arc::new(move |update| {
			let Some(inner) = inner.upgrade() else {
				return;
			};
			if inner.runtime_generation.load(Ordering::Acquire) == generation {
				inner
					.pending_runtime_status
					.store(Some(Arc::new(PendingRuntimeStatus { generation, update })));
			}
		})
	}

	async fn apply_pending_runtime_status(&self) {
		let _apply = self.inner.runtime_status_apply.lock().await;
		let Some(update) = self.inner.pending_runtime_status.swap(None) else {
			return;
		};
		if self.inner.runtime_generation.load(Ordering::Acquire) != update.generation {
			return;
		}
		let mut state = self.inner.state.write().await;
		if state.status.active_profile_id != update.update.active_profile_id {
			return;
		}
		state.status.running = update.update.running;
		state.status.health.clone_from(&update.update.health);
		state.status.frame_count = update.update.frame_count;
		state.status.packet_count = update.update.packet_count;
		state.runtime_snapshot.clone_from(&update.update.runtime_snapshot);
		state.status.updated_at_ms = now_unix_ms();
	}

	async fn clear_pending_runtime_status(&self) {
		let _apply = self.inner.runtime_status_apply.lock().await;
		self.inner.pending_runtime_status.store(None);
	}
}

async fn join_runtime_host(host: CoreRuntimeHost) -> Option<String> {
	match tokio::task::spawn_blocking(move || host.join()).await {
		Ok(result) => runtime_thread_join_error(result),
		Err(error) => Some(format!("runtime join task failed: {error}")),
	}
}

fn runtime_thread_join_error(result: std::thread::Result<()>) -> Option<String> {
	result
		.err()
		.map(|payload| format!("runtime thread panicked: {}", panic_payload_message(payload.as_ref())))
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> &str {
	if let Some(message) = payload.downcast_ref::<String>() {
		message
	} else if let Some(message) = payload.downcast_ref::<&'static str>() {
		message
	} else {
		"non-string panic payload"
	}
}

async fn run_blocking_runtime_operation<T, F>(operation: F) -> anyhow::Result<T>
where
	T: Send + 'static,
	F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
	tokio::task::spawn_blocking(operation)
		.await
		.map_err(|error| anyhow!("blocking runtime operation task failed: {error}"))?
}

fn now_unix_ms() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|duration| duration.as_millis() as u64)
		.unwrap_or_default()
}

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_profile_schema::CoreProfileDocumentProfile;

	#[tokio::test(flavor = "current_thread")]
	async fn blocking_runtime_operations_do_not_stall_async_tasks() {
		let operation = tokio::spawn(async {
			run_blocking_runtime_operation(|| {
				std::thread::sleep(std::time::Duration::from_millis(50));
				Ok(42)
			})
			.await
		});
		tokio::task::yield_now().await;

		tokio::time::timeout(
			std::time::Duration::from_millis(20),
			tokio::time::sleep(std::time::Duration::from_millis(1)),
		)
		.await
		.expect("async timer must remain responsive");
		assert_eq!(operation.await.expect("operation task").expect("blocking operation"), 42);
	}

	#[tokio::test]
	async fn runtime_status_callback_does_not_wait_for_state_lock() {
		let state = CoreControlState::default();
		let callback = state.runtime_status_callback();
		let state_guard = state.inner.state.write().await;
		callback(CoreRuntimeStatusUpdate {
			active_profile_id: "default".to_string(),
			running: true,
			health: "running".to_string(),
			frame_count: 12,
			packet_count: 34,
			runtime_snapshot: None,
		});
		assert!(state.inner.pending_runtime_status.load().is_some());
		drop(state_guard);

		let status = state.status().await;
		assert!(status.running);
		assert_eq!(status.frame_count, 12);
		assert_eq!(status.packet_count, 34);
	}

	#[tokio::test]
	async fn runtime_status_consumers_apply_the_latest_update_serially() {
		let state = CoreControlState::default();
		let callback = state.runtime_status_callback();
		let apply = state.inner.runtime_status_apply.lock().await;
		callback(CoreRuntimeStatusUpdate {
			active_profile_id: "default".to_string(),
			running: true,
			health: "first".to_string(),
			frame_count: 1,
			packet_count: 1,
			runtime_snapshot: None,
		});
		let status_state = state.clone();
		let mut status_task = tokio::spawn(async move { status_state.status().await });
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(20), &mut status_task)
				.await
				.is_err()
		);
		callback(CoreRuntimeStatusUpdate {
			active_profile_id: "default".to_string(),
			running: true,
			health: "latest".to_string(),
			frame_count: 2,
			packet_count: 2,
			runtime_snapshot: None,
		});
		drop(apply);

		let status = status_task.await.expect("status task");
		assert_eq!(status.health, "latest");
		assert_eq!(status.frame_count, 2);
	}

	#[tokio::test]
	async fn stale_runtime_status_callbacks_are_rejected() {
		let state = CoreControlState::default();
		let callback = state.runtime_status_callback();
		state.inner.runtime_generation.fetch_add(1, Ordering::AcqRel);
		callback(CoreRuntimeStatusUpdate {
			active_profile_id: "default".to_string(),
			running: true,
			health: "stale".to_string(),
			frame_count: 99,
			packet_count: 99,
			runtime_snapshot: None,
		});

		assert!(state.inner.pending_runtime_status.load().is_none());
		assert!(!state.status().await.running);
	}

	#[test]
	fn runtime_thread_join_errors_preserve_panic_messages() {
		assert_eq!(runtime_thread_join_error(Ok(())), None);
		assert_eq!(
			runtime_thread_join_error(Err(Box::new("runtime failure"))),
			Some("runtime thread panicked: runtime failure".to_string())
		);
		assert_eq!(
			runtime_thread_join_error(Err(Box::new(42_u32))),
			Some("runtime thread panicked: non-string panic payload".to_string())
		);
	}

	#[tokio::test]
	async fn poisoned_runtime_host_lock_remains_recoverable() {
		let state = CoreControlState::default();
		let inner = state.inner.clone();
		let panic = std::thread::spawn(move || {
			let _host = inner.runtime_host.lock().expect("runtime host lock");
			panic!("poison runtime host lock");
		})
		.join();
		assert!(panic.is_err());

		assert!(!state.runtime_host_is_running());
		assert!(state.runtime_handle().is_err());
		let stop = state.stop_runtime_host().await;
		assert!(!stop.was_running);
		assert!(stop.error.is_none());
	}

	#[tokio::test]
	async fn profile_mutations_wait_for_the_shared_operation_gate() {
		let state = CoreControlState::default();
		let document = state.profile_document().await;
		let gate = state.inner.mutation.lock().await;
		let task_state = state.clone();
		let mut task = tokio::spawn(async move { task_state.set_profile_document(document).await });

		assert!(tokio::time::timeout(std::time::Duration::from_millis(20), &mut task).await.is_err());
		drop(gate);
		task.await.expect("mutation task").expect("profile mutation");
	}

	#[tokio::test]
	async fn calibration_operations_wait_for_the_shared_operation_gate() {
		let state = CoreControlState::default();
		let gate = state.inner.mutation.lock().await;
		let calibration_state = state.clone();
		let mut calibration = tokio::spawn(async move { calibration_state.calibrate_neutral(1, None).await });
		let model_state = state.clone();
		let mut model = tokio::spawn(async move { model_state.build_face_pose_model(1).await });
		let clear_state = state.clone();
		let mut clear = tokio::spawn(async move { clear_state.clear_neutral_calibration().await });

		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(20), &mut calibration)
				.await
				.is_err()
		);
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(20), &mut model)
				.await
				.is_err()
		);
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(20), &mut clear)
				.await
				.is_err()
		);
		drop(gate);
		assert!(calibration.await.expect("calibration task").is_err());
		assert!(model.await.expect("model task").is_err());
		clear.await.expect("clear task").expect("clear calibration");
	}

	#[tokio::test]
	async fn start_stop_updates_status_and_emits_events() {
		let state = CoreControlState::default();
		let mut events = state.subscribe();

		let started = state.start_runtime().await;
		assert!(started.running);
		assert_eq!(started.health, "running");
		assert!(state.snapshot().await.runtime.is_some());
		let start_event = events.recv().await.expect("start event");
		assert_eq!(start_event.kind, CoreEventKind::RuntimeStarted);

		let stopped = state.stop_runtime().await;
		assert!(!stopped.running);
		assert_eq!(stopped.health, "stopped");
		assert!(state.snapshot().await.runtime.is_none());
		let stop_event = events.recv().await.expect("stop event");
		assert_eq!(stop_event.kind, CoreEventKind::RuntimeStopped);
	}

	#[tokio::test]
	async fn active_profile_requires_existing_profile() {
		let state = CoreControlState::new(vec![CoreProfile {
			id: "waidayo".to_string(),
			name: "Waidayo".to_string(),
			note: String::new(),
			icon_path: None,
			group: String::new(),
			engine: None,
		}]);

		assert!(state.set_active_profile("missing").await.is_err());
		assert_eq!(
			state.set_active_profile("waidayo").await.expect("profile").active_profile_id,
			"waidayo"
		);
	}

	#[tokio::test]
	async fn active_profile_selection_accepts_profile_added_after_initialization() {
		let root = std::env::temp_dir().join(format!("unm-core-new-selection-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
		let store = CoreProfileDocumentStore::from_root(&root);
		store.save(two_profile_document("p1")).unwrap();
		let state = CoreControlState::from_profile_document(store.load().unwrap(), Some(store.clone()));
		store.update(|doc| {
			let mut added = doc.profiles[1].clone();
			added.id = "p3".into();
			added.name = "Added in Supervisor".into();
			doc.profiles.push(added);
			Ok(())
		}).unwrap();
		let mut expected = store.load().unwrap();
		expected.selected_profile_id = "p3".into();
		let status = state.set_active_profile("p3").await.unwrap();
		assert_eq!(status.active_profile_id, "p3");
		assert!(!status.running);
		assert_eq!(store.load().unwrap(), expected);
		assert_eq!(state.profile_document().await, expected);
		let memory_only = CoreControlState::from_profile_document(two_profile_document("p1"), None);
		assert!(memory_only.set_active_profile("p3").await.is_err());
		assert_eq!(memory_only.status().await.active_profile_id, "p1");
		drop(state);
		std::fs::remove_dir_all(root).unwrap();
	}

	#[tokio::test]
	async fn active_profile_selection_preserves_newer_disk_edits() {
		let root = std::env::temp_dir().join(format!("unm-core-selection-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
		let store = CoreProfileDocumentStore::from_root(&root);
		store.save(two_profile_document("p1")).unwrap();
		let state = CoreControlState::from_profile_document(store.load().unwrap(), Some(store.clone()));
		store.update(|doc| {
			doc.profiles[0].note = "Edited while Capturer was open".into();
			doc.profiles[1].name = "New display name".into();
			Ok(())
		}).unwrap();
		let latest = store.load().unwrap();
		let status = state.set_active_profile("p2").await.unwrap();
		assert!(!status.running);
		let saved = store.load().unwrap();
		assert_eq!(saved.selected_profile_id, "p2");
		assert_eq!(saved.profiles, latest.profiles);
		assert_eq!(saved.profile_sources, latest.profile_sources);
		store.update(|doc| { doc.profiles.retain(|p| p.id != "p1"); Ok(()) }).unwrap();
		let deleted = store.load().unwrap();
		assert!(state.set_active_profile("p1").await.is_err());
		assert_eq!(store.load().unwrap(), deleted);
		drop(state);
		std::fs::remove_dir_all(root).unwrap();
	}

	#[tokio::test]
	async fn calibration_target_mismatch_rejects_before_runtime_or_document_changes() {
		let state = CoreControlState::from_profile_document(two_profile_document("p1"), None);
		let before = state.profile_document().await;
		let errors = [
			state.calibrate_neutral_for_profile(1, Some("U"), Some("p2")).await.unwrap_err(),
			state.clear_neutral_calibration_for_profile(Some("p2")).await.unwrap_err(),
			state.build_face_pose_model_for_profile(1, Some("p2")).await.unwrap_err(),
		];
		for error in errors {
			assert!(error.to_string().contains("calibration_target_conflict"), "{error}");
		}
		assert_eq!(state.profile_document().await, before);
		assert_eq!(state.status().await.active_profile_id, "p1");
		assert!(!state.status().await.running);
		// Matching target can clear without starting capture; measurement still needs a runtime.
		state.clear_neutral_calibration_for_profile(Some("p1")).await.unwrap();
	}

	#[tokio::test]
	async fn calibration_save_preserves_shared_default_and_newer_settings() {
		let root = std::env::temp_dir().join(format!("unm-core-calibration-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
		let store = CoreProfileDocumentStore::from_root(&root);
		store.save(two_profile_document("p1")).unwrap();
		let state = CoreControlState::from_profile_document(store.load().unwrap(), Some(store.clone()));
		store.update(|doc| {
			doc.selected_profile_id = "p2".into();
			doc.profiles[0].name = "New name".into();
			let modifier = doc.profiles[0].runtime_selection.get_or_insert_with(Default::default).modifier.get_or_insert_with(Default::default);
			modifier.hands_enabled = Some(false);
			modifier.neutral_calibration_enabled = Some(true);
			modifier.neutral_calibration_pose = Some("u".into());
			Ok(())
		}).unwrap();
		let mut expected = store.load().unwrap();
		let modifier = expected.profiles[0].runtime_selection.as_mut().unwrap().modifier.as_mut().unwrap();
		modifier.neutral_calibration_enabled = Some(false);
		modifier.neutral_calibration_pose = None;
		modifier.neutral_calibration_rotations = None;
		let cleared = state.clear_neutral_calibration().await.unwrap();
		assert_eq!(cleared.selected_profile_id, "p1");
		assert_eq!(store.load().unwrap(), expected);
		let mut result = cleared;
		let model = un_motion_profile_schema::profile_settings::ProfileFacePoseModelSettings { enabled: Some(true), sample_count: Some(90), ..Default::default() };
		result.profiles[0].runtime_selection.as_mut().unwrap().modifier.as_mut().unwrap().face_pose_model = Some(model.clone());
		result.profiles[0].name = "Stale result name".into();
		state.save_calibration_document_locked(result, true).await.unwrap();
		expected.profiles[0].runtime_selection.as_mut().unwrap().modifier.as_mut().unwrap().face_pose_model = Some(model);
		assert_eq!(store.load().unwrap(), expected);
		assert_eq!(state.status().await.active_profile_id, "p1");
		assert!(!state.status().await.running);
		drop(state);
		std::fs::remove_dir_all(root).unwrap();
	}

	#[tokio::test]
	async fn calibration_rejection_preserves_runtime_and_explicit_replace_remains_supported() {
		let root = std::env::temp_dir().join(format!("unm-core-save-reject-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
		let store = CoreProfileDocumentStore::from_root(&root);
		store.save(two_profile_document("p1")).unwrap();
		let state = CoreControlState::from_profile_document(store.load().unwrap(), Some(store.clone()));
		let before_runtime = state.profile_document().await;
		store.update(|doc| {
			doc.profiles.retain(|p| p.id != "p1");
			doc.selected_profile_id = "p2".into();
			Ok(())
		}).unwrap();
		let before_disk = store.load().unwrap();
		assert!(state.clear_neutral_calibration().await.is_err());
		assert_eq!(store.load().unwrap(), before_disk);
		assert_eq!(state.profile_document().await, before_runtime);
		assert_eq!(state.status().await.active_profile_id, "p1");
		assert!(!state.status().await.running);
		// A deliberate whole-document replacement still supports adding/removing profiles.
		let mut replacement = two_profile_document("p1");
		replacement.profiles.retain(|p| p.id == "p1");
		replacement.profiles[0].name = "Explicit replacement".into();
		state.set_profile_document(replacement).await.unwrap();
		let saved = store.load().unwrap();
		assert_eq!(saved.profiles.len(), 1);
		assert_eq!(saved.profiles[0].name, "Explicit replacement");
		assert_eq!(saved.selected_profile_id, "p1");
		assert_eq!(state.profile_document().await.profiles, saved.profiles);
		assert!(!state.status().await.running);
		drop(state);
		std::fs::remove_dir_all(root).unwrap();
	}

	#[tokio::test]
	async fn active_profile_change_restarts_running_runtime() {
		let state = CoreControlState::from_profile_document(two_profile_document("p1"), None);

		assert!(state.start_runtime().await.running);
		assert_eq!(runtime_profile_id(&state.snapshot().await), Some("p1".to_string()));

		let status = state.set_active_profile("p2").await.expect("active profile");

		assert!(status.running);
		assert_eq!(status.active_profile_id, "p2");
		assert_eq!(runtime_profile_id(&state.snapshot().await), Some("p2".to_string()));
		state.stop_runtime().await;
	}

	#[tokio::test]
	async fn profile_document_update_restarts_running_runtime_with_new_selection() {
		let state = CoreControlState::from_profile_document(two_profile_document("p1"), None);
		assert!(state.start_runtime().await.running);

		let updated = state
			.set_profile_document(two_profile_document("p2"))
			.await
			.expect("profile document");

		assert_eq!(updated.selected_profile_id, "p2");
		let snapshot = state.snapshot().await;
		assert!(snapshot.status.running);
		assert_eq!(snapshot.status.active_profile_id, "p2");
		assert_eq!(runtime_profile_id(&snapshot), Some("p2".to_string()));
		state.stop_runtime().await;
	}

	#[tokio::test]
	async fn profile_document_sync_preserves_running_active_profile() {
		let state = CoreControlState::from_profile_document(two_profile_document("p1"), None);
		assert!(state.start_runtime().await.running);

		let mut document = two_profile_document("p2");
		document.profiles[1].name = "Changed p2".to_string();
		let updated = state
			.sync_profile_document_preserving_active(document)
			.await
			.expect("profile document sync");

		assert_eq!(updated.selected_profile_id, "p1");
		let snapshot = state.snapshot().await;
		assert!(snapshot.status.running);
		assert_eq!(snapshot.status.active_profile_id, "p1");
		assert_eq!(runtime_profile_id(&snapshot), Some("p1".to_string()));
		assert_eq!(
			snapshot.profiles.iter().find(|profile| profile.id == "p2").unwrap().name,
			"Changed p2"
		);
		state.stop_runtime().await;
	}

	#[tokio::test]
	async fn profile_update_and_switch_invalid_output_preserve_running_runtime() {
		for switch in [false, true] {
			let mut original = two_profile_document("p1");
			let runtime = original.profiles[1].runtime_selection.get_or_insert_with(Default::default);
			runtime.vmc_enabled = Some(true);
			runtime.vmc_target_addr = Some("not-an-address".into());
			let root = std::env::temp_dir().join(format!("unm-invalid-output-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
			let store = CoreProfileDocumentStore::from_root(&root);
			store.save(original).unwrap();
			let disk_before = store.load().unwrap();
			let state = CoreControlState::from_profile_document(disk_before.clone(), Some(store.clone()));
			assert!(state.start_runtime().await.running);
			let before = state.profile_document().await;
			let failed = if switch {
				state.set_active_profile("p2").await.is_err()
			} else {
				let mut incoming = before.clone();
				incoming.selected_profile_id = "p2".into();
				state.set_profile_document(incoming).await.is_err()
			};
			let after = state.profile_document().await;
			let status = state.status().await;
			state.stop_runtime().await;
			assert!(failed, "invalid target must be refused, switch={switch}");
			assert_eq!(after, before);
			assert!(status.running);
			assert_eq!(status.active_profile_id, "p1");
			assert_eq!(store.load().unwrap(), disk_before);
			drop(state);
			assert!(root.starts_with(std::env::temp_dir()) && root.file_name().unwrap().to_string_lossy().starts_with("unm-invalid-output-"));
			std::fs::remove_dir_all(root).unwrap();
		}
	}

	#[tokio::test]
	async fn profile_restart_failure_is_reported_and_retained_settings_can_retry() {
		for sync in [false, true] {
			let state = CoreControlState::from_profile_document(two_profile_document("p1"), None);
			assert!(state.start_runtime().await.running);
			let mut incoming = state.profile_document().await;
			incoming.profiles[0].name = "Retained user edit".into();
			state.inner.fail_next_runtime_start.store(true, Ordering::Release);
			let result = if sync {
				state.sync_profile_document_for_target(incoming.clone(), Some("p1")).await
			} else {
				state.set_profile_document(incoming.clone()).await
			};
			assert!(result.unwrap_err().to_string().contains("injected runtime startup failure"));
			assert!(!state.status().await.running);
			assert!(!state.runtime_host_is_running());
			assert!(state.status().await.health.contains("injected runtime startup failure"));
			assert_eq!(state.profile_document().await, incoming);
			let retry = state.start_runtime().await;
			state.stop_runtime().await;
			assert!(retry.running);
			assert_eq!(retry.active_profile_id, "p1");
			assert_eq!(state.profile_document().await, incoming);
		}
	}

	#[tokio::test]
	async fn profile_sync_invalid_output_preserves_running_runtime() {
		let state = CoreControlState::from_profile_document(two_profile_document("p1"), None);
		assert!(state.start_runtime().await.running);
		let before = state.profile_document().await;
		let mut incoming = before.clone();
		let runtime = incoming.profiles[0].runtime_selection.get_or_insert_with(Default::default);
		runtime.vmc_enabled = Some(true);
		runtime.vmc_target_addr = Some("not-an-address".into());
		let result = state.sync_profile_document_for_target(incoming, Some("p1")).await;
		let after = state.profile_document().await;
		let status = state.status().await;
		state.stop_runtime().await;
		assert!(result.is_err(), "invalid output must be refused before stopping runtime");
		assert_eq!(after, before);
		assert!(status.running);
		assert_eq!(status.active_profile_id, "p1");
	}

	#[tokio::test]
	async fn profile_document_sync_rejects_missing_active_without_switching() {
		for empty in [false, true] {
			let original = two_profile_document("p1");
			let state = CoreControlState::from_profile_document(original, None);
			let before = state.profile_document().await;
			let mut incoming = two_profile_document("p2");
			incoming.profiles.retain(|profile| !empty && profile.id == "p2");
			incoming.profile_sources.clear();
			let error = state.sync_profile_document_preserving_active(incoming).await
				.expect_err("sync must not implicitly change active target");
			assert!(error.to_string().contains("p1"));
			assert_eq!(state.profile_document().await, before);
			assert_eq!(state.status().await.active_profile_id, "p1");
			assert!(!state.runtime_host_is_running());
		}
	}

	#[test]
	fn default_bind_is_loopback_and_non_loopback_requires_override() {
		assert!(CoreApiConfig::default().bind_addr.ip().is_loopback());
		assert!(CoreApiConfig::from_args(["--bind", "0.0.0.0:39580"]).is_err());
		let config = CoreApiConfig::from_args(["--bind", "0.0.0.0:39580", "--allow-non-loopback"]).expect("override");
		assert_eq!(config.bind_addr, "0.0.0.0:39580".parse::<SocketAddr>().expect("addr"));
	}

	#[test]
	fn tray_arg_is_parsed() {
		let config = CoreApiConfig::from_args(["--tray"]).expect("config");

		assert!(config.tray_enabled);
	}

	#[test]
	fn api_worker_threads_are_parsed_and_clamped() {
		assert_eq!(
			CoreApiConfig::default().normalized_api_worker_threads(),
			default_api_worker_threads()
		);

		let min = CoreApiConfig::from_args(["--api-workers", "0"]).expect("config");
		assert_eq!(min.api_worker_threads, 1);

		let max = CoreApiConfig::from_args(["--api-workers=9999"]).expect("config");
		assert_eq!(max.api_worker_threads, logical_core_count());
	}

	fn two_profile_document(selected_profile_id: &str) -> CoreProfileDocument {
		CoreProfileDocument {
			selected_profile_id: selected_profile_id.to_string(),
			profiles: vec![test_profile("p1"), test_profile("p2")],
			profile_sources: Vec::new(),
			next_profile_index: 3,
			next_source_index: 2,
		}
	}

	fn test_profile(id: &str) -> CoreProfileDocumentProfile {
		CoreProfileDocumentProfile {
			id: id.to_string(),
			name: id.to_string(),
			created_at: String::new(),
			note: String::new(),
			icon_path: None,
			group: String::new(),
			default_source_enabled: false,
			default_source_label: "UNMotion Default".to_string(),
			runtime_selection: None,
			pipeline_components: None,
		}
	}

	fn runtime_profile_id(snapshot: &CoreSnapshot) -> Option<String> {
		snapshot
			.runtime
			.as_ref()
			.and_then(|runtime| runtime.active_profile_id.as_ref())
			.map(|profile_id| profile_id.0.clone())
	}
}
