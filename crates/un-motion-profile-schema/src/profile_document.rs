use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::CoreProfile;
use crate::profile_settings::{ProfilePipelineComponents, ProfileRuntimeSettings};

const CONF_FILE_NAME: &str = "conf.toml";

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
		if self.conf_path.exists() {
			return Ok(false);
		}
		if has_any_toml(&self.profiles_dir) {
			return Ok(false);
		}
		fs::create_dir_all(&self.profiles_dir)
			.with_context(|| format!("failed to create user profiles dir {}", self.profiles_dir.display()))?;
		let mut copied = false;
		for entry in fs::read_dir(template_dir)
			.with_context(|| format!("failed to read template dir {}", template_dir.display()))?
			.flatten()
		{
			let path = entry.path();
			if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
				continue;
			}
			let Some(file_name) = path.file_name() else { continue };
			let dest = self.profiles_dir.join(file_name);
			fs::copy(&path, &dest).with_context(|| format!("failed to copy {} -> {}", path.display(), dest.display()))?;
			copied = true;
		}
		Ok(copied)
	}

	pub fn load(&self) -> CoreProfileDocument {
		let mut profiles = Vec::new();
		let mut profile_sources = Vec::new();
		if let Ok(entries) = fs::read_dir(&self.profiles_dir) {
			for entry in entries.flatten() {
				let path = entry.path();
				if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
					continue;
				}
				let Ok(raw) = fs::read_to_string(&path) else {
					continue;
				};
				let Ok(profile) = toml::from_str::<CoreProfileFile>(&raw) else {
					continue;
				};
				for source in profile.sources {
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
		sort_profiles_by_saved_order(&mut profiles, &load_profile_order(&self.conf_path));
		if profiles.is_empty() && !self.conf_path.exists() {
			profiles = default_profiles();
		}
		normalize_profile_document(CoreProfileDocument {
			next_profile_index: (profiles.len() as u32).saturating_add(1).max(default_next_profile_index()),
			next_source_index: (profile_sources.len() as u32).saturating_add(2).max(default_next_source_index()),
			selected_profile_id: load_active_profile_id(&self.conf_path).unwrap_or_else(default_selected_profile_id),
			profiles,
			profile_sources,
		})
	}

	pub fn save(&self, document: CoreProfileDocument) -> anyhow::Result<CoreProfileDocument> {
		let normalized = normalize_profile_document(document);
		fs::create_dir_all(&self.profiles_dir)
			.with_context(|| format!("failed to create profiles directory {}", self.profiles_dir.display()))?;
		let expected_files = normalized.profiles.iter().map(profile_file_name).collect::<HashSet<_>>();

		if let Ok(entries) = fs::read_dir(&self.profiles_dir) {
			for entry in entries.flatten() {
				let path = entry.path();
				if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
					continue;
				}
				let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
					continue;
				};
				if !expected_files.contains(file_name) {
					fs::remove_file(&path).with_context(|| format!("failed to remove stale profile {}", path.display()))?;
				}
			}
		}

		for profile in &normalized.profiles {
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
			let path = self.profiles_dir.join(profile_file_name(profile));
			fs::write(&path, raw).with_context(|| format!("failed to write profile {}", path.display()))?;
		}

		save_profile_metadata(
			&self.conf_path,
			&normalized.selected_profile_id,
			&normalized.profiles.iter().map(|profile| profile.id.clone()).collect::<Vec<_>>(),
		)?;
		Ok(normalized)
	}
}

pub fn normalize_profile_document(mut document: CoreProfileDocument) -> CoreProfileDocument {
	document.profiles.retain(|profile| !profile.id.trim().is_empty());
	let profile_ids = document.profiles.iter().map(|profile| profile.id.clone()).collect::<HashSet<_>>();
	document
		.profile_sources
		.retain(|source| !source.id.trim().is_empty() && profile_ids.contains(&source.profile_id));
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
	}
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

fn default_profiles() -> Vec<CoreProfileDocumentProfile> {
	vec![CoreProfileDocumentProfile {
		id: default_selected_profile_id(),
		name: "Default".to_string(),
		created_at: current_timestamp_compact(),
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
	current_timestamp_compact()
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

fn load_active_profile_id(path: &Path) -> Option<String> {
	let raw = fs::read_to_string(path).ok()?;
	let doc = raw.parse::<toml::Value>().ok()?;
	doc.get("desktop")?.get("active_profile_id")?.as_str().map(ToString::to_string)
}

fn load_profile_order(path: &Path) -> Vec<String> {
	let Ok(raw) = fs::read_to_string(path) else {
		return Vec::new();
	};
	let Ok(doc) = raw.parse::<toml::Value>() else {
		return Vec::new();
	};
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

fn save_profile_metadata(path: &Path, active_profile_id: &str, profile_order: &[String]) -> anyhow::Result<()> {
	let mut doc = match fs::read_to_string(path) {
		Ok(raw) => raw
			.parse::<toml::Value>()
			.unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new())),
		Err(_) => toml::Value::Table(toml::map::Map::new()),
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
	let raw = toml::to_string_pretty(&doc).context("failed to format conf.toml")?;
	fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
	use super::*;

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
host = "192.168.13.13"
port = 39540
mirrorCorrectionEnabled = true
"#,
		)
		.expect("profile");

		let document = CoreProfileDocumentStore::from_root(&root).load();

		assert_eq!(document.selected_profile_id, "waidayo");
		assert_eq!(document.profiles[0].name, "Waidayo");
		assert_eq!(document.profile_sources[0].host.as_deref(), Some("192.168.13.13"));
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
		let loaded = store.load();
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

		let loaded = store.load();

		assert_eq!(
			loaded.profiles.iter().map(|profile| profile.id.as_str()).collect::<Vec<_>>(),
			vec!["third", "first", "second"]
		);
		let conf = fs::read_to_string(root.join("conf.toml"))
			.expect("conf")
			.parse::<toml::Value>()
			.expect("conf toml");
		assert_eq!(
			load_profile_order(root.join("conf.toml").as_path()),
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
		let document = store.load();

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
