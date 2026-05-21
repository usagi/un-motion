use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use un_motion_frame::UNMotionFrame;
use un_motion_runtime::RuntimeSnapshot;

use un_motion_profile_schema::{
	CoreProfile, CoreProfileDocument, CoreProfileDocumentStore, document_from_profiles, document_profiles, normalize_profile_document,
};

use crate::runtime_host::{CoreRuntimeHost, CoreRuntimeStatusUpdate, NeutralCalibrationPose, core_runtime_config_from_document};

pub const DEFAULT_API_WORKER_THREADS: usize = 2;

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
struct CoreControlInner {
	state: RwLock<CoreMutableState>,
	profile_store: Option<CoreProfileDocumentStore>,
	runtime_host: Mutex<Option<CoreRuntimeHost>>,
	events: broadcast::Sender<CoreEvent>,
	next_event_sequence: AtomicU64,
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
		Ok(Self::from_profile_document(store.load(), Some(store)))
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
				profile_store,
				runtime_host: Mutex::new(None),
				events,
				next_event_sequence: AtomicU64::new(1),
			}),
		}
	}

	pub async fn snapshot(&self) -> CoreSnapshot {
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
		let normalized = if let Some(store) = &self.inner.profile_store {
			store.save(profile_document)?
		} else {
			normalize_profile_document(profile_document)
		};
		let was_running = self.stop_runtime_host();
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
			self.start_runtime_from_document(normalized.clone()).await;
		}
		Ok(normalized)
	}

	pub async fn start_runtime(&self) -> RuntimeStatus {
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
		let status_callback = self.runtime_status_callback();
		let host = match core_runtime_config_from_document(&document).and_then(|config| CoreRuntimeHost::spawn(config, status_callback)) {
			Ok(host) => host,
			Err(error) => {
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
		if let Ok(mut current) = self.inner.runtime_host.lock() {
			if current.is_some() {
				let _ = host.join();
				return self.status().await;
			}
			*current = Some(host);
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
		self.stop_runtime_host();
		let status = {
			let mut state = self.inner.state.write().await;
			state.status.running = false;
			state.status.health = "stopped".to_string();
			state.runtime_snapshot = None;
			state.status.updated_at_ms = now_unix_ms();
			state.status.clone()
		};
		self.emit(CoreEventKind::RuntimeStopped).await;
		status
	}

	pub async fn calibrate_neutral(&self, valid_sample_count: usize, pose: Option<&str>) -> anyhow::Result<CoreProfileDocument> {
		let pose = NeutralCalibrationPose::parse(pose)?;
		let result = {
			let host = self.inner.runtime_host.lock().map_err(|_| anyhow!("runtime host lock poisoned"))?;
			host.as_ref()
				.context("runtime is not running")?
				.calibrate_neutral(valid_sample_count, pose)?
		};
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
		self.set_profile_document(document).await
	}

	pub async fn clear_neutral_calibration(&self) -> anyhow::Result<CoreProfileDocument> {
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
		self.set_profile_document(document).await
	}

	pub async fn build_face_pose_model(&self, valid_sample_count: usize) -> anyhow::Result<CoreProfileDocument> {
		let result = {
			let host = self.inner.runtime_host.lock().map_err(|_| anyhow!("runtime host lock poisoned"))?;
			host.as_ref()
				.context("runtime is not running")?
				.build_face_pose_model(valid_sample_count)?
		};
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
		self.set_profile_document(document).await
	}

	pub async fn capture_unmotion_frame(&self) -> anyhow::Result<UNMotionFrame> {
		let host = self.inner.runtime_host.lock().map_err(|_| anyhow!("runtime host lock poisoned"))?;
		host.as_ref().context("runtime is not running")?.capture_unmotion_frame()
	}

	pub async fn set_active_profile(&self, profile_id: &str) -> anyhow::Result<RuntimeStatus> {
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
		let document = {
			let state = self.inner.state.read().await;
			if !state.profiles.iter().any(|profile| profile.id == profile_id) {
				bail!("profile not found: {profile_id}");
			}
			let mut document = state.profile_document.clone();
			document.selected_profile_id = profile_id.to_string();
			document
		};
		let normalized = if let Some(store) = &self.inner.profile_store {
			store.save(document)?
		} else {
			normalize_profile_document(document)
		};
		let was_running = self.stop_runtime_host();
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
		self.inner.runtime_host.lock().map(|host| host.is_some()).unwrap_or(false)
	}

	fn stop_runtime_host(&self) -> bool {
		let host = self.inner.runtime_host.lock().ok().and_then(|mut host| host.take());
		if let Some(host) = host {
			let _ = host.join();
			true
		} else {
			false
		}
	}

	fn runtime_status_callback(&self) -> std::sync::Arc<dyn Fn(CoreRuntimeStatusUpdate) + Send + Sync + 'static> {
		let inner = Arc::downgrade(&self.inner);
		Arc::new(move |update| {
			let Some(inner) = inner.upgrade() else {
				return;
			};
			let mut state = inner.state.blocking_write();
			if state.status.active_profile_id != update.active_profile_id {
				return;
			}
			state.status.running = update.running;
			state.status.health = update.health;
			state.status.frame_count = update.frame_count;
			state.status.packet_count = update.packet_count;
			state.runtime_snapshot = update.runtime_snapshot;
			state.status.updated_at_ms = now_unix_ms();
		})
	}
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
