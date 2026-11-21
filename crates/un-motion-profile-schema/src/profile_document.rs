use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::CoreProfile;
use crate::profile_settings::{ProfilePipelineComponents, ProfileRuntimeSettings};

const CONF_FILE_NAME: &str = "conf.toml";
const PROFILE_TRANSACTION_MARKER: &str = ".profiles-transaction";
const NEXT_PROFILES_DIR: &str = ".profiles-next";
const PREVIOUS_PROFILES_DIR: &str = ".profiles-previous";
const NEXT_CONF_FILE: &str = ".conf.toml.next";
const PROFILE_STORE_LOCK_FILE: &str = ".profiles.lock";
const MAX_PROFILE_STORE_FILE_BYTES: u64 = 1_048_576;
const MAX_PROFILE_FILES: usize = 256;
const MAX_PROFILE_SOURCES: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreProfileDocument {
	#[serde(default = "default_profiles")]
	pub profiles: Vec<CoreProfileDocumentProfile>,
	#[serde(default)]
	pub profile_sources: Vec<CoreProfileDocumentSource>,
	#[serde(default = "default_selected_profile_id")]
	pub selected_profile_id: String,
	#[serde(default = "default_next_profile_index")]
	pub next_profile_index: u32,
	#[serde(default = "default_next_source_index")]
	pub next_source_index: u32,
}

