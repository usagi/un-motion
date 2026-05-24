//! UN Motion 共通 Profile schema (TOML I/O + 強い型表現)。
//!
//! Phase D の re-architecture で `un-motion-core` から切り出した薄い crate。
//! Capturer / Core / Supervisor の各バイナリが共通で参照する Profile 型と
//! TOML 読み書きを提供する。actix-web / engines / runtime / zenoh など
//! 重い依存を引き込まないので、GUI 側 (Supervisor) からこの crate のみを
//! 依存することで Supervisor exe を軽量に保つ。
//!
//! 構成:
//! * `CoreProfile`: HTTP API レスポンス / GUI 表示で使う profile の summary
//!   (id / name / note / engine type 縮約)。
//! * `profile_document` module: `CoreProfileDocument` (TOML 全体) と
//!   `CoreProfileDocumentStore` (workspace 配下 `conf.toml` + `profiles/*.toml`
//!   の I/O)。
//! * `profile_settings` module: `ProfileRuntimeSettings` /
//!   `ProfileModifierSettings` / `ProfileMediaPipeAdvancedSettings` /
//!   `ProfilePipelineComponents` の strong-typed 設定構造体。

pub mod profile_document;
pub mod profile_settings;

pub use profile_document::{
	CoreProfileDocument, CoreProfileDocumentProfile, CoreProfileDocumentSource, CoreProfileDocumentStore, document_from_profiles,
	document_profiles, normalize_profile_document, profile_engine_summary, resolve_workspace_conf_path,
};
pub use profile_settings::{ProfileMediaPipeAdvancedSettings, ProfileModifierSettings, ProfilePipelineComponents, ProfileRuntimeSettings};

use serde::{Deserialize, Serialize};

/// HTTP API のレスポンス / GUI 表示で使う profile の summary。
///
/// 旧来は `un-motion-core::control::CoreProfile` として定義されていたが、
/// Supervisor 側が core 全体を依存する原因になっていたので Phase D で
/// schema crate に移した。`un-motion-core` からは pub use 再エクスポートされる。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CoreProfile {
	pub id: String,
	pub name: String,
	#[serde(default)]
	pub note: String,
	#[serde(default)]
	pub icon_path: Option<String>,
	#[serde(default)]
	pub group: String,
	/// `runtime_selection.engine` の一覧表示用 summary。
	#[serde(default)]
	pub engine: Option<String>,
}

impl CoreProfile {
	pub fn default_profile() -> Self {
		Self {
			id: "default".to_string(),
			name: "Default".to_string(),
			note: String::new(),
			icon_path: None,
			group: String::new(),
			engine: None,
		}
	}
}
