use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use un_motion_pipeline::PipelinePolicy;

const DEFAULT_STALE_TIMEOUT_NS: u64 = i64::MAX as u64;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct RunnerProfile {
	pub pipeline: PipelinePolicyConfig,
	pub components: PipelineComponentsConfig,
	pub output: OutputConfig,
}

impl Default for RunnerProfile {
	fn default() -> Self {
		Self {
			pipeline: PipelinePolicyConfig::default(),
			components: PipelineComponentsConfig::default(),
			output: OutputConfig::default(),
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct PipelineComponentsConfig {
	pub input: InputComponentConfig,
	pub input_buffer: BufferComponentConfig,
	pub engine: EngineComponentConfig,
	pub post_process: PostProcessComponentConfig,
	pub output_buffer: BufferComponentConfig,
	pub outputs: Vec<OutputComponentConfig>,
}

impl Default for PipelineComponentsConfig {
	fn default() -> Self {
		Self {
			input: InputComponentConfig::default(),
			input_buffer: BufferComponentConfig::input_default(),
			engine: EngineComponentConfig::default(),
			post_process: PostProcessComponentConfig::default(),
			output_buffer: BufferComponentConfig::output_default(),
			outputs: vec![OutputComponentConfig::vmc_default()],
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComponentOption {
	pub id: String,
	pub label: String,
	pub available: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PipelineComponentCatalog {
	pub inputs: Vec<ComponentOption>,
	pub engines: Vec<ComponentOption>,
	pub post_processes: Vec<ComponentOption>,
	pub outputs: Vec<ComponentOption>,
}

impl PipelineComponentCatalog {
	pub fn current_platform() -> Self {
		Self {
			inputs: InputComponentKind::all()
				.iter()
				.map(|kind| ComponentOption {
					id: kind.id().to_string(),
					label: kind.label().to_string(),
					available: kind.available_on_current_platform(),
				})
				.collect(),
			engines: EngineComponentKind::all()
				.iter()
				.map(|kind| ComponentOption {
					id: kind.id().to_string(),
					label: kind.label().to_string(),
					available: true,
				})
				.collect(),
			post_processes: PostProcessComponentKind::all()
				.iter()
				.map(|kind| ComponentOption {
					id: kind.id().to_string(),
					label: kind.label().to_string(),
					available: true,
				})
				.collect(),
			outputs: OutputComponentKind::all()
				.iter()
				.map(|kind| ComponentOption {
					id: kind.id().to_string(),
					label: kind.label().to_string(),
					available: true,
				})
				.collect(),
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct InputComponentConfig {
	pub id: String,
	pub kind: InputComponentKind,
	pub settings: toml::value::Table,
}

impl Default for InputComponentConfig {
	fn default() -> Self {
		Self {
			id: "input".to_string(),
			kind: InputComponentKind::default(),
			settings: toml::value::Table::new(),
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InputComponentKind {
	WebcamDirectShow,
	WebcamNokhwa,
	WebcamBrowser,
	FileImage,
	FileVideo,
}

impl Default for InputComponentKind {
	fn default() -> Self {
		if cfg!(windows) {
			Self::WebcamDirectShow
		} else {
			Self::WebcamNokhwa
		}
	}
}

impl InputComponentKind {
	pub const fn all() -> &'static [Self] {
		&[
			Self::WebcamDirectShow,
			Self::WebcamNokhwa,
			Self::WebcamBrowser,
			Self::FileImage,
			Self::FileVideo,
		]
	}

	pub const fn id(&self) -> &'static str {
		match self {
			Self::WebcamDirectShow => "webcam-directshow",
			Self::WebcamNokhwa => "webcam-nokhwa",
			Self::WebcamBrowser => "webcam-browser",
			Self::FileImage => "file-image",
			Self::FileVideo => "file-video",
		}
	}

	pub const fn label(&self) -> &'static str {
		match self {
			Self::WebcamDirectShow => "Webcam DirectShow",
			Self::WebcamNokhwa => "Webcam nokhwa",
			Self::WebcamBrowser => "Webcam Browser",
			Self::FileImage => "File Image",
			Self::FileVideo => "File Video",
		}
	}

	pub const fn available_on_current_platform(&self) -> bool {
		match self {
			Self::WebcamDirectShow => cfg!(windows),
			Self::WebcamNokhwa => !cfg!(windows),
			Self::WebcamBrowser => true,
			Self::FileImage | Self::FileVideo => true,
		}
	}

	pub const fn is_realtime(&self) -> bool {
		matches!(self, Self::WebcamDirectShow | Self::WebcamNokhwa | Self::WebcamBrowser)
	}

	pub const fn prefers_deterministic_fifo(&self) -> bool {
		matches!(self, Self::FileVideo)
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct BufferComponentConfig {
	pub id: String,
	pub kind: BufferComponentKind,
	pub capacity: usize,
	pub overflow: BufferOverflowPolicy,
	pub settings: toml::value::Table,
}

impl BufferComponentConfig {
	pub fn input_default() -> Self {
		Self::latest_wins("input-buffer")
	}

	pub fn output_default() -> Self {
		Self::latest_wins("output-buffer")
	}

	pub fn automatic_input_for(input: &InputComponentKind) -> Self {
		if input.prefers_deterministic_fifo() {
			Self::deterministic_fifo("input-buffer")
		} else {
			Self::latest_wins("input-buffer")
		}
	}

	pub fn automatic_output_for(input: &InputComponentKind) -> Self {
		if input.prefers_deterministic_fifo() {
			Self::deterministic_fifo("output-buffer")
		} else {
			Self::latest_wins("output-buffer")
		}
	}

	fn latest_wins(id: &str) -> Self {
		Self {
			id: id.to_string(),
			kind: BufferComponentKind::Latest,
			capacity: 1,
			overflow: BufferOverflowPolicy::ReplaceOld,
			settings: toml::value::Table::new(),
		}
	}

	fn deterministic_fifo(id: &str) -> Self {
		Self {
			id: id.to_string(),
			kind: BufferComponentKind::Ring,
			capacity: 8,
			overflow: BufferOverflowPolicy::BlockProducer,
			settings: toml::value::Table::new(),
		}
	}
}

impl Default for BufferComponentConfig {
	fn default() -> Self {
		Self::input_default()
	}
}

impl PipelineComponentsConfig {
	pub fn apply_automatic_buffer_strategy(&mut self) {
		let mut input_buffer = BufferComponentConfig::automatic_input_for(&self.input.kind);
		input_buffer.settings = self.input_buffer.settings.clone();
		let mut output_buffer = BufferComponentConfig::automatic_output_for(&self.input.kind);
		output_buffer.settings = self.output_buffer.settings.clone();
		self.input_buffer = input_buffer;
		self.output_buffer = output_buffer;
	}
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BufferComponentKind {
	#[default]
	Ring,
	Latest,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BufferOverflowPolicy {
	#[default]
	DropOldest,
	DropNewest,
	BlockProducer,
	ReplaceOld,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct EngineComponentConfig {
	pub id: String,
	pub kind: EngineComponentKind,
	pub settings: toml::value::Table,
}

impl Default for EngineComponentConfig {
	fn default() -> Self {
		Self {
			id: "engine".to_string(),
			kind: EngineComponentKind::default(),
			settings: toml::value::Table::new(),
		}
	}
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EngineComponentKind {
	#[default]
	MediaPipeNative,
}

impl EngineComponentKind {
	pub const fn all() -> &'static [Self] {
		&[Self::MediaPipeNative]
	}

	pub const fn id(&self) -> &'static str {
		match self {
			Self::MediaPipeNative => "media-pipe-native",
		}
	}

	pub const fn label(&self) -> &'static str {
		match self {
			Self::MediaPipeNative => "MediaPipe Native",
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct PostProcessComponentConfig {
	pub id: String,
	pub kind: PostProcessComponentKind,
	pub settings: toml::value::Table,
}

impl Default for PostProcessComponentConfig {
	fn default() -> Self {
		Self {
			id: "post-process".to_string(),
			kind: PostProcessComponentKind::default(),
			settings: toml::value::Table::new(),
		}
	}
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PostProcessComponentKind {
	#[default]
	#[serde(rename = "media-pipe-default", alias = "media-pipe-stable")]
	MediaPipeStable,
	MediaPipeExperimental1,
	MediaPipeExperimental2,
	None,
}

impl PostProcessComponentKind {
	pub const fn all() -> &'static [Self] {
		&[Self::MediaPipeStable, Self::None]
	}

	pub const fn id(&self) -> &'static str {
		match self {
			Self::MediaPipeStable => "media-pipe-default",
			Self::MediaPipeExperimental1 => "media-pipe-experimental1",
			Self::MediaPipeExperimental2 => "media-pipe-experimental2",
			Self::None => "none",
		}
	}

	pub const fn label(&self) -> &'static str {
		match self {
			Self::MediaPipeStable => "MediaPipe to UNMotion/VMC",
			Self::MediaPipeExperimental1 => "MediaPipe Experimental 1",
			Self::MediaPipeExperimental2 => "MediaPipe Experimental 2",
			Self::None => "Raw / no conversion",
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct OutputComponentConfig {
	pub id: String,
	pub kind: OutputComponentKind,
	pub enabled: bool,
	pub settings: toml::value::Table,
}

impl OutputComponentConfig {
	pub fn vmc_default() -> Self {
		Self {
			id: "output-vmc".to_string(),
			kind: OutputComponentKind::Vmc,
			enabled: true,
			settings: toml::value::Table::new(),
		}
	}
}

impl Default for OutputComponentConfig {
	fn default() -> Self {
		Self::vmc_default()
	}
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputComponentKind {
	#[default]
	Vmc,
	Zenoh,
	VrcOsc,
	Debug,
	Record,
}

impl OutputComponentKind {
	pub const fn all() -> &'static [Self] {
		&[Self::Vmc, Self::Zenoh, Self::VrcOsc, Self::Debug, Self::Record]
	}

	pub const fn id(&self) -> &'static str {
		match self {
			Self::Vmc => "vmc",
			Self::Zenoh => "zenoh",
			Self::VrcOsc => "vrc-osc",
			Self::Debug => "debug",
			Self::Record => "record",
		}
	}

	pub const fn label(&self) -> &'static str {
		match self {
			Self::Vmc => "VMC",
			Self::Zenoh => "Zenoh",
			Self::VrcOsc => "VRC (VRCFT) / OSC",
			Self::Debug => "Debug",
			Self::Record => "Record",
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct OutputConfig {
	pub vmc: VmcOutputConfig,
}

impl Default for OutputConfig {
	fn default() -> Self {
		Self {
			vmc: VmcOutputConfig::default(),
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct VmcOutputConfig {
	pub target_addr: String,
	pub send_ok_packet: bool,
	pub blendshape_map: HashMap<String, VmcBlendshapeEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum VmcBlendshapeEntry {
	Name(String),
	Detailed(VmcBlendshapeDetail),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct VmcBlendshapeDetail {
	pub name: String,
	pub scale: f32,
	pub offset: f32,
	pub clamp_min: Option<f32>,
	pub clamp_max: Option<f32>,
}

impl Default for VmcBlendshapeDetail {
	fn default() -> Self {
		Self {
			name: String::new(),
			scale: 1.0,
			offset: 0.0,
			clamp_min: None,
			clamp_max: None,
		}
	}
}

impl Default for VmcOutputConfig {
	fn default() -> Self {
		let mut blendshape_map = HashMap::new();
		blendshape_map.insert("head.yaw".to_string(), VmcBlendshapeEntry::Name("HeadYaw".to_string()));
		blendshape_map.insert("head.pitch".to_string(), VmcBlendshapeEntry::Name("HeadPitch".to_string()));
		blendshape_map.insert("head.roll".to_string(), VmcBlendshapeEntry::Name("HeadRoll".to_string()));
		Self {
			target_addr: "127.0.0.1:39539".to_string(),
			send_ok_packet: true,
			blendshape_map,
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct PipelinePolicyConfig {
	pub stale_timeout_ns: u64,
	pub hold_last_ticks: u32,
	pub hold_decay_per_tick: f32,
	pub source_priority: HashMap<String, i32>,
	pub source_min_confidence: HashMap<String, f32>,
	pub source_stale_timeout_ns: HashMap<String, u64>,
	pub priority_weight: f32,
	pub confidence_weight: f32,
	pub freshness_weight: f32,
}

impl Default for PipelinePolicyConfig {
	fn default() -> Self {
		Self {
			stale_timeout_ns: DEFAULT_STALE_TIMEOUT_NS,
			hold_last_ticks: 0,
			hold_decay_per_tick: 1.0,
			source_priority: HashMap::new(),
			source_min_confidence: HashMap::new(),
			source_stale_timeout_ns: HashMap::new(),
			priority_weight: 1000.0,
			confidence_weight: 100.0,
			freshness_weight: 1.0,
		}
	}
}

impl PipelinePolicyConfig {
	pub fn to_pipeline_policy(&self) -> PipelinePolicy {
		PipelinePolicy {
			stale_timeout_ns: self.stale_timeout_ns,
			hold_last_ticks: self.hold_last_ticks,
			hold_decay_per_tick: self.hold_decay_per_tick,
			source_priority: self.source_priority.clone(),
			source_min_confidence: self.source_min_confidence.clone(),
			source_stale_timeout_ns: self.source_stale_timeout_ns.clone(),
			priority_weight: self.priority_weight,
			confidence_weight: self.confidence_weight,
			freshness_weight: self.freshness_weight,
		}
	}
}

pub fn load_profile_from_path(path: impl AsRef<Path>) -> anyhow::Result<RunnerProfile> {
	let path = path.as_ref();
	let text = std::fs::read_to_string(path).with_context(|| format!("設定ファイルの読み込みに失敗: {}", path.display()))?;
	let profile: RunnerProfile = toml::from_str(&text).with_context(|| format!("設定ファイル(TOML)の解析に失敗: {}", path.display()))?;
	validate_profile(&profile).with_context(|| format!("設定ファイルの妥当性検証に失敗: {}", path.display()))?;
	Ok(profile)
}

pub fn validate_profile(profile: &RunnerProfile) -> anyhow::Result<()> {
	validate_component_id("components.input.id", &profile.components.input.id)?;
	validate_component_id("components.input_buffer.id", &profile.components.input_buffer.id)?;
	validate_component_id("components.engine.id", &profile.components.engine.id)?;
	validate_component_id("components.post_process.id", &profile.components.post_process.id)?;
	validate_component_id("components.output_buffer.id", &profile.components.output_buffer.id)?;
	if profile.components.input_buffer.capacity == 0 {
		bail!("components.input_buffer.capacity must be greater than 0");
	}
	if profile.components.output_buffer.capacity == 0 {
		bail!("components.output_buffer.capacity must be greater than 0");
	}
	validate_toml_integer_u64("pipeline.stale_timeout_ns", profile.pipeline.stale_timeout_ns)?;
	for (source, timeout) in &profile.pipeline.source_stale_timeout_ns {
		validate_toml_integer_u64(&format!("pipeline.source_stale_timeout_ns.{source}"), *timeout)?;
	}
	if profile.components.outputs.is_empty() {
		bail!("components.outputs must contain at least one output component");
	}
	let mut output_ids = std::collections::HashSet::new();
	for output in &profile.components.outputs {
		validate_component_id("components.outputs[].id", &output.id)?;
		if !output_ids.insert(output.id.as_str()) {
			bail!("components.outputs contains duplicate id: {}", output.id);
		}
	}

	for (signal_name, entry) in &profile.output.vmc.blendshape_map {
		if signal_name.trim().is_empty() {
			bail!("output.vmc.blendshape_map の signal 名が空です");
		}

		let detail = match entry {
			VmcBlendshapeEntry::Name(name) => {
				if name.trim().is_empty() {
					bail!("output.vmc.blendshape_map[{}] の name が空です", signal_name);
				}
				continue;
			}
			VmcBlendshapeEntry::Detailed(detail) => detail,
		};

		if detail.name.trim().is_empty() {
			bail!("output.vmc.blendshape_map[{}].name が空です", signal_name);
		}

		if let (Some(min), Some(max)) = (detail.clamp_min, detail.clamp_max) {
			if min > max {
				bail!(
					"output.vmc.blendshape_map[{}] の clamp 範囲が不正です (min={} > max={})",
					signal_name,
					min,
					max
				);
			}
		}
	}

	Ok(())
}

fn validate_toml_integer_u64(field: &str, value: u64) -> anyhow::Result<()> {
	if value > i64::MAX as u64 {
		bail!("{field} must be <= i64::MAX for TOML serialization");
	}
	Ok(())
}

fn validate_component_id(field: &str, id: &str) -> anyhow::Result<()> {
	if id.trim().is_empty() {
		bail!("{field} must not be empty");
	}
	if !id
		.chars()
		.all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
	{
		bail!("{field} may only contain ASCII letters, digits, '.', '-', and '_'");
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_profile_with_partial_values_and_defaults() {
		let text = r#"
[pipeline]
stale_timeout_ns = 50000000
hold_last_ticks = 2
hold_decay_per_tick = 0.8

[output.vmc]
target_addr = "127.0.0.1:39539"
send_ok_packet = false

[output.vmc.blendshape_map]
"head.yaw" = "HeadYaw"

[output.vmc.blendshape_map."head.pitch"]
name = "HeadPitch"
scale = 0.5
offset = 0.1
clamp_min = -1.0
clamp_max = 1.0

[pipeline.source_priority]
camera = 10

[pipeline.source_min_confidence]
camera = 0.65
"#;

		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		assert_eq!(profile.components.engine.kind, EngineComponentKind::MediaPipeNative);
		assert_eq!(profile.components.input_buffer.kind, BufferComponentKind::Latest);
		assert_eq!(profile.components.input_buffer.capacity, 1);
		assert_eq!(profile.components.input_buffer.overflow, BufferOverflowPolicy::ReplaceOld);
		assert_eq!(profile.pipeline.stale_timeout_ns, 50_000_000);
		assert_eq!(profile.pipeline.hold_last_ticks, 2);
		assert_eq!(profile.pipeline.hold_decay_per_tick, 0.8);
		assert_eq!(profile.pipeline.source_priority.get("camera"), Some(&10));
		assert_eq!(profile.pipeline.source_min_confidence.get("camera"), Some(&0.65));
		assert!(profile.pipeline.source_stale_timeout_ns.is_empty());
		assert_eq!(profile.pipeline.priority_weight, 1000.0);
		assert_eq!(profile.output.vmc.target_addr, "127.0.0.1:39539");
		assert!(!profile.output.vmc.send_ok_packet);
		assert_eq!(
			profile.output.vmc.blendshape_map.get("head.yaw"),
			Some(&VmcBlendshapeEntry::Name("HeadYaw".to_string()))
		);
		assert_eq!(
			profile.output.vmc.blendshape_map.get("head.pitch"),
			Some(&VmcBlendshapeEntry::Detailed(VmcBlendshapeDetail {
				name: "HeadPitch".to_string(),
				scale: 0.5,
				offset: 0.1,
				clamp_min: Some(-1.0),
				clamp_max: Some(1.0),
			}))
		);
	}

	#[test]
	fn pipeline_policy_conversion_copies_values() {
		let mut cfg = PipelinePolicyConfig {
			stale_timeout_ns: 100,
			hold_last_ticks: 1,
			hold_decay_per_tick: 0.9,
			source_priority: HashMap::new(),
			source_min_confidence: HashMap::new(),
			source_stale_timeout_ns: HashMap::new(),
			priority_weight: 3.0,
			confidence_weight: 2.0,
			freshness_weight: 1.0,
		};
		cfg.source_priority.insert("s1".to_string(), 9);
		cfg.source_min_confidence.insert("s1".to_string(), 0.7);

		let policy = cfg.to_pipeline_policy();
		assert_eq!(policy.stale_timeout_ns, 100);
		assert_eq!(policy.hold_last_ticks, 1);
		assert_eq!(policy.hold_decay_per_tick, 0.9);
		assert_eq!(policy.source_priority.get("s1"), Some(&9));
		assert_eq!(policy.source_min_confidence.get("s1"), Some(&0.7));
		assert_eq!(policy.priority_weight, 3.0);
		assert_eq!(policy.confidence_weight, 2.0);
		assert_eq!(policy.freshness_weight, 1.0);
	}

	#[test]
	fn output_defaults_are_applied_when_omitted() {
		let text = r#"
[pipeline]
stale_timeout_ns = 10
"#;

		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		assert_eq!(profile.output.vmc.target_addr, "127.0.0.1:39539");
		assert!(profile.output.vmc.send_ok_packet);
		assert_eq!(
			profile.output.vmc.blendshape_map.get("head.yaw"),
			Some(&VmcBlendshapeEntry::Name("HeadYaw".to_string()))
		);
		assert_eq!(
			profile.output.vmc.blendshape_map.get("head.pitch"),
			Some(&VmcBlendshapeEntry::Name("HeadPitch".to_string()))
		);
		assert_eq!(
			profile.output.vmc.blendshape_map.get("head.roll"),
			Some(&VmcBlendshapeEntry::Name("HeadRoll".to_string()))
		);
	}

	#[test]
	fn default_profile_serializes_to_toml() {
		let raw = toml::to_string_pretty(&RunnerProfile::default()).expect("default profile should serialize");
		assert!(raw.contains("stale_timeout_ns"));
	}

	#[test]
	fn validate_fails_on_toml_integer_overflow_timeout() {
		let mut profile = RunnerProfile::default();
		profile.pipeline.stale_timeout_ns = u64::MAX;
		let err = validate_profile(&profile).expect_err("validation should fail");
		assert!(err.to_string().contains("i64::MAX"));
	}

	#[test]
	fn validate_fails_on_empty_detailed_name() {
		let text = r#"
[output.vmc.blendshape_map."head.pitch"]
name = ""
"#;
		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		let err = validate_profile(&profile).expect_err("validation should fail");
		assert!(err.to_string().contains("name が空"));
	}

	#[test]
	fn validate_fails_on_invalid_clamp_range() {
		let text = r#"
[output.vmc.blendshape_map."head.pitch"]
name = "HeadPitch"
clamp_min = 1.0
clamp_max = 0.5
"#;
		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		let err = validate_profile(&profile).expect_err("validation should fail");
		assert!(err.to_string().contains("clamp 範囲が不正"));
	}

	#[test]
	fn parse_component_graph_config() {
		let text = r##"
[components.input]
id = "camera"
kind = "file-image"

[components.input.settings]
path = "target/vmc-captures/thumb-opposition-08.png"
emission_mode = "repeat-fps"
fps = 30
pad_color = "#000000ff"

[components.input_buffer]
id = "input-ring"
kind = "ring"
capacity = 8
overflow = "drop-newest"

[components.engine]
id = "mp-native"
kind = "media-pipe-native"

[components.post_process]
id = "mp-default"
kind = "media-pipe-default"

[components.output_buffer]
id = "output-ring"
kind = "ring"
capacity = 4
overflow = "replace-old"

[[components.outputs]]
id = "vmc"
kind = "vmc"
enabled = true

[[components.outputs]]
id = "debug"
kind = "debug"
enabled = false
"##;
		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		assert_eq!(profile.components.input.id, "camera");
		assert_eq!(profile.components.input.kind, InputComponentKind::FileImage);
		assert_eq!(profile.components.input_buffer.capacity, 8);
		assert_eq!(profile.components.input_buffer.overflow, BufferOverflowPolicy::DropNewest);
		assert_eq!(profile.components.engine.kind, EngineComponentKind::MediaPipeNative);
		assert_eq!(profile.components.output_buffer.overflow, BufferOverflowPolicy::ReplaceOld);
		assert_eq!(profile.components.outputs.len(), 2);
		validate_profile(&profile).expect("component graph should validate");
	}

	#[test]
	fn parse_research_penn_action_file_video_example() {
		let text = include_str!("../../../configs/research-penn-action-file-video.example.toml");
		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		assert_eq!(profile.components.input.kind, InputComponentKind::FileVideo);
		assert_eq!(
			profile.components.input.settings.get("source_id").and_then(toml::Value::as_str),
			Some("penn-action:0001")
		);
		validate_profile(&profile).expect("example config should validate");
	}

	#[test]
	fn automatic_buffer_strategy_matches_input_kind() {
		let mut realtime = RunnerProfile::default().components;
		realtime.input.kind = InputComponentKind::WebcamDirectShow;
		realtime.apply_automatic_buffer_strategy();
		assert_eq!(realtime.input_buffer.kind, BufferComponentKind::Latest);
		assert_eq!(realtime.input_buffer.capacity, 1);
		assert_eq!(realtime.input_buffer.overflow, BufferOverflowPolicy::ReplaceOld);
		assert_eq!(realtime.output_buffer.kind, BufferComponentKind::Latest);

		let mut file_video = RunnerProfile::default().components;
		file_video.input.kind = InputComponentKind::FileVideo;
		file_video.apply_automatic_buffer_strategy();
		assert_eq!(file_video.input_buffer.kind, BufferComponentKind::Ring);
		assert_eq!(file_video.input_buffer.capacity, 8);
		assert_eq!(file_video.input_buffer.overflow, BufferOverflowPolicy::BlockProducer);
		assert_eq!(file_video.output_buffer.kind, BufferComponentKind::Ring);
	}

	#[test]
	fn validate_fails_on_duplicate_output_component_id() {
		let text = r#"
[[components.outputs]]
id = "same"
kind = "vmc"

[[components.outputs]]
id = "same"
kind = "debug"
"#;
		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		let err = validate_profile(&profile).expect_err("validation should fail");
		assert!(err.to_string().contains("duplicate id"));
	}

	#[test]
	fn validate_fails_on_zero_buffer_capacity() {
		let text = r#"
[components.input_buffer]
capacity = 0
"#;
		let profile: RunnerProfile = toml::from_str(text).expect("toml should parse");
		let err = validate_profile(&profile).expect_err("validation should fail");
		assert!(err.to_string().contains("capacity"));
	}

	#[test]
	fn component_catalog_marks_platform_specific_inputs() {
		let catalog = PipelineComponentCatalog::current_platform();
		assert!(catalog.inputs.iter().any(|option| option.id == "file-image" && option.available));
		assert!(catalog.inputs.iter().any(|option| option.id == "file-video" && option.available));
		let directshow = catalog
			.inputs
			.iter()
			.find(|option| option.id == "webcam-directshow")
			.expect("directshow option");
		assert_eq!(directshow.available, cfg!(windows));
		let nokhwa = catalog
			.inputs
			.iter()
			.find(|option| option.id == "webcam-nokhwa")
			.expect("nokhwa option");
		assert_eq!(nokhwa.available, !cfg!(windows));
		assert!(
			catalog
				.inputs
				.iter()
				.any(|option| option.id == "webcam-browser" && option.available)
		);
		assert!(catalog.engines.iter().any(|option| option.id == "media-pipe-native"));
		assert!(catalog.post_processes.iter().any(|option| option.id == "media-pipe-default"));
		assert!(catalog.outputs.iter().any(|option| option.id == "vmc"));
	}
}