impl Default for CoreProfileDocument {
	fn default() -> Self {
		Self {
			profiles: default_profiles(),
			profile_sources: Vec::new(),
			selected_profile_id: default_selected_profile_id(),
			next_profile_index: default_next_profile_index(),
			next_source_index: default_next_source_index(),
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreProfileDocumentProfile {
	pub id: String,
	pub name: String,
	#[serde(default)]
	pub created_at: String,
	#[serde(default)]
	pub note: String,
	#[serde(default)]
	pub icon_path: Option<String>,
	#[serde(default)]
	pub group: String,
	#[serde(default = "default_true")]
	pub default_source_enabled: bool,
	#[serde(default = "default_source_label")]
	pub default_source_label: String,
	#[serde(default)]
	pub runtime_selection: Option<ProfileRuntimeSettings>,
	#[serde(default)]
	pub pipeline_components: Option<ProfilePipelineComponents>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreProfileDocumentSource {
	pub id: String,
	pub profile_id: String,
	pub kind: String,
	pub label: String,
	#[serde(default)]
	pub protocol: Option<String>,
	#[serde(default)]
	pub host: Option<String>,
	#[serde(default)]
	pub port: Option<u16>,
	#[serde(default)]
	pub mirror_correction_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CoreProfileFile {
	id: String,
	name: String,
	#[serde(default)]
	created_at: String,
	#[serde(default)]
	note: String,
	#[serde(default)]
	icon_path: Option<String>,
	#[serde(default)]
	group: String,
	#[serde(default = "default_true")]
	default_source_enabled: bool,
	#[serde(default = "default_source_label")]
	default_source_label: String,
	#[serde(default)]
	sources: Vec<CoreProfileFileSource>,
	#[serde(default)]
	runtime_selection: Option<ProfileRuntimeSettings>,
	#[serde(default)]
	pipeline_components: Option<ProfilePipelineComponents>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CoreProfileFileSource {
	id: String,
	kind: String,
	label: String,
	#[serde(default)]
	protocol: Option<String>,
	#[serde(default)]
	host: Option<String>,
	#[serde(default)]
	port: Option<u16>,
	#[serde(default)]
	mirror_correction_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct CoreProfileDocumentStore {
	conf_path: PathBuf,
	profiles_dir: PathBuf,
}

impl CoreProfileDocumentStore {
	pub fn from_workspace() -> Self {
		let conf_path = resolve_conf_path();
		let profiles_dir = conf_path
			.parent()
			.map(|parent| parent.join("profiles"))
			.unwrap_or_else(|| PathBuf::from("profiles"));
		Self { conf_path, profiles_dir }
	}

	pub fn from_root(root: impl Into<PathBuf>) -> Self {
		let root = root.into();
		Self {
			conf_path: root.join(CONF_FILE_NAME),
			profiles_dir: root.join("profiles"),
		}
	}

	/// `from_root` のエイリアス。意図を明示するためだけに名前を変えた版で、
	/// 「ユーザーディレクトリ (例: `%APPDATA%\UN Motion\`) を root として扱う」
	/// 用途で呼ぶ。Phase E settings policy: Capturer / Supervisor は workspace
	/// `conf.toml` ではなく `%APPDATA%` (Linux なら `$XDG_CONFIG_HOME`) を user
	/// store として共有する。
	pub fn from_user_dir(user_root: impl Into<PathBuf>) -> Self {
		Self::from_root(user_root)
	}

	pub fn profiles_dir(&self) -> &std::path::Path {
		&self.profiles_dir
	}

	pub fn conf_path(&self) -> &std::path::Path {
		&self.conf_path
	}

	/// User profile store が未初期化 (`conf.toml` なし、かつ profile TOML なし) の
	/// 場合に限り、`template_dir/*.toml` を `self.profiles_dir/` にコピーする。
	///
	/// Phase E "Seed 廃止 + bundled templates + 初回コピー" の核。
	///
	/// * 初回起動 (= ユーザー設定 metadata も profile TOML もまだ無い) では bundled
	///   テンプレートを全部コピーして、ユーザーがすぐにそれをベースに編集できる状態にする。
	/// * 2 回目以降は **何もしない** (ユーザーが消した profile を勝手に復活させない /
	///   ユーザー編集を上書きしない)。
	/// * `template_dir` が存在しない場合も何もしない (release build で同梱忘れ等の
	///   フェイルセーフ)。
	///
	/// Returns `Ok(true)` if at least one template file was copied (= first run
	/// seeding actually happened), `Ok(false)` otherwise.
	pub fn seed_from_templates(&self, template_dir: &Path) -> anyhow::Result<bool> {
		if !template_dir.is_dir() {
			return Ok(false);
		}
		let _store_lock = self.acquire_store_lock()?;
		self.recover_profile_transaction()?;
		if self.conf_path.exists() {
			return Ok(false);
		}
		if has_any_toml(&self.profiles_dir) {
			return Ok(false);
		}
		let root = self.store_root();
		let next_profiles = root.join(NEXT_PROFILES_DIR);
		let previous_profiles = root.join(PREVIOUS_PROFILES_DIR);
		let marker = root.join(PROFILE_TRANSACTION_MARKER);
		let mut templates = Vec::new();
		let mut profile_ids = HashSet::new();
		let mut source_ids = HashSet::new();
		for entry in fs::read_dir(template_dir).with_context(|| format!("failed to read template dir {}", template_dir.display()))? {
			let entry = entry.with_context(|| format!("failed to read entry in template dir {}", template_dir.display()))?;
			let path = entry.path();
			if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
				continue;
			}
			if templates.len() >= MAX_PROFILE_FILES {
				bail!("template directory contains more than {MAX_PROFILE_FILES} profile files");
			}
			let raw = read_bounded_utf8(&path)?;
			let profile =
				toml::from_str::<CoreProfileFile>(&raw).with_context(|| format!("failed to parse profile template {}", path.display()))?;
			let profile_id = profile.id.trim();
			if profile_id.is_empty() {
				bail!("profile template {} has an empty profile id", path.display());
			}
			if !profile_ids.insert(profile_id.to_string()) {
				bail!("profile template {} has duplicate profile id '{profile_id}'", path.display());
			}
			for source in &profile.sources {
				if source_ids.len() >= MAX_PROFILE_SOURCES {
					bail!("profile templates contain more than {MAX_PROFILE_SOURCES} sources");
				}
				let source_id = source.id.trim();
				if source_id.is_empty() {
					bail!("profile template {} has an empty source id", path.display());
				}
				if !source_ids.insert(source_id.to_string()) {
					bail!("profile template {} has duplicate source id '{source_id}'", path.display());
				}
			}
			let file_name = path
				.file_name()
				.ok_or_else(|| anyhow::anyhow!("profile template has no file name: {}", path.display()))?
				.to_owned();
			templates.push((path, file_name, raw));
		}
		if templates.is_empty() {
			return Ok(false);
		}
		remove_path_if_exists(&next_profiles)?;
		remove_path_if_exists(&previous_profiles)?;
		fs::create_dir(&next_profiles)
			.with_context(|| format!("failed to create staged profiles directory {}", next_profiles.display()))?;
		for (path, file_name, raw) in templates {
			let dest = next_profiles.join(file_name);
			atomic_write(&dest, raw.as_bytes())
				.with_context(|| format!("failed to stage template {} -> {}", path.display(), dest.display()))?;
		}
		atomic_write(&marker, b"prepared\n")?;
		if self.profiles_dir.exists() {
			fs::rename(&self.profiles_dir, &previous_profiles).with_context(|| {
				format!(
					"failed to preserve profiles {} -> {}",
					self.profiles_dir.display(),
					previous_profiles.display()
				)
			})?;
		}
		fs::rename(&next_profiles, &self.profiles_dir)
			.with_context(|| format!("failed to install seeded profiles {}", self.profiles_dir.display()))?;
		remove_path_if_exists(&previous_profiles)?;
		remove_path_if_exists(&marker)?;
		Ok(true)
	}

	pub fn load(&self) -> anyhow::Result<CoreProfileDocument> {
		let _store_lock = self.acquire_store_lock()?;
		self.recover_profile_transaction()?;
		self.load_unlocked()
	}

	fn load_unlocked(&self) -> anyhow::Result<CoreProfileDocument> {
		let mut profiles = Vec::new();
		let mut profile_sources = Vec::new();
		let mut profile_file_count = 0;
		let conf = load_conf_document(&self.conf_path)?;
		let profile_manifest = conf.as_ref().and_then(profile_manifest_from_document);
		if self.profiles_dir.exists() {
			let entries = fs::read_dir(&self.profiles_dir)
				.with_context(|| format!("failed to read profiles directory {}", self.profiles_dir.display()))?;
			for entry in entries {
				let entry = entry.with_context(|| format!("failed to read entry in {}", self.profiles_dir.display()))?;
				let path = entry.path();
				if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
					continue;
				}
				profile_file_count += 1;
				if profile_file_count > MAX_PROFILE_FILES {
					bail!("profile store contains more than {MAX_PROFILE_FILES} profile files");
				}
				let raw = read_bounded_utf8(&path)?;
				let profile =
					toml::from_str::<CoreProfileFile>(&raw).with_context(|| format!("failed to parse profile {}", path.display()))?;
				if profile_manifest.as_ref().is_some_and(|manifest| !manifest.contains(&profile.id)) {
					continue;
				}
				for source in profile.sources {
					if profile_sources.len() >= MAX_PROFILE_SOURCES {
						bail!("profile store contains more than {MAX_PROFILE_SOURCES} sources");
					}
					let kind = normalize_source_kind(&source.kind);
					profile_sources.push(CoreProfileDocumentSource {
						id: source.id,
						profile_id: profile.id.clone(),
						kind: kind.clone(),
						label: source.label,
						protocol: normalize_source_protocol(source.protocol.as_deref(), &kind),
						host: Some(source.host.unwrap_or_else(|| default_source_host(&kind).to_string())),
						port: Some(normalize_source_port(source.port, &kind)),
						mirror_correction_enabled: kind == "vmc-osc" && source.mirror_correction_enabled,
					});
				}
				profiles.push(CoreProfileDocumentProfile {
					id: profile.id,
					name: profile.name,
					created_at: normalize_created_at(&profile.created_at, &path),
					note: profile.note,
					icon_path: normalize_optional_string(profile.icon_path),
					group: profile.group.trim().to_string(),
					default_source_enabled: profile.default_source_enabled,
					default_source_label: profile.default_source_label,
					runtime_selection: profile.runtime_selection,
					pipeline_components: profile.pipeline_components,
				});
			}
		}
		let profile_order = conf.as_ref().map(profile_order_from_document).unwrap_or_default();
		sort_profiles_by_saved_order(&mut profiles, &profile_order);
		if profiles.is_empty() && !self.conf_path.exists() {
			profiles = default_profiles();
		}
		Ok(normalize_profile_document(CoreProfileDocument {
			next_profile_index: (profiles.len() as u32).saturating_add(1).max(default_next_profile_index()),
			next_source_index: (profile_sources.len() as u32).saturating_add(2).max(default_next_source_index()),
			selected_profile_id: conf
				.as_ref()
				.and_then(active_profile_id_from_document)
				.unwrap_or_else(default_selected_profile_id),
			profiles,
			profile_sources,
		}))
	}

	pub fn save(&self, document: CoreProfileDocument) -> anyhow::Result<CoreProfileDocument> {
		let _store_lock = self.acquire_store_lock()?;
		self.recover_profile_transaction()?;
		self.save_unlocked(document)
	}

	/// Keep cooperating writers serialized across read, edit and publication.
	pub fn update(&self, edit: impl FnOnce(&mut CoreProfileDocument) -> anyhow::Result<()>) -> anyhow::Result<CoreProfileDocument> {
		let _store_lock = self.acquire_store_lock()?;
		self.recover_profile_transaction()?;
		let mut document = self.load_unlocked()?;
		let previous = document.clone();
		edit(&mut document)?;
		if document == previous { return Ok(document); }
		self.save_unlocked(document)
	}

	/// Selection is metadata only. Avoid a multi-file transaction whose recovery
	/// could publish a selection after its caller was told that saving failed.
	pub fn set_selected_profile_id(&self, profile_id: &str) -> anyhow::Result<()> {
		let _store_lock = self.acquire_store_lock()?;
		self.recover_profile_transaction()?;
		let document = self.load_unlocked()?;
		let selected = profile_id.trim();
		if !selected.is_empty() && !document.profiles.iter().any(|profile| profile.id == selected) {
			bail!("profile not found: {selected}");
		}
		if document.selected_profile_id == selected { return Ok(()); }
		let raw = format_profile_metadata(&self.conf_path, selected,
			&document.profiles.iter().map(|profile| profile.id.clone()).collect::<Vec<_>>())?;
		atomic_write(&self.conf_path, raw.as_bytes())
	}

	fn save_unlocked(&self, document: CoreProfileDocument) -> anyhow::Result<CoreProfileDocument> {
		validate_profile_document_limits(&document)?;
		let normalized = normalize_profile_document(document);
		let root = self.store_root();
		fs::create_dir_all(root).with_context(|| format!("failed to create profile store {}", root.display()))?;
		let next_profiles = root.join(NEXT_PROFILES_DIR);
		let previous_profiles = root.join(PREVIOUS_PROFILES_DIR);
		let next_conf = root.join(NEXT_CONF_FILE);
		let marker = root.join(PROFILE_TRANSACTION_MARKER);
		remove_path_if_exists(&next_profiles)?;
		remove_path_if_exists(&previous_profiles)?;
		remove_path_if_exists(&next_conf)?;
		fs::create_dir(&next_profiles)
			.with_context(|| format!("failed to create staged profiles directory {}", next_profiles.display()))?;
		let profile_file_names = unique_profile_file_names(&normalized.profiles);

		let mut profile_files = Vec::with_capacity(normalized.profiles.len());
		for (profile, file_name) in normalized.profiles.iter().zip(profile_file_names) {
			let sources = normalized
				.profile_sources
				.iter()
				.filter(|source| source.profile_id == profile.id)
				.map(|source| CoreProfileFileSource {
					id: source.id.clone(),
					kind: normalize_source_kind(&source.kind),
					label: source.label.clone(),
					protocol: normalize_source_protocol(source.protocol.as_deref(), &source.kind),
					host: Some(source.host.clone().unwrap_or_else(|| default_source_host(&source.kind).to_string())),
					port: Some(normalize_source_port(source.port, &source.kind)),
					mirror_correction_enabled: source.mirror_correction_enabled,
				})
				.collect();
			let file = CoreProfileFile {
				id: profile.id.clone(),
				name: profile.name.clone(),
				created_at: normalize_created_at(&profile.created_at, Path::new("")),
				note: profile.note.clone(),
				icon_path: normalize_optional_string(profile.icon_path.clone()),
				group: profile.group.trim().to_string(),
				default_source_enabled: profile.default_source_enabled,
				default_source_label: profile.default_source_label.clone(),
				sources,
				runtime_selection: profile.runtime_selection.clone(),
				pipeline_components: profile.pipeline_components.clone(),
			};
			let raw = toml::to_string_pretty(&file).context("failed to format profile TOML")?;
			let path = next_profiles.join(file_name);
			profile_files.push((path, raw));
		}
		for (path, raw) in profile_files {
			atomic_write(&path, raw.as_bytes()).with_context(|| format!("failed to write profile {}", path.display()))?;
		}

		let conf_raw = format_profile_metadata(
			&self.conf_path,
			&normalized.selected_profile_id,
			&normalized.profiles.iter().map(|profile| profile.id.clone()).collect::<Vec<_>>(),
		)?;
		atomic_write(&next_conf, conf_raw.as_bytes())?;
		atomic_write(&marker, b"prepared\n")?;
		if self.profiles_dir.exists() {
			fs::rename(&self.profiles_dir, &previous_profiles).with_context(|| {
				format!(
					"failed to preserve profiles {} -> {}",
					self.profiles_dir.display(),
					previous_profiles.display()
				)
			})?;
		}
		fs::rename(&next_profiles, &self.profiles_dir)
			.with_context(|| format!("failed to install profiles {}", self.profiles_dir.display()))?;
		atomic_write(&self.conf_path, conf_raw.as_bytes())?;
		remove_path_if_exists(&previous_profiles)?;
		remove_path_if_exists(&next_conf)?;
		remove_path_if_exists(&marker)?;
		Ok(normalized)
	}

	fn store_root(&self) -> &Path {
		self.conf_path
			.parent()
			.filter(|path| !path.as_os_str().is_empty())
			.unwrap_or_else(|| Path::new("."))
	}

	fn acquire_store_lock(&self) -> anyhow::Result<File> {
		let root = self.store_root();
		fs::create_dir_all(root).with_context(|| format!("failed to create profile store {}", root.display()))?;
		let lock_path = root.join(PROFILE_STORE_LOCK_FILE);
		let lock = OpenOptions::new()
			.read(true)
			.write(true)
			.create(true)
			.truncate(false)
			.open(&lock_path)
			.with_context(|| format!("failed to open profile store lock {}", lock_path.display()))?;
		lock.lock()
			.with_context(|| format!("failed to lock profile store {}", lock_path.display()))?;
		Ok(lock)
	}

	fn recover_profile_transaction(&self) -> anyhow::Result<()> {
		let root = self.store_root();
		let marker = root.join(PROFILE_TRANSACTION_MARKER);
		if !marker.exists() {
			return Ok(());
		}
		let next_profiles = root.join(NEXT_PROFILES_DIR);
		let previous_profiles = root.join(PREVIOUS_PROFILES_DIR);
		let next_conf = root.join(NEXT_CONF_FILE);
		if !self.profiles_dir.exists() {
			if next_profiles.exists() {
				fs::rename(&next_profiles, &self.profiles_dir)
					.with_context(|| format!("failed to recover profiles {}", self.profiles_dir.display()))?;
			} else if previous_profiles.exists() {
				// The staged profiles are unavailable: restore the old set with
				// its existing metadata, never publish the new set's selection.
				// Remove staged metadata before rename so an interrupted rollback
				// cannot be mistaken for a completed forward swap on the next load.
				remove_path_if_exists(&next_conf)?;
				fs::rename(&previous_profiles, &self.profiles_dir)
					.with_context(|| format!("failed to restore profiles {}", self.profiles_dir.display()))?;
			}
		}
		if self.profiles_dir.exists() && !next_profiles.exists() && next_conf.exists() {
			let raw = fs::read(&next_conf).with_context(|| format!("failed to read staged conf {}", next_conf.display()))?;
			atomic_write(&self.conf_path, &raw)?;
		}
		remove_path_if_exists(&next_profiles)?;
		remove_path_if_exists(&previous_profiles)?;
		remove_path_if_exists(&next_conf)?;
		remove_path_if_exists(&marker)?;
		Ok(())
	}
}

fn remove_path_if_exists(path: &Path) -> anyhow::Result<()> {
	if path.is_dir() {
		fs::remove_dir_all(path).with_context(|| format!("failed to remove directory {}", path.display()))?;
	} else if path.exists() {
		fs::remove_file(path).with_context(|| format!("failed to remove file {}", path.display()))?;
	}
	Ok(())
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
	let parent = path
		.parent()
		.filter(|parent| !parent.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new("."));
	fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
	let mut temporary =
		tempfile::NamedTempFile::new_in(parent).with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
	temporary
		.write_all(contents)
		.with_context(|| format!("failed to write temporary file for {}", path.display()))?;
	temporary
		.as_file_mut()
		.sync_all()
		.with_context(|| format!("failed to sync temporary file for {}", path.display()))?;
	temporary
		.persist(path)
		.map_err(|error| error.error)
		.with_context(|| format!("failed to replace {}", path.display()))?;
	Ok(())
}

pub fn normalize_profile_document(mut document: CoreProfileDocument) -> CoreProfileDocument {
	for profile in &mut document.profiles {
		profile.id = profile.id.trim().to_string();
	}
	let mut seen_profile_ids = HashSet::with_capacity(document.profiles.len());
	document
		.profiles
		.retain(|profile| !profile.id.is_empty() && seen_profile_ids.insert(profile.id.clone()));
	let profile_ids = document.profiles.iter().map(|profile| profile.id.clone()).collect::<HashSet<_>>();
	for source in &mut document.profile_sources {
		source.id = source.id.trim().to_string();
		source.profile_id = source.profile_id.trim().to_string();
	}
	let mut seen_source_ids = HashSet::with_capacity(document.profile_sources.len());
	document
		.profile_sources
		.retain(|source| !source.id.is_empty() && profile_ids.contains(&source.profile_id) && seen_source_ids.insert(source.id.clone()));
	for source in &mut document.profile_sources {
		source.kind = normalize_source_kind(&source.kind);
		source.protocol = normalize_source_protocol(source.protocol.as_deref(), &source.kind);
		source.host = Some(
			source
				.host
				.clone()
				.filter(|host| !host.trim().is_empty())
				.unwrap_or_else(|| default_source_host(&source.kind).to_string()),
		);
		source.port = Some(normalize_source_port(source.port, &source.kind));
		if source.kind != "vmc-osc" {
			source.mirror_correction_enabled = false;
		}
	}
	for profile in &mut document.profiles {
		if profile.created_at.trim().is_empty() {
			profile.created_at = current_timestamp_compact();
		}
		if profile.name.trim().is_empty() {
			profile.name = profile.id.clone();
		}
		if profile.default_source_label.trim().is_empty() {
			profile.default_source_label = default_source_label();
		}
		profile.icon_path = normalize_optional_string(profile.icon_path.clone());
		profile.group = profile.group.trim().to_string();
		let pipeline_engine = profile
			.pipeline_components
			.as_ref()
			.and_then(|pipeline| pipeline.engine.as_deref())
			.and_then(normalize_runtime_engine_id)
			.map(ToOwned::to_owned);
		if let Some(runtime) = profile.runtime_selection.as_mut() {
			runtime.engine = runtime
				.engine
				.as_deref()
				.and_then(normalize_runtime_engine_id)
				.map(ToOwned::to_owned)
				.or(pipeline_engine)
				.or_else(|| Some("mediapipe-native".to_string()));
		} else if let Some(engine) = pipeline_engine {
			profile.runtime_selection = Some(ProfileRuntimeSettings {
				engine: Some(engine),
				..ProfileRuntimeSettings::default()
			});
		}
	}
	document.selected_profile_id = document.selected_profile_id.trim().to_string();
	if !profile_ids.contains(&document.selected_profile_id) {
		document.selected_profile_id = document.profiles.first().map(|profile| profile.id.clone()).unwrap_or_default();
	}
	document.next_profile_index = document.next_profile_index.max(default_next_profile_index());
	document.next_source_index = document.next_source_index.max(default_next_source_index());
	document
}

/// 一覧表示用: `runtime_selection.engine` を使う。
pub fn profile_engine_summary(profile: &CoreProfileDocumentProfile) -> Option<String> {
	profile
		.runtime_selection
		.as_ref()
		.and_then(|r| r.engine.as_deref())
		.map(str::trim)
		.filter(|s| !s.is_empty())
		.map(ToOwned::to_owned)
}

pub fn document_profiles(document: &CoreProfileDocument) -> Vec<CoreProfile> {
	document
		.profiles
		.iter()
		.map(|profile| CoreProfile {
			id: profile.id.clone(),
			name: profile.name.clone(),
			note: profile.note.clone(),
			icon_path: profile.icon_path.clone(),
			group: profile.group.clone(),
			engine: profile_engine_summary(profile),
		})
		.collect()
}

pub fn document_from_profiles(profiles: Vec<CoreProfile>) -> CoreProfileDocument {
	let profiles = profiles
		.into_iter()
		.map(|profile| CoreProfileDocumentProfile {
			id: profile.id,
			name: profile.name,
			created_at: current_timestamp_compact(),
			note: profile.note,
			icon_path: profile.icon_path,
			group: profile.group.trim().to_string(),
			default_source_enabled: true,
			default_source_label: default_source_label(),
			runtime_selection: profile
				.engine
				.filter(|e| !e.trim().is_empty())
				.map(|engine| ProfileRuntimeSettings {
					engine: Some(engine),
					..Default::default()
				}),
			pipeline_components: None,
		})
		.collect::<Vec<_>>();
	normalize_profile_document(CoreProfileDocument {
		selected_profile_id: profiles.first().map(|profile| profile.id.clone()).unwrap_or_default(),
		profiles,
		profile_sources: Vec::new(),
		next_profile_index: default_next_profile_index(),
		next_source_index: default_next_source_index(),
	})
}

/// `dir/*.toml` が 1 件でも存在するかどうかを軽量にチェックする。`dir` 自体が
/// 無い / 読めない場合は `false`。`seed_from_templates` の早期 return 用。
fn has_any_toml(dir: &Path) -> bool {
	let Ok(entries) = fs::read_dir(dir) else {
		return false;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
			return true;
		}
	}
	false
}

fn read_bounded_utf8(path: &Path) -> anyhow::Result<String> {
	let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
	let mut bytes = Vec::new();
	file.take(MAX_PROFILE_STORE_FILE_BYTES + 1)
		.read_to_end(&mut bytes)
		.with_context(|| format!("failed to read {}", path.display()))?;
	if bytes.len() as u64 > MAX_PROFILE_STORE_FILE_BYTES {
		bail!(
			"{} exceeds the profile store file size limit of {MAX_PROFILE_STORE_FILE_BYTES} bytes",
			path.display()
		);
	}
	String::from_utf8(bytes).with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn validate_profile_document_limits(document: &CoreProfileDocument) -> anyhow::Result<()> {
	if document.profiles.len() > MAX_PROFILE_FILES {
		bail!("profile document contains more than {MAX_PROFILE_FILES} profiles");
	}
	if document.profile_sources.len() > MAX_PROFILE_SOURCES {
		bail!("profile document contains more than {MAX_PROFILE_SOURCES} sources");
	}
	Ok(())
}

fn default_profiles() -> Vec<CoreProfileDocumentProfile> {
	vec![CoreProfileDocumentProfile {
		id: default_selected_profile_id(),
		name: "Default".to_string(),
		// This is a virtual fallback, not a newly created persisted profile.
		created_at: "unknown".to_string(),
		note: "UNMotion Default".to_string(),
		icon_path: None,
		group: String::new(),
		default_source_enabled: true,
		default_source_label: default_source_label(),
		runtime_selection: None,
		pipeline_components: None,
	}]
}

fn default_selected_profile_id() -> String {
	"default".to_string()
}

fn default_next_profile_index() -> u32 {
	2
}

fn default_next_source_index() -> u32 {
	2
}

fn default_source_label() -> String {
	"UNMotion Default".to_string()
}

fn default_true() -> bool {
	true
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
	value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

fn normalize_source_kind(kind: &str) -> String {
	match kind {
		"osc" | "vmc-osc" | "mp-osc" => "vmc-osc".to_string(),
		"ifacialmocap" => "ifacialmocap".to_string(),
		_ => "unmotion".to_string(),
	}
}

fn normalize_runtime_engine_id(engine: &str) -> Option<&'static str> {
	match engine.trim() {
		"media-pipe-native" | "mediapipe-native" => Some("mediapipe-native"),
		"vmc" => Some("vmc"),
		"ifacialmocap" => Some("ifacialmocap"),
		_ => None,
	}
}

fn normalize_source_protocol(protocol: Option<&str>, kind: &str) -> Option<String> {
	let value = match normalize_source_kind(kind).as_str() {
		"ifacialmocap" => match protocol {
			Some("tcp") => "tcp",
			_ => "udp",
		},
		_ => "udp",
	};
	Some(value.to_string())
}

fn default_source_host(kind: &str) -> &'static str {
	match normalize_source_kind(kind).as_str() {
		"vmc-osc" | "ifacialmocap" => "0.0.0.0",
		_ => "127.0.0.1",
	}
}

fn normalize_source_port(port: Option<u16>, kind: &str) -> u16 {
	if let Some(value) = port.filter(|value| *value > 0) {
		return value;
	}
	match normalize_source_kind(kind).as_str() {
		"vmc-osc" => 39539,
		"ifacialmocap" => 49983,
		_ => 39539,
	}
}

fn profile_file_name(profile: &CoreProfileDocumentProfile) -> String {
	format!(
		"{}-{}.toml",
		normalize_created_at(&profile.created_at, Path::new("")),
		sanitize_profile_file_label(&profile.name)
	)
}

fn unique_profile_file_names(profiles: &[CoreProfileDocumentProfile]) -> Vec<String> {
	let mut used = HashSet::with_capacity(profiles.len());
	profiles
		.iter()
		.map(|profile| {
			let base = profile_file_name(profile);
			if used.insert(base.clone()) {
				return base;
			}
			let stem = base.trim_suffix(".toml");
			for suffix in 2_u32.. {
				let candidate = format!("{stem}-{suffix}.toml");
				if used.insert(candidate.clone()) {
					return candidate;
				}
			}
			unreachable!("u32 profile filename suffixes exhausted")
		})
		.collect()
}

fn normalize_created_at(value: &str, path: &Path) -> String {
	let trimmed = value.trim();
	if is_compact_timestamp(trimmed) {
		return trimmed.to_string();
	}
	if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
		let prefix = stem.split('-').next().unwrap_or_default();
		if is_compact_timestamp(prefix) {
			return prefix.to_string();
		}
	}
	// Reads must be deterministic: inventing the current time changes optimistic
	// save/delete revisions even when the underlying file has not changed.
	"unknown".to_string()
}

fn is_compact_timestamp(value: &str) -> bool {
	value.len() == 16
		&& value.as_bytes().get(8) == Some(&b'T')
		&& value.as_bytes().get(15) == Some(&b'Z')
		&& value
			.chars()
			.enumerate()
			.all(|(index, ch)| matches!(index, 8 | 15) || ch.is_ascii_digit())
}

fn sanitize_profile_file_label(name: &str) -> String {
	let mut out = String::new();
	let mut prev_dash = false;
	for lower in name.trim().chars().flat_map(char::to_lowercase) {
		let invalid = matches!(lower, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || lower.is_control();
		let ch = if invalid || lower.is_whitespace() { '-' } else { lower };
		if ch == '-' {
			if !prev_dash {
				out.push('-');
			}
			prev_dash = true;
		} else {
			out.push(ch);
			prev_dash = false;
		}
		if out.len() >= 96 {
			break;
		}
	}
	let trimmed = out.trim_matches(|ch| matches!(ch, '-' | '.' | ' ')).to_string();
	let label = if trimmed.is_empty() { "profile".to_string() } else { trimmed };
	let upper = label.to_ascii_uppercase();
	let reserved = matches!(
		upper.as_str(),
		"CON"
			| "PRN" | "AUX"
			| "NUL" | "COM1"
			| "COM2" | "COM3"
			| "COM4" | "COM5"
			| "COM6" | "COM7"
			| "COM8" | "COM9"
			| "LPT1" | "LPT2"
			| "LPT3" | "LPT4"
			| "LPT5" | "LPT6"
			| "LPT7" | "LPT8"
			| "LPT9"
	);
	if reserved { format!("{label}-profile") } else { label }
}

fn current_timestamp_compact() -> String {
	let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
	let (year, month, day, hour, minute, second) = unix_seconds_to_utc(secs);
	format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

fn unix_seconds_to_utc(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
	let days = secs.div_euclid(86_400);
	let rem = secs.rem_euclid(86_400);
	let (year, month, day) = civil_from_days(days);
	let hour = (rem / 3_600) as u32;
	let minute = ((rem % 3_600) / 60) as u32;
	let second = (rem % 60) as u32;
	(year, month, day, hour, minute, second)
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
	let z = days + 719_468;
	let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
	let doe = z - era * 146_097;
	let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
	let y = yoe + era * 400;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let d = doy - (153 * mp + 2) / 5 + 1;
	let m = mp + if mp < 10 { 3 } else { -9 };
	let year = y + if m <= 2 { 1 } else { 0 };
	(year as i32, m as u32, d as u32)
}

fn resolve_conf_path() -> PathBuf {
	resolve_workspace_conf_path()
}

pub fn resolve_workspace_conf_path() -> PathBuf {
	let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
	for dir in cwd.ancestors() {
		let candidate = dir.join(CONF_FILE_NAME);
		if candidate.exists() {
			return candidate;
		}
	}
	cwd.join(CONF_FILE_NAME)
}

fn active_profile_id_from_document(doc: &toml::Value) -> Option<String> {
	doc.get("desktop")?.get("active_profile_id")?.as_str().map(ToString::to_string)
}

fn profile_order_from_document(doc: &toml::Value) -> Vec<String> {
	doc.get("desktop")
		.and_then(|desktop| desktop.get("profile_order"))
		.and_then(toml::Value::as_array)
		.map(|items| {
			items
				.iter()
				.filter_map(toml::Value::as_str)
				.map(ToString::to_string)
				.collect::<Vec<_>>()
		})
		.unwrap_or_default()
}

fn profile_manifest_from_document(doc: &toml::Value) -> Option<HashSet<String>> {
	let ids = doc.get("desktop")?.get("profile_ids")?.as_array()?;
	Some(ids.iter().filter_map(toml::Value::as_str).map(ToString::to_string).collect())
}

fn load_conf_document(path: &Path) -> anyhow::Result<Option<toml::Value>> {
	match read_bounded_utf8(path) {
		Ok(raw) => raw
			.parse::<toml::Value>()
			.with_context(|| format!("failed to parse {}", path.display()))
			.map(Some),
		Err(error)
			if error
				.downcast_ref::<std::io::Error>()
				.is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
		{
			Ok(None)
		}
		Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
	}
}

fn sort_profiles_by_saved_order(profiles: &mut [CoreProfileDocumentProfile], order: &[String]) {
	let order_index = order
		.iter()
		.enumerate()
		.map(|(index, id)| (id.as_str(), index))
		.collect::<std::collections::HashMap<_, _>>();
	profiles.sort_by(
		|left, right| match (order_index.get(left.id.as_str()), order_index.get(right.id.as_str())) {
			(Some(left_index), Some(right_index)) => left_index.cmp(right_index),
			(Some(_), None) => std::cmp::Ordering::Less,
			(None, Some(_)) => std::cmp::Ordering::Greater,
			(None, None) => left.name.cmp(&right.name).then(left.id.cmp(&right.id)),
		},
	);
}

fn format_profile_metadata(path: &Path, active_profile_id: &str, profile_order: &[String]) -> anyhow::Result<String> {
	let mut doc = match read_bounded_utf8(path) {
		Ok(raw) => raw
			.parse::<toml::Value>()
			.with_context(|| format!("failed to parse {}", path.display()))?,
		Err(error)
			if error
				.downcast_ref::<std::io::Error>()
				.is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
		{
			toml::Value::Table(toml::map::Map::new())
		}
		Err(error) => return Err(error).with_context(|| format!("failed to read {}", path.display())),
	};
	if !doc.is_table() {
		doc = toml::Value::Table(toml::map::Map::new());
	}
	let Some(root) = doc.as_table_mut() else {
		bail!("failed to access conf.toml root table");
	};
	if !root.contains_key("desktop") || !root.get("desktop").is_some_and(toml::Value::is_table) {
		root.insert("desktop".to_string(), toml::Value::Table(toml::map::Map::new()));
	}
	let Some(desktop) = root.get_mut("desktop").and_then(toml::Value::as_table_mut) else {
		bail!("failed to access [desktop] table");
	};
	desktop.insert("active_profile_id".to_string(), toml::Value::String(active_profile_id.to_string()));
	desktop.insert(
		"profile_order".to_string(),
		toml::Value::Array(profile_order.iter().map(|id| toml::Value::String(id.clone())).collect()),
	);
	desktop.insert(
		"profile_ids".to_string(),
		toml::Value::Array(profile_order.iter().map(|id| toml::Value::String(id.clone())).collect()),
	);
	toml::to_string_pretty(&doc).context("failed to format conf.toml")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn unknown_creation_time_stays_stable_across_reads_and_first_edit() {
		for legacy in [false, true] {
			let root = tempfile::tempdir().unwrap();
			let store = CoreProfileDocumentStore::from_root(root.path());
			if legacy {
				fs::create_dir_all(store.profiles_dir()).unwrap();
				fs::write(store.profiles_dir().join("legacy.toml"), "id = \"default\"\nname = \"Legacy\"\n").unwrap();
			}
			let original = store.load().unwrap();
			std::thread::sleep(std::time::Duration::from_millis(1100));
			let reopened = CoreProfileDocumentStore::from_root(root.path());
			assert_eq!(reopened.load().unwrap(), original, "reading must not invent a new creation time");
			reopened.update(|document| {
				assert_eq!(document.profiles[0], original.profiles[0], "optimistic save sees the same profile");
				document.profiles[0].note = "first edit".into();
				Ok(())
			}).unwrap();
			assert_eq!(reopened.load().unwrap().profiles[0].note, "first edit");
		}
	}

	#[cfg(windows)]
	#[test]
	fn failed_startup_selection_save_does_not_commit_on_reopen() {
		use std::os::windows::fs::OpenOptionsExt;
		let root = tempfile::tempdir().unwrap();
		let store = CoreProfileDocumentStore::from_root(root.path());
		let mut initial = CoreProfileDocument::default();
		initial.profiles = vec![test_document_profile("a", "Alpha"), test_document_profile("b", "Beta")];
		initial.selected_profile_id = "a".into();
		store.save(initial).unwrap();
		let original = store.load().unwrap();
		let profile_bytes: Vec<_> = fs::read_dir(store.profiles_dir()).unwrap().map(|entry| {
			let path = entry.unwrap().path(); let bytes = fs::read(&path).unwrap(); (path, bytes)
		}).collect();
		let before = fs::read(store.conf_path()).unwrap();
		let held = OpenOptions::new().read(true).share_mode(1).open(store.conf_path()).unwrap();
		assert!(store.set_selected_profile_id("b").is_err());
		assert_eq!(fs::read(store.conf_path()).unwrap(), before);
		drop(held);
		let reopened = CoreProfileDocumentStore::from_root(root.path());
		assert_eq!(reopened.load().unwrap(), original, "A failed selection save must not silently commit during recovery");
		reopened.set_selected_profile_id("b").unwrap();
		let saved = CoreProfileDocumentStore::from_root(root.path()).load().unwrap();
		assert_eq!(saved.selected_profile_id, "b");
		assert_eq!(saved.profiles, original.profiles);
		assert_eq!(saved.profile_sources, original.profile_sources);
		for (path, bytes) in profile_bytes { assert_eq!(fs::read(path).unwrap(), bytes); }
		let committed = fs::read(store.conf_path()).unwrap();
		assert!(reopened.set_selected_profile_id("missing").is_err());
		assert_eq!(fs::read(store.conf_path()).unwrap(), committed);
	}

	#[test]
	#[ignore = "subprocess worker for concurrent_process_updates_preserve_all_edits"]
	fn profile_store_process_worker() {
		let root = std::env::var_os("UNM_TEST_PROFILE_STORE_ROOT").expect("isolated store root");
		let worker = std::env::var("UNM_TEST_PROFILE_STORE_WORKER").expect("worker id");
		let store = CoreProfileDocumentStore::from_root(PathBuf::from(root));
		for index in 0..12 {
			store.update(|doc| {
				doc.profiles[0].note.push_str(&format!("{worker}:{index};"));
				Ok(())
			}).expect("atomic process edit");
		}
	}

	#[test]
	fn concurrent_process_updates_preserve_all_edits() {
		let root = tempfile::tempdir().expect("isolated store");
		let store = CoreProfileDocumentStore::from_root(root.path());
		let mut initial = CoreProfileDocument::default();
		initial.profiles[0].note.clear();
		store.save(initial.clone()).expect("seed store");
		let executable = std::env::current_exe().expect("test executable");
		let mut children: Vec<_> = ["a", "b"].into_iter().map(|worker| {
			std::process::Command::new(&executable)
				.args(["--exact", "profile_document::tests::profile_store_process_worker", "--ignored"])
				.env("UNM_TEST_PROFILE_STORE_ROOT", root.path())
				.env("UNM_TEST_PROFILE_STORE_WORKER", worker)
				.stdout(std::process::Stdio::null())
				.spawn().expect("spawn isolated writer")
		}).collect();
		let statuses: Vec<_> = children.iter_mut().map(|child| child.wait().expect("wait writer")).collect();
		assert!(statuses.iter().all(|status| status.success()), "writer failed: {statuses:?}");
		let saved = store.load().expect("reload after both processes exit");
		let tokens: Vec<_> = saved.profiles[0].note.split(';').filter(|token| !token.is_empty()).collect();
		assert_eq!(tokens.len(), 24);
		for worker in ["a", "b"] {
			for index in 0..12 {
				assert_eq!(tokens.iter().filter(|token| **token == format!("{worker}:{index}")).count(), 1);
			}
		}
		let mut restored = saved;
		restored.profiles[0].note.clear();
		assert_eq!(restored, initial, "unrelated fields changed");
	}

	#[test]
	fn concurrent_updates_keep_both_independent_edits() {
		let root = tempfile::tempdir().unwrap();
		let store = CoreProfileDocumentStore::from_root(root.path());
		store.save(CoreProfileDocument::default()).unwrap();
		let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
		let workers: Vec<_> = [true, false].into_iter().map(|name| {
			let store = store.clone();
			let barrier = barrier.clone();
			std::thread::spawn(move || {
				barrier.wait();
				store.update(|doc| {
					if name { doc.profiles[0].name = "Renamed".into(); }
					else { doc.profiles[0].note = "Other editor note".into(); }
					Ok(())
				}).unwrap();
			})
		}).collect();
		for worker in workers { worker.join().unwrap(); }
		let saved = store.load().unwrap();
		assert_eq!(saved.profiles[0].name, "Renamed");
		assert_eq!(saved.profiles[0].note, "Other editor note");
	}

	#[test]
	fn rejected_update_does_not_publish_partial_edits() {
		let root = tempfile::tempdir().unwrap();
		let store = CoreProfileDocumentStore::from_root(root.path());
		store.save(CoreProfileDocument::default()).unwrap();
		let before = store.load().unwrap();
		assert!(store.update(|doc| {
			doc.profiles[0].name = "Must not persist".into();
			bail!("invalid later field")
		}).is_err());
		assert_eq!(store.load().unwrap(), before);
		assert!(!root.path().join(PROFILE_TRANSACTION_MARKER).exists());
	}

	#[test]
	fn unchanged_update_does_not_rewrite_metadata() {
		let root = tempfile::tempdir().unwrap();
		let store = CoreProfileDocumentStore::from_root(root.path());
		store.save(CoreProfileDocument::default()).unwrap();
		let path = store.conf_path();
		let text = format!("# keep unchanged metadata bytes\n{}", fs::read_to_string(path).unwrap());
		fs::write(path, &text).unwrap();
		store.update(|_| Ok(())).unwrap();
		assert_eq!(fs::read_to_string(path).unwrap(), text);
		assert!(!root.path().join(PROFILE_TRANSACTION_MARKER).exists());
	}

	#[test]
	fn atomic_write_replaces_existing_file_without_leaving_temporary_files() {
		let root = temp_root("atomic-write");
		let path = root.join("settings.toml");
		fs::write(&path, b"old").expect("seed destination");

		atomic_write(&path, b"new").expect("atomic replace");

		assert_eq!(fs::read(&path).expect("replacement"), b"new");
		assert_eq!(fs::read_dir(&root).expect("root entries").count(), 2);
	}

	#[test]
	fn store_keeps_profiles_with_colliding_timestamp_and_name() {
		let root = temp_root("profile-filename-collision");
		let store = CoreProfileDocumentStore::from_root(&root);
		let mut profiles = default_profiles();
		let mut duplicate = profiles[0].clone();
		profiles[0].id = "profile-a".to_string();
		profiles[0].name = "Same Name".to_string();
		profiles[0].created_at = "20260712T120000Z".to_string();
		duplicate.id = "profile-b".to_string();
		duplicate.name = profiles[0].name.clone();
		duplicate.created_at = profiles[0].created_at.clone();
		profiles = vec![profiles.remove(0), duplicate];

		store
			.save(CoreProfileDocument {
				selected_profile_id: "profile-a".to_string(),
				profiles,
				..CoreProfileDocument::default()
			})
			.expect("save colliding profiles");

		let loaded = store.load().expect("load profiles");
		assert_eq!(loaded.profiles.len(), 2);
		assert!(loaded.profiles.iter().any(|profile| profile.id == "profile-a"));
		assert!(loaded.profiles.iter().any(|profile| profile.id == "profile-b"));
		assert_eq!(fs::read_dir(root.join("profiles")).expect("profiles").count(), 2);
		let _ = fs::remove_dir_all(root);
	}

	fn temp_root(label: &str) -> PathBuf {
		let root = std::env::temp_dir().join(format!("un-motion-core-{label}-{}-{}", std::process::id(), now_millis()));
		fs::create_dir_all(root.join("profiles")).expect("temp profiles");
		root
	}

	fn now_millis() -> u128 {
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map(|duration| duration.as_millis())
			.unwrap_or_default()
	}

	#[test]
	fn store_loads_existing_desktop_profile_files() {
		let root = temp_root("load-profile-document");
		fs::write(root.join("conf.toml"), "[desktop]\nactive_profile_id = \"waidayo\"\n").expect("conf");
		fs::write(
			root.join("profiles/waidayo.toml"),
			r#"
id = "waidayo"
name = "Waidayo"
note = "Sub Send Motion"
defaultSourceEnabled = false
defaultSourceLabel = "UNMotion Default"

[[sources]]
id = "source-1"
kind = "vmc-osc"
label = "Waidayo VMC"
host = "192.0.2.20"
port = 39540
mirrorCorrectionEnabled = true
"#,
		)
		.expect("profile");

		let document = CoreProfileDocumentStore::from_root(&root).load().expect("load profile document");

		assert_eq!(document.selected_profile_id, "waidayo");
		assert_eq!(document.profiles[0].name, "Waidayo");
		assert_eq!(document.profile_sources[0].host.as_deref(), Some("192.0.2.20"));
		assert_eq!(document.profile_sources[0].port, Some(39540));
		assert!(document.profile_sources[0].mirror_correction_enabled);
	}

	#[test]
	fn store_saves_document_and_active_profile() {
		let root = temp_root("save-profile-document");
		let store = CoreProfileDocumentStore::from_root(&root);
		let saved = store
			.save(CoreProfileDocument {
				selected_profile_id: "p2".to_string(),
				profiles: vec![CoreProfileDocumentProfile {
					id: "p2".to_string(),
					name: "Profile 2".to_string(),
					created_at: String::new(),
					note: String::new(),
					icon_path: None,
					group: String::new(),
					default_source_enabled: false,
					default_source_label: default_source_label(),
					runtime_selection: None,
					pipeline_components: None,
				}],
				profile_sources: vec![CoreProfileDocumentSource {
					id: "vmc".to_string(),
					profile_id: "p2".to_string(),
					kind: "vmc-osc".to_string(),
					label: "VMC".to_string(),
					protocol: None,
					host: None,
					port: None,
					mirror_correction_enabled: false,
				}],
				next_profile_index: 2,
				next_source_index: 2,
			})
			.expect("save");

		assert_eq!(saved.profile_sources[0].host.as_deref(), Some("0.0.0.0"));
		assert!(
			fs::read_to_string(root.join("conf.toml"))
				.expect("conf")
				.contains("active_profile_id = \"p2\"")
		);
		let profile_text = fs::read_to_string(profile_file_for_id(&root, "p2")).expect("profile");
		assert!(profile_text.contains("host = \"0.0.0.0\""));
		let stale = profile_text
			.replace("id = \"p2\"", "id = \"stale\"")
			.replace("name = \"Profile 2\"", "name = \"Stale\"");
		fs::write(root.join("profiles/stale.toml"), stale).expect("simulate stale profile after metadata commit");
		let reloaded = store.load().expect("reload profile document");
		assert_eq!(
			reloaded.profiles.iter().map(|profile| profile.id.as_str()).collect::<Vec<_>>(),
			vec!["p2"]
		);
	}

	#[test]
	fn store_recovers_interrupted_profile_directory_swap() {
		let root = temp_root("recover-profile-transaction");
		let store = CoreProfileDocumentStore::from_root(&root);
		let previous = root.join(PREVIOUS_PROFILES_DIR);
		let next = root.join(NEXT_PROFILES_DIR);
		fs::remove_dir(root.join("profiles")).expect("simulate profiles moved aside");
		fs::create_dir_all(&previous).expect("previous profiles");
		fs::create_dir_all(&next).expect("next profiles");
		fs::write(previous.join("old.toml"), "id = \"old\"\nname = \"Old\"\ncreated_at = \"1\"\n").expect("old profile");
		fs::write(next.join("new.toml"), "id = \"new\"\nname = \"New\"\ncreated_at = \"2\"\n").expect("new profile");
		fs::write(
			root.join(NEXT_CONF_FILE),
			"[desktop]\nactive_profile_id = \"new\"\nprofile_ids = [\"new\"]\n",
		)
		.expect("next conf");
		fs::write(root.join(PROFILE_TRANSACTION_MARKER), "prepared\n").expect("marker");

		let loaded = store.load().expect("recover profile document");

		assert_eq!(loaded.selected_profile_id, "new");
		assert_eq!(
			loaded.profiles.iter().map(|profile| profile.id.as_str()).collect::<Vec<_>>(),
			vec!["new"]
		);
		assert!(!previous.exists());
		assert!(!next.exists());
		assert!(!root.join(NEXT_CONF_FILE).exists());
		assert!(!root.join(PROFILE_TRANSACTION_MARKER).exists());
	}

	#[test]
	fn interrupted_save_boundaries_recover_a_consistent_document() {
		for stage in 0..=5 {
			let root = temp_root(&format!("save-boundary-{stage}"));
			let old_conf = "[desktop]\nactive_profile_id = \"old\"\nprofile_ids = [\"old\"]\n";
			let new_conf = "[desktop]\nactive_profile_id = \"new\"\nprofile_ids = [\"new\"]\n";
			fs::write(root.join("conf.toml"), old_conf).unwrap();
			fs::write(root.join("profiles/old.toml"), "id=\"old\"\nname=\"Old\"\ncreated_at=\"1\"\n").unwrap();
			fs::create_dir(root.join(NEXT_PROFILES_DIR)).unwrap();
			fs::write(root.join(NEXT_PROFILES_DIR).join("new.toml"), "id=\"new\"\nname=\"New\"\ncreated_at=\"2\"\n").unwrap();
			fs::write(root.join(NEXT_CONF_FILE), new_conf).unwrap();
			fs::write(root.join(PROFILE_TRANSACTION_MARKER), "prepared\n").unwrap();
			if stage >= 1 { fs::rename(root.join("profiles"), root.join(PREVIOUS_PROFILES_DIR)).unwrap(); }
			if stage >= 2 { fs::rename(root.join(NEXT_PROFILES_DIR), root.join("profiles")).unwrap(); }
			if stage >= 3 { fs::write(root.join("conf.toml"), new_conf).unwrap(); }
			if stage >= 4 { fs::remove_dir_all(root.join(PREVIOUS_PROFILES_DIR)).unwrap(); }
			if stage >= 5 { fs::remove_file(root.join(NEXT_CONF_FILE)).unwrap(); }
			let expected = if stage == 0 { "old" } else { "new" };
			let store = CoreProfileDocumentStore::from_root(&root);
			for _ in 0..2 {
				let loaded = store.load().unwrap();
				assert_eq!(loaded.selected_profile_id, expected, "stage {stage}");
				assert_eq!(loaded.profiles.len(), 1, "stage {stage}");
				assert_eq!(loaded.profiles[0].id, expected, "stage {stage}");
				assert_eq!(fs::read_to_string(root.join("conf.toml")).unwrap(), if stage == 0 { old_conf } else { new_conf });
			}
			for name in [NEXT_PROFILES_DIR, PREVIOUS_PROFILES_DIR, NEXT_CONF_FILE, PROFILE_TRANSACTION_MARKER] {
				assert!(!root.join(name).exists(), "stage {stage}: leftover {name}");
			}
		}
	}

	#[test]
	fn store_recovery_to_previous_profiles_keeps_previous_metadata() {
		let root = temp_root("recover-previous-without-next");
		let old_conf = "[desktop]\nactive_profile_id = \"old\"\nprofile_ids = [\"old\"]\n";
		fs::write(root.join("conf.toml"), old_conf).unwrap();
		fs::write(root.join("profiles/old.toml"), "id = \"old\"\nname = \"Old\"\ncreated_at = \"1\"\n").unwrap();
		fs::rename(root.join("profiles"), root.join(PREVIOUS_PROFILES_DIR)).unwrap();
		fs::write(root.join(NEXT_CONF_FILE), "[desktop]\nactive_profile_id = \"new\"\nprofile_ids = [\"new\"]\n").unwrap();
		fs::write(root.join(PROFILE_TRANSACTION_MARKER), "prepared\n").unwrap();
		let loaded = CoreProfileDocumentStore::from_root(&root).load().unwrap();
		assert_eq!(fs::read_to_string(root.join("conf.toml")).unwrap(), old_conf);
		assert_eq!(loaded.selected_profile_id, "old");
		assert_eq!(loaded.profiles.len(), 1);
		assert_eq!(loaded.profiles[0].id, "old");
	}

	#[test]
	fn store_load_waits_for_process_wide_file_lock() {
		let root = temp_root("profile-store-lock");
		let store = CoreProfileDocumentStore::from_root(&root);
		let held_lock = store.acquire_store_lock().expect("hold store lock");
		let (tx, rx) = std::sync::mpsc::channel();
		let load_root = root.clone();
		let join = std::thread::spawn(move || {
			let result = CoreProfileDocumentStore::from_root(load_root).load();
			tx.send(result).expect("send load result");
		});

		assert!(rx.recv_timeout(std::time::Duration::from_millis(30)).is_err());
		drop(held_lock);
		assert!(
			rx.recv_timeout(std::time::Duration::from_secs(1))
				.expect("load after unlock")
				.is_ok()
		);
		join.join().expect("load thread");
	}

	#[test]
	fn store_load_propagates_recovery_errors() {
		let root = temp_root("profile-recovery-error");
		fs::write(root.join(PROFILE_TRANSACTION_MARKER), "prepared\n").expect("marker");
		fs::create_dir(root.join(NEXT_CONF_FILE)).expect("invalid staged conf directory");

		let error = CoreProfileDocumentStore::from_root(&root).load().expect_err("recovery must fail");

		assert!(error.to_string().contains("failed to read staged conf"));
	}

	#[test]
	fn store_load_rejects_malformed_profile_and_conf_files() {
		let profile_root = temp_root("malformed-profile");
		fs::write(profile_root.join("profiles/broken.toml"), "id = [").expect("broken profile");
		let profile_error = CoreProfileDocumentStore::from_root(&profile_root)
			.load()
			.expect_err("malformed profile must fail");
		assert!(profile_error.to_string().contains("failed to parse profile"));

		let conf_root = temp_root("malformed-conf");
		fs::write(conf_root.join("conf.toml"), "[desktop").expect("broken conf");
		let conf_error = CoreProfileDocumentStore::from_root(&conf_root)
			.load()
			.expect_err("malformed conf must fail");
		assert!(conf_error.to_string().contains("failed to parse"));
	}

	#[test]
	fn store_rejects_oversized_profile_files() {
		let root = temp_root("oversized-profile");
		let path = root.join("profiles/oversized.toml");
		File::create(&path)
			.expect("create oversized profile")
			.set_len(MAX_PROFILE_STORE_FILE_BYTES + 1)
			.expect("extend oversized profile");

		let error = CoreProfileDocumentStore::from_root(&root)
			.load()
			.expect_err("oversized profile must fail");

		assert!(error.to_string().contains("file size limit"), "{error:#}");
		let _ = fs::remove_dir_all(root);
	}

	#[test]
	fn store_rejects_too_many_profile_files() {
		let root = temp_root("too-many-profiles");
		for index in 0..=MAX_PROFILE_FILES {
			fs::write(
				root.join("profiles").join(format!("profile-{index}.toml")),
				format!("id = \"profile-{index}\"\nname = \"Profile {index}\"\n"),
			)
			.expect("write profile");
		}

		let error = CoreProfileDocumentStore::from_root(&root)
			.load()
			.expect_err("excess profile files must fail");

		assert!(error.to_string().contains("more than 256 profile files"), "{error:#}");
		let _ = fs::remove_dir_all(root);
	}

	#[test]
	fn store_rejects_oversized_documents_before_save() {
		let root = temp_root("oversized-document");
		let store = CoreProfileDocumentStore::from_root(&root);
		let profile = default_profiles().remove(0);
		let too_many_profiles = CoreProfileDocument {
			profiles: (0..=MAX_PROFILE_FILES)
				.map(|index| CoreProfileDocumentProfile {
					id: format!("profile-{index}"),
					..profile.clone()
				})
				.collect(),
			..CoreProfileDocument::default()
		};
		assert!(
			store
				.save(too_many_profiles)
				.expect_err("profile limit")
				.to_string()
				.contains("more than 256 profiles")
		);

		let mut too_many_sources = CoreProfileDocument::default();
		let profile_id = too_many_sources.profiles[0].id.clone();
		too_many_sources.profile_sources = (0..=MAX_PROFILE_SOURCES)
			.map(|index| CoreProfileDocumentSource {
				id: format!("source-{index}"),
				profile_id: profile_id.clone(),
				kind: "vmc-osc".to_string(),
				label: "Source".to_string(),
				protocol: None,
				host: None,
				port: None,
				mirror_correction_enabled: false,
			})
			.collect();
		assert!(
			store
				.save(too_many_sources)
				.expect_err("source limit")
				.to_string()
				.contains("more than 4096 sources")
		);
		let _ = fs::remove_dir_all(root);
	}

	#[test]
	fn normalize_allows_empty_profile_document() {
		let normalized = normalize_profile_document(CoreProfileDocument {
			selected_profile_id: "missing".to_string(),
			profiles: Vec::new(),
			profile_sources: vec![CoreProfileDocumentSource {
				id: "source".to_string(),
				profile_id: "missing".to_string(),
				kind: "vmc-osc".to_string(),
				label: "orphan".to_string(),
				protocol: None,
				host: None,
				port: None,
				mirror_correction_enabled: true,
			}],
			next_profile_index: 0,
			next_source_index: 0,
		});

		assert!(normalized.profiles.is_empty());
		assert!(normalized.profile_sources.is_empty());
		assert_eq!(normalized.selected_profile_id, "");
		assert_eq!(normalized.next_profile_index, default_next_profile_index());
		assert_eq!(normalized.next_source_index, default_next_source_index());
	}

	#[test]
	fn normalize_trims_and_deduplicates_profile_and_source_ids() {
		let source = |id: &str, profile_id: &str| CoreProfileDocumentSource {
			id: id.to_string(),
			profile_id: profile_id.to_string(),
			kind: "vmc-osc".to_string(),
			label: id.to_string(),
			protocol: None,
			host: None,
			port: None,
			mirror_correction_enabled: false,
		};
		let normalized = normalize_profile_document(CoreProfileDocument {
			selected_profile_id: " p1 ".to_string(),
			profiles: vec![test_document_profile(" p1 ", "first"), test_document_profile("p1", "duplicate")],
			profile_sources: vec![
				source(" source-1 ", " p1 "),
				source("source-1", "p1"),
				source("source-2", "missing"),
			],
			..CoreProfileDocument::default()
		});

		assert_eq!(normalized.profiles.len(), 1);
		assert_eq!(normalized.profiles[0].id, "p1");
		assert_eq!(normalized.profiles[0].name, "first");
		assert_eq!(normalized.profile_sources.len(), 1);
		assert_eq!(normalized.profile_sources[0].id, "source-1");
		assert_eq!(normalized.profile_sources[0].profile_id, "p1");
		assert_eq!(normalized.selected_profile_id, "p1");
	}

	#[test]
	fn store_saves_empty_profile_document_and_removes_stale_profiles() {
		let root = temp_root("save-empty-profile-document");
		let store = CoreProfileDocumentStore::from_root(&root);
		store
			.save(CoreProfileDocument {
				selected_profile_id: "p1".to_string(),
				profiles: vec![test_document_profile("p1", "Profile 1")],
				profile_sources: Vec::new(),
				next_profile_index: 2,
				next_source_index: 2,
			})
			.expect("save initial");
		assert!(profile_file_for_id(&root, "p1").is_file());

		let saved = store
			.save(CoreProfileDocument {
				selected_profile_id: "p1".to_string(),
				profiles: Vec::new(),
				profile_sources: Vec::new(),
				next_profile_index: 2,
				next_source_index: 2,
			})
			.expect("save empty");

		assert!(saved.profiles.is_empty());
		assert_eq!(saved.selected_profile_id, "");
		assert_eq!(fs::read_dir(root.join("profiles")).unwrap().count(), 0);
		assert!(
			fs::read_to_string(root.join("conf.toml"))
				.expect("conf")
				.contains("active_profile_id = \"\"")
		);
		let loaded = store.load().expect("load empty profile document");
		assert!(loaded.profiles.is_empty());
		assert_eq!(loaded.selected_profile_id, "");
	}

	#[test]
	fn store_roundtrips_profile_order() {
		let root = temp_root("profile-order");
		let store = CoreProfileDocumentStore::from_root(&root);
		store
			.save(CoreProfileDocument {
				selected_profile_id: "third".to_string(),
				profiles: vec![
					test_document_profile("third", "Third"),
					test_document_profile("first", "First"),
					test_document_profile("second", "Second"),
				],
				..CoreProfileDocument::default()
			})
			.expect("save");

		let loaded = store.load().expect("load ordered profiles");

		assert_eq!(
			loaded.profiles.iter().map(|profile| profile.id.as_str()).collect::<Vec<_>>(),
			vec!["third", "first", "second"]
		);
		let conf = fs::read_to_string(root.join("conf.toml"))
			.expect("conf")
			.parse::<toml::Value>()
			.expect("conf toml");
		assert_eq!(
			profile_order_from_document(&conf),
			vec!["third".to_string(), "first".to_string(), "second".to_string()]
		);
		assert!(conf.get("desktop").and_then(|desktop| desktop.get("profile_order")).is_some());
	}

	#[test]
	fn store_roundtrips_nested_runtime_selection() {
		let root = temp_root("roundtrip-runtime-selection");
		fs::write(
			root.join("profiles/default.toml"),
			r#"
id = "default"
name = "Default"
note = "UNMotion Default"

[runtimeSelection]
fps = 90
engine = "mediapipe-native"

[runtimeSelection.modifier]
headEnabled = true
"#,
		)
		.expect("profile");
		let store = CoreProfileDocumentStore::from_root(&root);
		let document = store.load().expect("load nested runtime selection");

		assert_eq!(
			document.profiles[0].runtime_selection.as_ref().and_then(|settings| settings.fps),
			Some(90)
		);
		assert_eq!(
			document.profiles[0]
				.runtime_selection
				.as_ref()
				.and_then(|settings| settings.modifier.as_ref())
				.and_then(|modifier| modifier.head_enabled),
			Some(true)
		);

		store.save(document).expect("save");
		let saved = fs::read_to_string(profile_file_for_id(&root, "default")).expect("saved");

		assert!(saved.contains("[runtimeSelection]"));
		assert!(saved.contains("[runtimeSelection.modifier]"));
		assert!(saved.contains("headEnabled = true"));
	}

	#[test]
	fn normalize_fills_missing_runtime_engine() {
		let normalized = normalize_profile_document(CoreProfileDocument {
			selected_profile_id: "default".to_string(),
			profiles: vec![CoreProfileDocumentProfile {
				id: "default".to_string(),
				name: "Default".to_string(),
				created_at: String::new(),
				note: String::new(),
				icon_path: None,
				group: String::new(),
				default_source_enabled: true,
				default_source_label: default_source_label(),
				runtime_selection: Some(ProfileRuntimeSettings {
					fps: Some(60),
					..ProfileRuntimeSettings::default()
				}),
				pipeline_components: None,
			}],
			profile_sources: Vec::new(),
			next_profile_index: 0,
			next_source_index: 0,
		});

		assert_eq!(
			normalized.profiles[0]
				.runtime_selection
				.as_ref()
				.and_then(|runtime| runtime.engine.as_deref()),
			Some("mediapipe-native")
		);
	}

	#[test]
	fn normalize_migrates_legacy_pipeline_engine() {
		let normalized = normalize_profile_document(CoreProfileDocument {
			selected_profile_id: "waidayo".to_string(),
			profiles: vec![CoreProfileDocumentProfile {
				id: "waidayo".to_string(),
				name: "Waidayo".to_string(),
				created_at: String::new(),
				note: String::new(),
				icon_path: None,
				group: String::new(),
				default_source_enabled: true,
				default_source_label: default_source_label(),
				runtime_selection: None,
				pipeline_components: Some(ProfilePipelineComponents {
					engine: Some("vmc".to_string()),
					..ProfilePipelineComponents::default()
				}),
			}],
			profile_sources: Vec::new(),
			next_profile_index: 0,
			next_source_index: 0,
		});

		assert_eq!(
			normalized.profiles[0]
				.runtime_selection
				.as_ref()
				.and_then(|runtime| runtime.engine.as_deref()),
			Some("vmc")
		);
	}

	/// Phase E "Seed 廃止 + bundled templates + 初回コピー": user store が未初期化の
	/// ときに限り template dir の `*.toml` を user dir/profiles/ にコピーする。
	#[test]
	fn seed_from_templates_copies_into_empty_user_dir() {
		let user_root = temp_root("seed-empty-user");
		// `temp_root` は profiles/ を作るが中身は空。これが「初回起動」。
		assert!(user_root.join("profiles").is_dir());
		assert_eq!(fs::read_dir(user_root.join("profiles")).unwrap().count(), 0);

		let template_dir = temp_root("seed-templates");
		fs::write(
			template_dir.join("profiles/default.toml"),
			r#"
id = "default"
name = "Default"
note = "bundled"
"#,
		)
		.expect("write template default");
		fs::write(
			template_dir.join("profiles/waidayo.toml"),
			r#"
id = "waidayo"
name = "Waidayo"
note = "bundled"
"#,
		)
		.expect("write template waidayo");

		let store = CoreProfileDocumentStore::from_user_dir(&user_root);
		let copied = store
			.seed_from_templates(template_dir.join("profiles").as_path())
			.expect("seed_from_templates");
		assert!(copied, "seed should report at least one file copied on first run");
		assert!(user_root.join("profiles/default.toml").is_file());
		assert!(user_root.join("profiles/waidayo.toml").is_file());

		// 2 回目は何もしない (ユーザーが削除した profile を勝手に復活させない)。
		store
			.save(CoreProfileDocument {
				selected_profile_id: String::new(),
				profiles: Vec::new(),
				profile_sources: Vec::new(),
				next_profile_index: 3,
				next_source_index: 2,
			})
			.expect("save empty document");
		let copied_again = store
			.seed_from_templates(template_dir.join("profiles").as_path())
			.expect("seed_from_templates idempotent");
		assert!(!copied_again, "2nd seed must be a no-op after user removed all profiles");
		assert!(!user_root.join("profiles/default.toml").exists(), "user removal must persist");
		assert!(!user_root.join("profiles/waidayo.toml").exists(), "user removal must persist");
	}

	/// template dir が存在しない場合は (release build で同梱忘れ等) silent no-op。
	#[test]
	fn seed_from_templates_no_op_when_template_dir_missing() {
		let user_root = temp_root("seed-no-template");
		let store = CoreProfileDocumentStore::from_user_dir(&user_root);
		let missing = std::env::temp_dir().join(format!("un-motion-missing-tpl-{}", std::process::id()));
		assert!(!missing.exists());
		let copied = store.seed_from_templates(&missing).expect("seed no-op");
		assert!(!copied);
	}

	#[test]
	fn seed_from_templates_rejects_invalid_templates_before_publish() {
		let user_root = temp_root("seed-invalid-user");
		let template_root = temp_root("seed-invalid-templates");
		let template_dir = template_root.join("profiles");
		fs::write(template_dir.join("valid.toml"), "id = \"valid\"\nname = \"Valid\"\n").expect("write valid template");
		fs::write(template_dir.join("invalid.toml"), "id = [\n").expect("write invalid template");
		let store = CoreProfileDocumentStore::from_user_dir(&user_root);

		let error = store.seed_from_templates(&template_dir).expect_err("invalid template must fail");

		assert!(error.to_string().contains("failed to parse profile template"));
		assert_eq!(fs::read_dir(user_root.join("profiles")).expect("user profiles").count(), 0);
		assert!(!user_root.join(PROFILE_TRANSACTION_MARKER).exists());
	}

	#[test]
	fn seed_from_templates_rejects_duplicate_profile_and_source_ids() {
		for (case, second) in [
			("profile", "id = \"same\"\nname = \"Second\"\n"),
			(
				"source",
				"id = \"second\"\nname = \"Second\"\n[[sources]]\nid = \"same-source\"\nkind = \"vmc-osc\"\nlabel = \"B\"\n",
			),
		] {
			let user_root = temp_root(&format!("seed-duplicate-{case}-user"));
			let template_root = temp_root(&format!("seed-duplicate-{case}-templates"));
			let template_dir = template_root.join("profiles");
			fs::write(
				template_dir.join("first.toml"),
				"id = \"same\"\nname = \"First\"\n[[sources]]\nid = \"same-source\"\nkind = \"vmc-osc\"\nlabel = \"A\"\n",
			)
			.expect("write first template");
			fs::write(template_dir.join("second.toml"), second).expect("write second template");
			let store = CoreProfileDocumentStore::from_user_dir(&user_root);

			let error = store.seed_from_templates(&template_dir).expect_err("duplicate id must fail");

			assert!(error.to_string().contains("duplicate"), "{error:#}");
			assert_eq!(fs::read_dir(user_root.join("profiles")).expect("user profiles").count(), 0);
		}
	}

	fn test_document_profile(id: &str, name: &str) -> CoreProfileDocumentProfile {
		CoreProfileDocumentProfile {
			id: id.to_string(),
			name: name.to_string(),
			created_at: String::new(),
			note: String::new(),
			icon_path: None,
			group: String::new(),
			default_source_enabled: true,
			default_source_label: default_source_label(),
			runtime_selection: None,
			pipeline_components: None,
		}
	}

	fn profile_file_for_id(root: &Path, id: &str) -> PathBuf {
		for entry in fs::read_dir(root.join("profiles")).expect("profiles").flatten() {
			let path = entry.path();
			let raw = fs::read_to_string(&path).expect("profile raw");
			let value = raw.parse::<toml::Value>().expect("profile toml");
			if value.get("id").and_then(toml::Value::as_str) == Some(id) {
				return path;
			}
		}
		panic!("profile file not found: {id}");
	}
}
