//! Capturer 出力段で適用する Modifier (Engine 非依存の宣言的パイプライン)。
//!
//! Capturer の正式経路
//! `Input → Engine(MediaPipe + post-process) → UNMotionFrame → Modifier → Output`
//! の Modifier 段に相当する。Engine が推定 / 受信して生成した `UNMotionFrame` に
//! 対して、Capturer の各出力 (UNMotionFrame/Zenoh, VMC/UDP) が共通で参照できる
//! 軽量な変換層を提供する。
//!
//! # Phase E-α 設計
//!
//! 各 stage を **第一級関数 (trait object)** として扱う。stage の trait object を
//! `Vec<Box<dyn ModifierStage>>` に並べた `ModifierPipeline` を Profile の設定から
//! 構築し、frame ごとに `apply` するだけ、というシンプルな形に揃える。
//!
//! - **stage level の遅延評価**: 各 stage の `from_config` は no-op (identity 動作) の
//!   ときに `None` を返し、Pipeline に含めない。Mirror が `MirrorMode::Normal` の場合
//!   は Mirror stage そのものが Pipeline に存在しなくなり、frame 毎ループから完全に
//!   姿を消す。
//! - **bone level の遅延評価**: 各 stage は内部で `BoneSubsetConfig::bone_enabled` を
//!   参照し、Filter で disable されている bone は smoothing / mirror 計算自体を skip
//!   できる。stage 内部の実装最適化として段階的に取り入れる。
//! - **段順は宣言的に Profile から指定可能** (`ModifierConfig::stage_order`)。デフォルト
//!   は `NeutralCalibration → TorsoPitch → Smoothing → Mirror → BoneFilter` で、
//!   意味論的に「**基準を整える** → **姿勢量を整える** → **ノイズを整える** →
//!   **視点を整える** → **不要を捨てる**」と読める並びを採る。
//!
use std::collections::HashMap;

use un_motion_frame::{HandMotion, HumanoidBone, MotionSignalValue, Quatf, TransformSample, UNMotionFrame, Vec3f};

// ============================================================================
// Stage 種別
// ============================================================================

/// Pipeline の段順を Profile から宣言的に指定するための識別子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageKind {
	NeutralCalibration,
	TorsoPitch,
	Smoothing,
	Mirror,
	BoneFilter,
}

/// デフォルトの段順: `NeutralCalibration → TorsoPitch → Smoothing → Mirror → BoneFilter`。
/// 意味論的に「基準を整える → 姿勢量を整える → ノイズを整える → 視点を整える → 不要を捨てる」と読める並び。
pub fn default_stage_order() -> Vec<StageKind> {
	vec![
		StageKind::NeutralCalibration,
		StageKind::TorsoPitch,
		StageKind::Smoothing,
		StageKind::Mirror,
		StageKind::BoneFilter,
	]
}

// ============================================================================
// Stage 個別 config
// ============================================================================

const DEFAULT_ADAPTIVE_MIN_CUTOFF_HZ: f32 = 0.35;
const DEFAULT_ADAPTIVE_BETA: f32 = 0.08;
const DEFAULT_ADAPTIVE_DERIVATIVE_CUTOFF_HZ: f32 = 1.0;

/// Smoothing preset。ユーザー GUI から選択する単一値で fixed-α slerp EMA または
/// One-Euro Filter の挙動を表現する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SmoothingPreset {
	/// 平滑化なし (pass-through)。Stage は Pipeline に含まれない。
	#[default]
	Off,
	/// 軽い平滑化。現在値の重み α = 0.7。
	Low,
	/// 中程度の平滑化。desktop baseline に対応する α = 0.45。
	Medium,
	/// 強い平滑化。α = 0.25。
	High,
	/// 速度に応じてカットオフ周波数を自動調整 (One-Euro Filter for quaternions /
	/// scalars; Casiez et al. 2012)。低速時は強い jitter 抑制、高速時は lag 最小化を
	/// 同時に達成する。既定値は実カメラ姿勢推定の静止 jitter を抑えるため
	/// `min_cutoff = 0.35 Hz, beta = 0.08, d_cutoff = 1.0 Hz`。
	Adaptive,
}

/// Smoothing stage 設定。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmoothingConfig {
	pub preset: SmoothingPreset,
	pub ema_enabled: bool,
	pub ema_alpha: f32,
	pub one_euro_enabled: bool,
	pub confidence_adaptive_cutoff_enabled: bool,
	pub adaptive_min_cutoff_hz: f32,
	pub adaptive_beta: f32,
	pub adaptive_derivative_cutoff_hz: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NeutralCalibrationConfig {
	pub enabled: bool,
	pub rotations: HashMap<String, [f32; 4]>,
}

impl NeutralCalibrationConfig {
	pub fn is_disabled(&self) -> bool {
		!self.enabled || self.rotations.is_empty()
	}
}

impl Default for SmoothingConfig {
	fn default() -> Self {
		Self {
			preset: SmoothingPreset::Off,
			ema_enabled: false,
			ema_alpha: 0.45,
			one_euro_enabled: false,
			confidence_adaptive_cutoff_enabled: false,
			adaptive_min_cutoff_hz: DEFAULT_ADAPTIVE_MIN_CUTOFF_HZ,
			adaptive_beta: DEFAULT_ADAPTIVE_BETA,
			adaptive_derivative_cutoff_hz: DEFAULT_ADAPTIVE_DERIVATIVE_CUTOFF_HZ,
		}
	}
}

impl SmoothingConfig {
	pub fn is_disabled(&self) -> bool {
		!self.ema_enabled && !self.one_euro_enabled && matches!(self.preset, SmoothingPreset::Off)
	}
}

/// Mirror stage 設定。Phase E-α-2 で Engine 側 (`MediaPipePostProcessConfig::mirror_mode`)
/// から Modifier 側に移管予定。現時点は mode enum のみ持つ skeleton。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MirrorMode {
	/// 反転なし (identity)。
	#[default]
	Normal,
	/// 出力データそのものを左右反転する (LeftX ↔ RightX の swap + quaternion x 軸反転)。
	MirrorOutput,
	/// 左右の対応関係をそのままに、quaternion だけ反転する (ハードウェア座標系補正向け)。
	SwapSides,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MirrorConfig {
	pub mode: MirrorMode,
}

impl MirrorConfig {
	pub fn is_identity(&self) -> bool {
		self.mode == MirrorMode::Normal
	}
}

/// Bone subset filter 設定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoneSubsetConfig {
	pub head_enabled: bool,
	pub face_enabled: bool,
	pub hands_enabled: bool,
	pub arms_ik_enabled: bool,
	pub torso_enabled: bool,
	pub legs_enabled: bool,
	pub feet_enabled: bool,
}

impl Default for BoneSubsetConfig {
	fn default() -> Self {
		Self {
			head_enabled: true,
			face_enabled: true,
			hands_enabled: true,
			arms_ik_enabled: true,
			torso_enabled: true,
			legs_enabled: true,
			feet_enabled: true,
		}
	}
}

impl BoneSubsetConfig {
	pub fn is_pass_through(&self) -> bool {
		self.head_enabled
			&& self.face_enabled
			&& self.hands_enabled
			&& self.arms_ik_enabled
			&& self.torso_enabled
			&& self.legs_enabled
			&& self.feet_enabled
	}

	pub fn bone_enabled(&self, bone: HumanoidBone) -> bool {
		match bone {
			HumanoidBone::Head | HumanoidBone::Neck => self.head_enabled,
			HumanoidBone::LeftEye | HumanoidBone::RightEye | HumanoidBone::Jaw => self.face_enabled,
			HumanoidBone::LeftHand | HumanoidBone::RightHand => self.hands_enabled,
			HumanoidBone::LeftShoulder
			| HumanoidBone::LeftUpperArm
			| HumanoidBone::LeftLowerArm
			| HumanoidBone::RightShoulder
			| HumanoidBone::RightUpperArm
			| HumanoidBone::RightLowerArm => self.arms_ik_enabled,
			HumanoidBone::Spine | HumanoidBone::Chest | HumanoidBone::UpperChest => self.torso_enabled,
			HumanoidBone::Hips => self.legs_enabled,
			HumanoidBone::LeftUpperLeg | HumanoidBone::LeftLowerLeg | HumanoidBone::RightUpperLeg | HumanoidBone::RightLowerLeg => {
				self.legs_enabled
			}
			HumanoidBone::LeftFoot | HumanoidBone::LeftToes | HumanoidBone::RightFoot | HumanoidBone::RightToes => self.feet_enabled,
		}
	}
}

// ============================================================================
// ModifierConfig (全体)
// ============================================================================

/// Capturer 出力段で適用する Modifier 全体の設定。
///
/// Bone subset、neutral calibration、smoothing、mirror を 1 つの出力段設定として保持する。
#[derive(Clone, Debug, PartialEq)]
pub struct ModifierConfig {
	pub head_enabled: bool,
	pub face_enabled: bool,
	pub hands_enabled: bool,
	pub arms_ik_enabled: bool,
	pub torso_enabled: bool,
	pub legs_enabled: bool,
	pub feet_enabled: bool,
	pub torso_pitch_scale: f32,
	pub neutral_calibration: NeutralCalibrationConfig,
	pub smoothing: SmoothingConfig,
	pub mirror: MirrorConfig,
	pub stage_order: Vec<StageKind>,
}

impl Default for ModifierConfig {
	fn default() -> Self {
		Self {
			head_enabled: true,
			face_enabled: true,
			hands_enabled: true,
			arms_ik_enabled: true,
			torso_enabled: true,
			legs_enabled: true,
			feet_enabled: true,
			torso_pitch_scale: 1.0,
			neutral_calibration: NeutralCalibrationConfig::default(),
			smoothing: SmoothingConfig::default(),
			mirror: MirrorConfig::default(),
			stage_order: default_stage_order(),
		}
	}
}

impl ModifierConfig {
	/// Pipeline 全体が identity (no-op) かどうか。bone subset 全 ON かつ smoothing
	/// 無効かつ mirror identity のとき `true`。
	pub fn is_pass_through(&self) -> bool {
		self.bone_subset().is_pass_through()
			&& torso_pitch_is_identity(self.torso_pitch_scale)
			&& self.neutral_calibration.is_disabled()
			&& self.smoothing.is_disabled()
			&& self.mirror.is_identity()
	}

	pub fn bone_enabled(&self, bone: HumanoidBone) -> bool {
		self.bone_subset().bone_enabled(bone)
	}

	/// top-level bone subset フラグを `BoneSubsetConfig` に集約。
	pub fn bone_subset(&self) -> BoneSubsetConfig {
		BoneSubsetConfig {
			head_enabled: self.head_enabled,
			face_enabled: self.face_enabled,
			hands_enabled: self.hands_enabled,
			arms_ik_enabled: self.arms_ik_enabled,
			torso_enabled: self.torso_enabled,
			legs_enabled: self.legs_enabled,
			feet_enabled: self.feet_enabled,
		}
	}
}

// ============================================================================
// ModifierStage trait
// ============================================================================

/// Modifier の各 stage が実装する trait。
///
/// stage は `&mut self` を取るので内部に状態 (前フレーム履歴等) を保持できる。Smoothing
/// stage のような stateful な処理も同じ trait で扱える。
///
pub trait ModifierStage: Send {
	/// 観測用名前。stage 自体が pipeline に入っていれば `ModifierPipeline::stage_names`
	/// 経由で読まれる。
	#[allow(dead_code)]
	fn name(&self) -> &'static str;
	fn apply(&mut self, frame: &mut UNMotionFrame);
}

// ============================================================================
// ModifierPipeline
// ============================================================================

/// `ModifierConfig` から構築した stage の合成パイプライン。
///
/// 各 stage の `from_config` が `None` を返す (= no-op) 場合は Pipeline に含めない。
/// これにより Mirror OFF / Smoothing OFF / Bone Filter 全 ON のような pass-through
/// ケースでは `stages` が空、もしくは `BoneFilterStage` 1 つだけになる。
pub struct ModifierPipeline {
	stages: Vec<Box<dyn ModifierStage>>,
}

impl ModifierPipeline {
	pub fn from_config(config: &ModifierConfig) -> Self {
		// 各 stage で「Filter で消える bone は smoothing / mirror 計算自体を skip」する
		// bone level 遅延評価のため、Pipeline 全体で共有する bone subset スナップショットを
		// 各 stage に DI で渡す (Copy で渡せる軽量 struct)。
		let bone_subset = config.bone_subset();
		let mut stages: Vec<Box<dyn ModifierStage>> = Vec::new();
		for kind in &config.stage_order {
			match kind {
				StageKind::NeutralCalibration => {
					if let Some(stage) = NeutralCalibrationStage::from_config(&config.neutral_calibration, bone_subset) {
						stages.push(Box::new(stage));
					}
				}
				StageKind::TorsoPitch => {
					if let Some(stage) = TorsoPitchStage::from_config(config.torso_pitch_scale, bone_subset) {
						stages.push(Box::new(stage));
					}
				}
				StageKind::Smoothing => {
					if let Some(stage) = SmoothingStage::from_config(&config.smoothing, bone_subset) {
						stages.push(Box::new(stage));
					}
				}
				StageKind::Mirror => {
					if let Some(stage) = MirrorStage::from_config(&config.mirror, bone_subset) {
						stages.push(Box::new(stage));
					}
				}
				StageKind::BoneFilter => {
					if let Some(stage) = BoneFilterStage::from_config(&bone_subset) {
						stages.push(Box::new(stage));
					}
				}
			}
		}
		Self { stages }
	}

	/// Pipeline 内 stage 数。テストおよび観測用 (Pipeline level の遅延評価が効いている
	/// ことの確認に使う)。
	#[allow(dead_code)]
	pub fn stage_count(&self) -> usize {
		self.stages.len()
	}

	/// 各 stage 名のリスト (観測 / debug 用)。
	#[allow(dead_code)]
	pub fn stage_names(&self) -> Vec<&'static str> {
		self.stages.iter().map(|s| s.name()).collect()
	}

	pub fn apply(&mut self, frame: &mut UNMotionFrame) {
		if self.stages.is_empty() {
			return;
		}
		for stage in &mut self.stages {
			stage.apply(frame);
		}
	}
}

pub struct NeutralCalibrationStage {
	rotations: HashMap<String, Quatf>,
	bone_subset: BoneSubsetConfig,
}

impl NeutralCalibrationStage {
	pub fn from_config(config: &NeutralCalibrationConfig, bone_subset: BoneSubsetConfig) -> Option<Self> {
		if config.is_disabled() {
			return None;
		}
		let rotations = config
			.rotations
			.iter()
			.filter(|(key, _)| neutral_calibration_key_enabled(key))
			.map(|(key, rotation)| (key.clone(), quat_normalize(quat_from_array(*rotation))))
			.collect::<HashMap<_, _>>();
		Some(Self { rotations, bone_subset })
	}

	fn apply_rotation(&self, key: &str, rotation: &mut Quatf) {
		let Some(neutral) = self.rotation_for_key(key) else {
			return;
		};
		*rotation = quat_normalize(quat_mul(*rotation, quat_inverse(neutral)));
	}

	fn rotation_for_key(&self, key: &str) -> Option<Quatf> {
		self.rotations.get(key).copied().or_else(|| match key {
			"FaceHead" => self.rotations.get("Head").copied(),
			"LeftWrist" => self.rotations.get("LeftHand").copied(),
			"RightWrist" => self.rotations.get("RightHand").copied(),
			_ => None,
		})
	}
}

fn neutral_calibration_key_enabled(key: &str) -> bool {
	// Limb rotations are pose outputs, not stable neutral offsets. Applying U/T/I
	// calibration offsets to arms or wrists makes normal motion over-rotate badly.
	matches!(key, "Root" | "Head")
}

impl ModifierStage for NeutralCalibrationStage {
	fn name(&self) -> &'static str {
		"neutral-calibration"
	}

	fn apply(&mut self, frame: &mut UNMotionFrame) {
		if let Some(body) = frame.body.as_mut()
			&& let Some(humanoid) = body.humanoid.as_mut()
		{
			if let Some(root) = humanoid.root.as_mut()
				&& let Some(rotation) = root.rotation.as_mut()
			{
				self.apply_rotation("Root", rotation);
			}
			for bone in &mut humanoid.bones {
				if !self.bone_subset.bone_enabled(bone.bone) {
					continue;
				}
				if let Some(rotation) = bone.transform.rotation.as_mut() {
					self.apply_rotation(humanoid_bone_key(bone.bone), rotation);
				}
			}
		}
		if let Some(face) = frame.face.as_mut()
			&& let Some(head) = face.head.as_mut()
			&& let Some(rotation) = head.rotation.as_mut()
		{
			self.apply_rotation("FaceHead", rotation);
		}
		if let Some(hand) = frame.left_hand.as_mut()
			&& let Some(wrist) = hand.wrist.as_mut()
			&& let Some(rotation) = wrist.rotation.as_mut()
		{
			self.apply_rotation("LeftWrist", rotation);
		}
		if let Some(hand) = frame.right_hand.as_mut()
			&& let Some(wrist) = hand.wrist.as_mut()
			&& let Some(rotation) = wrist.rotation.as_mut()
		{
			self.apply_rotation("RightWrist", rotation);
		}
	}
}

// ============================================================================
// TorsoPitchStage
// ============================================================================

pub struct TorsoPitchStage {
	scale: f32,
	bone_subset: BoneSubsetConfig,
}

impl TorsoPitchStage {
	pub fn from_config(scale: f32, bone_subset: BoneSubsetConfig) -> Option<Self> {
		let scale = sanitize_torso_pitch_scale(scale);
		if torso_pitch_is_identity(scale) || !bone_subset.torso_enabled {
			return None;
		}
		Some(Self { scale, bone_subset })
	}

	fn apply_rotation(&self, bone: HumanoidBone, rotation: &mut Quatf) {
		if !self.bone_subset.bone_enabled(bone) || !torso_pitch_bone(bone) {
			return;
		}
		*rotation = scale_quat_local_x_component(*rotation, self.scale);
	}
}

impl ModifierStage for TorsoPitchStage {
	fn name(&self) -> &'static str {
		"torso_pitch"
	}

	fn apply(&mut self, frame: &mut UNMotionFrame) {
		if let Some(body) = frame.body.as_mut()
			&& let Some(humanoid) = body.humanoid.as_mut()
		{
			for bone in &mut humanoid.bones {
				if let Some(rotation) = bone.transform.rotation.as_mut() {
					self.apply_rotation(bone.bone, rotation);
				}
			}
		}
	}
}

fn torso_pitch_bone(bone: HumanoidBone) -> bool {
	matches!(bone, HumanoidBone::Spine | HumanoidBone::Chest | HumanoidBone::UpperChest)
}

fn sanitize_torso_pitch_scale(scale: f32) -> f32 {
	if scale.is_finite() { scale.clamp(0.0, 1.0) } else { 1.0 }
}

fn torso_pitch_is_identity(scale: f32) -> bool {
	sanitize_torso_pitch_scale(scale) >= 0.999
}

// ============================================================================
// BoneFilterStage (実体)
// ============================================================================

pub struct BoneFilterStage {
	config: BoneSubsetConfig,
}

impl BoneFilterStage {
	pub fn from_config(config: &BoneSubsetConfig) -> Option<Self> {
		if config.is_pass_through() {
			None
		} else {
			Some(Self { config: *config })
		}
	}
}

impl ModifierStage for BoneFilterStage {
	fn name(&self) -> &'static str {
		"bone_filter"
	}

	fn apply(&mut self, frame: &mut UNMotionFrame) {
		if let Some(body) = frame.body.as_mut()
			&& let Some(humanoid) = body.humanoid.as_mut()
		{
			humanoid.bones.retain(|bone| self.config.bone_enabled(bone.bone));
			if !self.config.torso_enabled {
				humanoid.root = None;
			}
			if humanoid.bones.is_empty() && humanoid.root.is_none() {
				body.humanoid = None;
			}
		}
		if let Some(face) = frame.face.as_mut() {
			if !self.config.face_enabled {
				face.expressions.clear();
			}
			if !self.config.head_enabled {
				face.head = None;
			}
		}
		if !self.config.head_enabled {
			frame.eyes = None;
		}
		if !self.config.hands_enabled {
			frame.left_hand = None;
			frame.right_hand = None;
		}
		frame.signals.retain(|signal| self.signal_enabled(&signal.name));
	}
}

impl BoneFilterStage {
	fn signal_enabled(&self, name: &str) -> bool {
		if name.starts_with("head.") {
			return self.config.head_enabled;
		}
		if name.starts_with("face.") {
			return self.config.face_enabled;
		}
		if name.starts_with("eye.") {
			return self.config.face_enabled;
		}
		if name.starts_with("hand.") {
			return self.config.hands_enabled;
		}
		if name.starts_with("arm.") {
			return self.config.arms_ik_enabled;
		}
		if name.starts_with("torso.") {
			return self.config.torso_enabled;
		}
		if name.starts_with("leg.") {
			return self.config.legs_enabled;
		}
		if name.starts_with("foot.") {
			return self.config.feet_enabled;
		}
		true
	}
}

// ============================================================================
// SmoothingStage (Phase E-α-1a: fixed-α slerp EMA)
// ============================================================================

/// `UNMotionFrame` の `body.humanoid` 内 bone rotation (quaternion)、root transform
/// (translation / rotation)、`face.expressions` (blendshape scalar) に slerp / lerp
/// ベースの平滑化を適用する stage。
///
/// # 2 つのモード
///
/// - `SmoothingMode::Fixed { alpha }` (preset Low/Medium/High): 固定の重み α で
///   `slerp(prev_smoothed, current, α)` を計算する。シンプルだが、急峻な動きに対して
///   一律で lag が出る。
/// - `SmoothingMode::Adaptive { min_cutoff, beta, d_cutoff }` (preset Adaptive):
///   Casiez et al. 2012 "1€ Filter" を quaternion / scalar に拡張した適応的フィルタ。
///   サンプル間 dt と推定角速度に応じてカットオフ周波数を動的に調整し、低速時は
///   jitter を強く抑え、高速時は lag を減らす。`min_cutoff` が静止時の応答、
///   `beta` が速度依存ゲイン、`d_cutoff` が角速度推定自体の平滑化カットオフ。
///   推奨値 `min_cutoff = 1.0 Hz, beta = 0.007, d_cutoff = 1.0 Hz` (Casiez 推奨)。
///
/// # bone level の遅延評価
///
/// `bone_subset` (BoneSubsetConfig の copy) を保持し、`bone_enabled(bone) == false`
/// の bone は smoothing 計算自体を skip する。これにより BoneFilter で disable
/// される bone は smoothing 段でも計算ゼロで通過する。
///
/// # スコープ
///
/// - `body.humanoid.root` の translation / rotation
/// - `body.humanoid.bones[*].transform.rotation` (bone_subset で skip 判定)
/// - `face.expressions[*].value` (blendshape scalar; expression name で indexing)
/// - `left_hand` / `right_hand` の wrist / finger joint transform。
pub struct SmoothingStage {
	mode: SmoothingMode,
	bone_subset: BoneSubsetConfig,
	bone_state: HashMap<HumanoidBone, QuatSmoothingState>,
	named_quat_state: HashMap<String, QuatSmoothingState>,
	named_vec3_state: HashMap<String, Vec3SmoothingState>,
	root_translation_state: Option<Vec3SmoothingState>,
	root_rotation_state: Option<QuatSmoothingState>,
	face_expression_state: HashMap<String, ScalarSmoothingState>,
	signal_scalar_state: HashMap<String, ScalarSmoothingState>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SmoothingMode {
	/// 固定 α の slerp / lerp EMA。`alpha` は「現在値の重み」 (1.0 = pass-through,
	/// 0.0 = static)。
	Fixed { alpha: f32 },
	/// One-Euro Filter (Casiez et al. 2012) を quaternion / scalar に拡張した
	/// 適応的フィルタ。すべての周波数単位は Hz。
	Adaptive {
		min_cutoff: f32,
		beta: f32,
		d_cutoff: f32,
		confidence_adaptive_cutoff: bool,
	},
	/// EMA と One-Euro を同一 stage 内で順に適用する明示設定モード。
	Combined {
		ema_alpha: Option<f32>,
		adaptive: Option<AdaptiveParams>,
	},
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdaptiveParams {
	min_cutoff: f32,
	beta: f32,
	d_cutoff: f32,
	confidence_adaptive_cutoff: bool,
}

/// Quaternion (bone rotation / root rotation) の smoothing state。
#[derive(Clone, Copy, Debug)]
struct QuatSmoothingState {
	smoothed: Quatf,
	/// Adaptive 専用: 前回 raw 入力。角速度推定の差分計算に使う。
	last_raw: Quatf,
	/// Adaptive 専用: 平滑化された角速度 (rad/sec)。
	smoothed_speed: f32,
	/// Adaptive 専用: 前回サンプルの timestamp (ns)。0 / None は「未初期化」。
	last_timestamp_ns: Option<u64>,
}

/// Vec3 (root translation) の smoothing state。Adaptive モードでは各成分独立の
/// 1€ Filter。
#[derive(Clone, Copy, Debug)]
struct Vec3SmoothingState {
	smoothed: Vec3f,
	last_raw: Vec3f,
	smoothed_speed: Vec3f,
	last_timestamp_ns: Option<u64>,
}

/// Scalar (face expression value) の smoothing state。
#[derive(Clone, Copy, Debug)]
struct ScalarSmoothingState {
	smoothed: f32,
	last_raw: f32,
	smoothed_speed: f32,
	last_timestamp_ns: Option<u64>,
}

impl SmoothingStage {
	pub fn from_config(config: &SmoothingConfig, bone_subset: BoneSubsetConfig) -> Option<Self> {
		let adaptive = AdaptiveParams {
			min_cutoff: sanitize_positive(config.adaptive_min_cutoff_hz, DEFAULT_ADAPTIVE_MIN_CUTOFF_HZ),
			beta: sanitize_non_negative(config.adaptive_beta, DEFAULT_ADAPTIVE_BETA),
			d_cutoff: sanitize_positive(config.adaptive_derivative_cutoff_hz, DEFAULT_ADAPTIVE_DERIVATIVE_CUTOFF_HZ),
			confidence_adaptive_cutoff: config.confidence_adaptive_cutoff_enabled,
		};
		let mode = if config.ema_enabled || config.one_euro_enabled {
			SmoothingMode::Combined {
				ema_alpha: config.ema_enabled.then(|| config.ema_alpha.clamp(0.0, 1.0)),
				adaptive: config.one_euro_enabled.then_some(adaptive),
			}
		} else {
			match config.preset {
				SmoothingPreset::Off => return None,
				SmoothingPreset::Low => SmoothingMode::Fixed { alpha: 0.70 },
				SmoothingPreset::Medium => SmoothingMode::Fixed { alpha: 0.45 },
				SmoothingPreset::High => SmoothingMode::Fixed { alpha: 0.25 },
				SmoothingPreset::Adaptive => SmoothingMode::Adaptive {
					min_cutoff: adaptive.min_cutoff,
					beta: adaptive.beta,
					d_cutoff: adaptive.d_cutoff,
					confidence_adaptive_cutoff: adaptive.confidence_adaptive_cutoff,
				},
			}
		};
		Some(Self {
			mode,
			bone_subset,
			bone_state: HashMap::new(),
			named_quat_state: HashMap::new(),
			named_vec3_state: HashMap::new(),
			root_translation_state: None,
			root_rotation_state: None,
			face_expression_state: HashMap::new(),
			signal_scalar_state: HashMap::new(),
		})
	}

	fn step_quat(mode: SmoothingMode, state: &mut QuatSmoothingState, current: Quatf, timestamp_ns: u64, confidence: f32) -> Quatf {
		match mode {
			SmoothingMode::Fixed { alpha } => {
				let smoothed = quat_slerp(state.smoothed, current, alpha);
				state.smoothed = smoothed;
				smoothed
			}
			SmoothingMode::Adaptive {
				min_cutoff,
				beta,
				d_cutoff,
				confidence_adaptive_cutoff,
			} => {
				let dt = match state.last_timestamp_ns {
					Some(prev_ns) if timestamp_ns > prev_ns => (timestamp_ns - prev_ns) as f32 / 1_000_000_000.0,
					_ => {
						// dt 不明 / 同タイムスタンプ → 速度推定不可、Fixed 0.45 相当で fallback。
						let smoothed = quat_slerp(state.smoothed, current, 0.45);
						state.smoothed = smoothed;
						state.last_raw = current;
						state.last_timestamp_ns = Some(timestamp_ns);
						return smoothed;
					}
				};
				// 角速度推定: angle = 2 * acos(|dot|) [rad], speed = angle / dt [rad/s]
				let dot = quat_dot(state.last_raw, current).abs().min(1.0);
				let angle = 2.0 * dot.acos();
				let raw_speed = angle / dt.max(1e-6);
				// 角速度自体を 1€ Filter の d_cutoff で平滑化
				let d_alpha = one_euro_alpha(dt, d_cutoff);
				state.smoothed_speed = lerp(state.smoothed_speed, raw_speed, d_alpha);
				// 速度依存 cutoff frequency
				let min_cutoff = confidence_adjusted_min_cutoff(min_cutoff, confidence, confidence_adaptive_cutoff);
				let cutoff = (min_cutoff + beta * state.smoothed_speed).max(min_cutoff);
				let alpha = one_euro_alpha(dt, cutoff);
				let smoothed = quat_slerp(state.smoothed, current, alpha);
				state.smoothed = smoothed;
				state.last_raw = current;
				state.last_timestamp_ns = Some(timestamp_ns);
				smoothed
			}
			SmoothingMode::Combined { ema_alpha, adaptive } => {
				let raw_current = current;
				let mut current = current;
				if let Some(alpha) = ema_alpha {
					current = quat_slerp(state.smoothed, current, alpha);
				}
				if let Some(params) = adaptive {
					let smoothed = Self::step_quat(
						SmoothingMode::Adaptive {
							min_cutoff: params.min_cutoff,
							beta: params.beta,
							d_cutoff: params.d_cutoff,
							confidence_adaptive_cutoff: params.confidence_adaptive_cutoff,
						},
						state,
						current,
						timestamp_ns,
						if params.confidence_adaptive_cutoff { confidence } else { 1.0 },
					);
					state.last_raw = raw_current;
					smoothed
				} else {
					state.smoothed = current;
					current
				}
			}
		}
	}

	fn step_vec3(mode: SmoothingMode, state: &mut Vec3SmoothingState, current: Vec3f, timestamp_ns: u64, confidence: f32) -> Vec3f {
		match mode {
			SmoothingMode::Fixed { alpha } => {
				let smoothed = vec3_lerp(state.smoothed, current, alpha);
				state.smoothed = smoothed;
				smoothed
			}
			SmoothingMode::Adaptive {
				min_cutoff,
				beta,
				d_cutoff,
				confidence_adaptive_cutoff,
			} => {
				let dt = match state.last_timestamp_ns {
					Some(prev_ns) if timestamp_ns > prev_ns => (timestamp_ns - prev_ns) as f32 / 1_000_000_000.0,
					_ => {
						let smoothed = vec3_lerp(state.smoothed, current, 0.45);
						state.smoothed = smoothed;
						state.last_raw = current;
						state.last_timestamp_ns = Some(timestamp_ns);
						return smoothed;
					}
				};
				let raw_speed = Vec3f {
					x: (current.x - state.last_raw.x) / dt.max(1e-6),
					y: (current.y - state.last_raw.y) / dt.max(1e-6),
					z: (current.z - state.last_raw.z) / dt.max(1e-6),
				};
				let d_alpha = one_euro_alpha(dt, d_cutoff);
				state.smoothed_speed = Vec3f {
					x: lerp(state.smoothed_speed.x, raw_speed.x, d_alpha),
					y: lerp(state.smoothed_speed.y, raw_speed.y, d_alpha),
					z: lerp(state.smoothed_speed.z, raw_speed.z, d_alpha),
				};
				let min_cutoff = confidence_adjusted_min_cutoff(min_cutoff, confidence, confidence_adaptive_cutoff);
				let cutoff_x = (min_cutoff + beta * state.smoothed_speed.x.abs()).max(min_cutoff);
				let cutoff_y = (min_cutoff + beta * state.smoothed_speed.y.abs()).max(min_cutoff);
				let cutoff_z = (min_cutoff + beta * state.smoothed_speed.z.abs()).max(min_cutoff);
				let smoothed = Vec3f {
					x: lerp(state.smoothed.x, current.x, one_euro_alpha(dt, cutoff_x)),
					y: lerp(state.smoothed.y, current.y, one_euro_alpha(dt, cutoff_y)),
					z: lerp(state.smoothed.z, current.z, one_euro_alpha(dt, cutoff_z)),
				};
				state.smoothed = smoothed;
				state.last_raw = current;
				state.last_timestamp_ns = Some(timestamp_ns);
				smoothed
			}
			SmoothingMode::Combined { ema_alpha, adaptive } => {
				let raw_current = current;
				let mut current = current;
				if let Some(alpha) = ema_alpha {
					current = vec3_lerp(state.smoothed, current, alpha);
				}
				if let Some(params) = adaptive {
					let smoothed = Self::step_vec3(
						SmoothingMode::Adaptive {
							min_cutoff: params.min_cutoff,
							beta: params.beta,
							d_cutoff: params.d_cutoff,
							confidence_adaptive_cutoff: params.confidence_adaptive_cutoff,
						},
						state,
						current,
						timestamp_ns,
						if params.confidence_adaptive_cutoff { confidence } else { 1.0 },
					);
					state.last_raw = raw_current;
					smoothed
				} else {
					state.smoothed = current;
					current
				}
			}
		}
	}

	fn step_scalar(mode: SmoothingMode, state: &mut ScalarSmoothingState, current: f32, timestamp_ns: u64, confidence: f32) -> f32 {
		match mode {
			SmoothingMode::Fixed { alpha } => {
				let smoothed = lerp(state.smoothed, current, alpha);
				state.smoothed = smoothed;
				smoothed
			}
			SmoothingMode::Adaptive {
				min_cutoff,
				beta,
				d_cutoff,
				confidence_adaptive_cutoff,
			} => {
				let dt = match state.last_timestamp_ns {
					Some(prev_ns) if timestamp_ns > prev_ns => (timestamp_ns - prev_ns) as f32 / 1_000_000_000.0,
					_ => {
						let smoothed = lerp(state.smoothed, current, 0.45);
						state.smoothed = smoothed;
						state.last_raw = current;
						state.last_timestamp_ns = Some(timestamp_ns);
						return smoothed;
					}
				};
				let raw_speed = (current - state.last_raw) / dt.max(1e-6);
				let d_alpha = one_euro_alpha(dt, d_cutoff);
				state.smoothed_speed = lerp(state.smoothed_speed, raw_speed, d_alpha);
				let min_cutoff = confidence_adjusted_min_cutoff(min_cutoff, confidence, confidence_adaptive_cutoff);
				let cutoff = (min_cutoff + beta * state.smoothed_speed.abs()).max(min_cutoff);
				let alpha = one_euro_alpha(dt, cutoff);
				let smoothed = lerp(state.smoothed, current, alpha);
				state.smoothed = smoothed;
				state.last_raw = current;
				state.last_timestamp_ns = Some(timestamp_ns);
				smoothed
			}
			SmoothingMode::Combined { ema_alpha, adaptive } => {
				let raw_current = current;
				let mut current = current;
				if let Some(alpha) = ema_alpha {
					current = lerp(state.smoothed, current, alpha);
				}
				if let Some(params) = adaptive {
					let smoothed = Self::step_scalar(
						SmoothingMode::Adaptive {
							min_cutoff: params.min_cutoff,
							beta: params.beta,
							d_cutoff: params.d_cutoff,
							confidence_adaptive_cutoff: params.confidence_adaptive_cutoff,
						},
						state,
						current,
						timestamp_ns,
						if params.confidence_adaptive_cutoff { confidence } else { 1.0 },
					);
					state.last_raw = raw_current;
					smoothed
				} else {
					state.smoothed = current;
					current
				}
			}
		}
	}

	/// Quaternion を 1 サンプル進める。状態を持たない bone は current をそのまま使う
	/// (= 初回サンプル)。bone_subset で disable されている bone は呼出側で skip。
	fn step_bone_rotation(&mut self, bone: HumanoidBone, current: Quatf, timestamp_ns: u64, confidence: f32) -> Quatf {
		let mode = self.mode;
		if let Some(state) = self.bone_state.get_mut(&bone) {
			Self::step_quat(mode, state, current, timestamp_ns, confidence)
		} else {
			self.bone_state.insert(
				bone,
				QuatSmoothingState {
					smoothed: current,
					last_raw: current,
					smoothed_speed: 0.0,
					last_timestamp_ns: if timestamp_ns > 0 { Some(timestamp_ns) } else { None },
				},
			);
			current
		}
	}

	fn step_root_translation(&mut self, current: Vec3f, timestamp_ns: u64, confidence: f32) -> Vec3f {
		let mode = self.mode;
		if let Some(state) = self.root_translation_state.as_mut() {
			Self::step_vec3(mode, state, current, timestamp_ns, confidence)
		} else {
			self.root_translation_state = Some(Vec3SmoothingState {
				smoothed: current,
				last_raw: current,
				smoothed_speed: Vec3f { x: 0.0, y: 0.0, z: 0.0 },
				last_timestamp_ns: if timestamp_ns > 0 { Some(timestamp_ns) } else { None },
			});
			current
		}
	}

	fn step_root_rotation(&mut self, current: Quatf, timestamp_ns: u64, confidence: f32) -> Quatf {
		let mode = self.mode;
		if let Some(state) = self.root_rotation_state.as_mut() {
			Self::step_quat(mode, state, current, timestamp_ns, confidence)
		} else {
			self.root_rotation_state = Some(QuatSmoothingState {
				smoothed: current,
				last_raw: current,
				smoothed_speed: 0.0,
				last_timestamp_ns: if timestamp_ns > 0 { Some(timestamp_ns) } else { None },
			});
			current
		}
	}

	fn step_expression(&mut self, name: &str, current: f32, timestamp_ns: u64, confidence: f32) -> f32 {
		Self::step_named_scalar(&mut self.face_expression_state, self.mode, name, current, timestamp_ns, confidence)
	}

	fn step_signal_scalar(&mut self, name: &str, current: f32, timestamp_ns: u64, confidence: f32) -> f32 {
		Self::step_named_scalar(&mut self.signal_scalar_state, self.mode, name, current, timestamp_ns, confidence)
	}

	fn step_named_scalar(
		state_map: &mut HashMap<String, ScalarSmoothingState>,
		mode: SmoothingMode,
		name: &str,
		current: f32,
		timestamp_ns: u64,
		confidence: f32,
	) -> f32 {
		if let Some(state) = state_map.get_mut(name) {
			Self::step_scalar(mode, state, current, timestamp_ns, confidence)
		} else {
			state_map.insert(
				name.to_string(),
				ScalarSmoothingState {
					smoothed: current,
					last_raw: current,
					smoothed_speed: 0.0,
					last_timestamp_ns: if timestamp_ns > 0 { Some(timestamp_ns) } else { None },
				},
			);
			current
		}
	}

	fn step_named_translation(&mut self, name: &str, current: Vec3f, timestamp_ns: u64, confidence: f32) -> Vec3f {
		let mode = self.mode;
		if let Some(state) = self.named_vec3_state.get_mut(name) {
			Self::step_vec3(mode, state, current, timestamp_ns, confidence)
		} else {
			self.named_vec3_state.insert(
				name.to_string(),
				Vec3SmoothingState {
					smoothed: current,
					last_raw: current,
					smoothed_speed: Vec3f { x: 0.0, y: 0.0, z: 0.0 },
					last_timestamp_ns: if timestamp_ns > 0 { Some(timestamp_ns) } else { None },
				},
			);
			current
		}
	}

	fn step_named_rotation(&mut self, name: &str, current: Quatf, timestamp_ns: u64, confidence: f32) -> Quatf {
		let mode = self.mode;
		if let Some(state) = self.named_quat_state.get_mut(name) {
			Self::step_quat(mode, state, current, timestamp_ns, confidence)
		} else {
			self.named_quat_state.insert(
				name.to_string(),
				QuatSmoothingState {
					smoothed: current,
					last_raw: current,
					smoothed_speed: 0.0,
					last_timestamp_ns: if timestamp_ns > 0 { Some(timestamp_ns) } else { None },
				},
			);
			current
		}
	}

	fn smooth_hand_motion(&mut self, side: &str, hand: &mut HandMotion, timestamp_ns: u64) {
		let confidence = hand.confidence;
		if let Some(wrist) = hand.wrist.as_mut() {
			self.smooth_transform_sample(&format!("hand.{side}.wrist"), wrist, timestamp_ns, confidence);
		}
		for finger in &mut hand.fingers {
			for (index, joint) in finger.joints.iter_mut().enumerate() {
				self.smooth_transform_sample(&format!("hand.{side}.{:?}.{index}", finger.finger), joint, timestamp_ns, confidence);
			}
		}
	}

	fn smooth_transform_sample(&mut self, key: &str, transform: &mut TransformSample, timestamp_ns: u64, confidence: f32) {
		if let Some(translation) = transform.translation {
			transform.translation = Some(self.step_named_translation(&format!("{key}.translation"), translation, timestamp_ns, confidence));
		}
		if let Some(rotation) = transform.rotation {
			transform.rotation = Some(self.step_named_rotation(&format!("{key}.rotation"), rotation, timestamp_ns, confidence));
		}
	}
}

impl ModifierStage for SmoothingStage {
	fn name(&self) -> &'static str {
		"smoothing"
	}

	fn apply(&mut self, frame: &mut UNMotionFrame) {
		// timestamp は header の frame_timestamp_ns を最優先で使う。0 のときは
		// Adaptive の dt 推定ができないが、step_quat 内で fallback (Fixed 0.45 相当)
		// が走るので致命的にはならない。
		let timestamp_ns = frame.header.frame_timestamp_ns;
		if let Some(body) = frame.body.as_mut()
			&& let Some(humanoid) = body.humanoid.as_mut()
		{
			if let Some(root) = humanoid.root.as_mut() {
				if let Some(translation) = root.translation {
					root.translation = Some(self.step_root_translation(translation, timestamp_ns, body.confidence));
				}
				if let Some(rotation) = root.rotation {
					root.rotation = Some(self.step_root_rotation(rotation, timestamp_ns, body.confidence));
				}
			}
			for bone in humanoid.bones.iter_mut() {
				// 遅延評価: BoneFilter で消える bone は smoothing 計算自体を skip。
				if !self.bone_subset.bone_enabled(bone.bone) {
					continue;
				}
				let Some(rotation) = bone.transform.rotation else {
					continue;
				};
				let smoothed = self.step_bone_rotation(bone.bone, rotation, timestamp_ns, bone.confidence);
				bone.transform.rotation = Some(smoothed);
			}
		}
		// face.expressions の blendshape value を smoothing。face 全体を切る判定は
		// BoneFilter (face_enabled=false) の責務なのでここでは触らない。
		if let Some(face) = frame.face.as_mut() {
			if let Some(head) = face.head.as_mut() {
				self.smooth_transform_sample("face.head", head, timestamp_ns, face.confidence);
			}
			for expression in face.expressions.iter_mut() {
				let smoothed = self.step_expression(&expression.name, expression.value, timestamp_ns, expression.confidence);
				expression.value = smoothed;
			}
		}
		if self.bone_subset.hands_enabled {
			if let Some(hand) = frame.left_hand.as_mut() {
				self.smooth_hand_motion("left", hand, timestamp_ns);
			}
			if let Some(hand) = frame.right_hand.as_mut() {
				self.smooth_hand_motion("right", hand, timestamp_ns);
			}
		}
		for signal in frame.signals.iter_mut() {
			let MotionSignalValue::Scalar(value) = signal.value else {
				continue;
			};
			signal.value = MotionSignalValue::Scalar(self.step_signal_scalar(&signal.name, value, timestamp_ns, signal.confidence));
		}
	}
}

// ============================================================================
// MirrorStage (E-α-2: UNMotionFrame に対する bone-transform level mirror)
// ============================================================================

/// Mirror stage 本実装。Engine 側 (`MediaPipePostProcessConfig::mirror_mode`) の
/// signal-level mirror から Modifier 側の bone-transform level mirror に処理を
/// 移管する。
///
/// # MirrorMode の意味論
///
/// - `MirrorMode::Normal`: 反転なし。`from_config` が `None` を返し pipeline に
///   入らないため、ホットパスでは仮想呼び出しすら発生しない。
/// - `MirrorMode::MirrorOutput`: ユーザーの動きを avatar に「鏡像で」マップする。
///   Webcam を見て右手を上げると avatar (avatar 視点) の右手が上がる。実装的には
///   bone を Left ↔ Right pair で swap し、translation の x 成分と quaternion の
///   y/z 成分の符号を反転する (X 軸鏡映)。`face.expressions[*].name` の `Left` /
///   `Right` パターンも入れ替える。
/// - `MirrorMode::SwapSides`: 座標系補正用に bone を Left ↔ Right pair で swap
///   するのみ。quaternion / translation は触らない (ハードウェア座標系の左右誤り
///   補正向け)。
///
/// # bone-level の遅延評価
///
/// `bone_subset` で disable された bone は mirror 処理も skip する。BoneFilter で
/// 消える bone は smoothing 段同様 mirror 段でも計算ゼロで通過する。
pub struct MirrorStage {
	mode: MirrorMode,
	bone_subset: BoneSubsetConfig,
}

impl MirrorStage {
	pub fn from_config(config: &MirrorConfig, bone_subset: BoneSubsetConfig) -> Option<Self> {
		if config.is_identity() {
			return None;
		}
		Some(Self {
			mode: config.mode,
			bone_subset,
		})
	}
}

impl ModifierStage for MirrorStage {
	fn name(&self) -> &'static str {
		"mirror"
	}

	fn apply(&mut self, frame: &mut UNMotionFrame) {
		let mode = self.mode;
		let mirror_transforms = matches!(mode, MirrorMode::MirrorOutput);
		if let Some(body) = frame.body.as_mut()
			&& let Some(humanoid) = body.humanoid.as_mut()
		{
			if mirror_transforms {
				if let Some(root) = humanoid.root.as_mut() {
					if let Some(t) = root.translation.as_mut() {
						t.x = -t.x;
					}
					if let Some(r) = root.rotation.as_mut() {
						r.y = -r.y;
						r.z = -r.z;
					}
				}
			}
			for bone in humanoid.bones.iter_mut() {
				if !self.bone_subset.bone_enabled(bone.bone) {
					continue;
				}
				bone.bone = mirror_bone(bone.bone);
				if mirror_transforms {
					if let Some(t) = bone.transform.translation.as_mut() {
						t.x = -t.x;
					}
					if let Some(r) = bone.transform.rotation.as_mut() {
						r.y = -r.y;
						r.z = -r.z;
					}
				}
			}
		}
		// face.head pose も X 軸鏡映を受ける (MirrorOutput のみ)。
		if mirror_transforms
			&& let Some(face) = frame.face.as_mut()
			&& let Some(head) = face.head.as_mut()
		{
			if let Some(t) = head.translation.as_mut() {
				t.x = -t.x;
			}
			if let Some(r) = head.rotation.as_mut() {
				r.y = -r.y;
				r.z = -r.z;
			}
		}
		// face.expressions の名前に含まれる Left/Right を入れ替える。
		// (ARKit blendshape は eyeBlinkLeft / mouthSmileRight などの命名規則)
		if let Some(face) = frame.face.as_mut() {
			for expr in face.expressions.iter_mut() {
				expr.name = swap_left_right_in_expression_name(&expr.name);
			}
		}
	}
}

/// `HumanoidBone` の Left ↔ Right pair を入れ替える。pair でない bone はそのまま返す。
fn mirror_bone(bone: HumanoidBone) -> HumanoidBone {
	use HumanoidBone::*;
	match bone {
		LeftShoulder => RightShoulder,
		RightShoulder => LeftShoulder,
		LeftUpperArm => RightUpperArm,
		RightUpperArm => LeftUpperArm,
		LeftLowerArm => RightLowerArm,
		RightLowerArm => LeftLowerArm,
		LeftHand => RightHand,
		RightHand => LeftHand,
		LeftUpperLeg => RightUpperLeg,
		RightUpperLeg => LeftUpperLeg,
		LeftLowerLeg => RightLowerLeg,
		RightLowerLeg => LeftLowerLeg,
		LeftFoot => RightFoot,
		RightFoot => LeftFoot,
		LeftToes => RightToes,
		RightToes => LeftToes,
		LeftEye => RightEye,
		RightEye => LeftEye,
		other => other,
	}
}

fn humanoid_bone_key(bone: HumanoidBone) -> &'static str {
	use HumanoidBone::*;
	match bone {
		Hips => "Hips",
		Spine => "Spine",
		Chest => "Chest",
		UpperChest => "UpperChest",
		Neck => "Neck",
		Head => "Head",
		Jaw => "Jaw",
		LeftEye => "LeftEye",
		RightEye => "RightEye",
		LeftShoulder => "LeftShoulder",
		LeftUpperArm => "LeftUpperArm",
		LeftLowerArm => "LeftLowerArm",
		LeftHand => "LeftHand",
		RightShoulder => "RightShoulder",
		RightUpperArm => "RightUpperArm",
		RightLowerArm => "RightLowerArm",
		RightHand => "RightHand",
		LeftUpperLeg => "LeftUpperLeg",
		LeftLowerLeg => "LeftLowerLeg",
		LeftFoot => "LeftFoot",
		LeftToes => "LeftToes",
		RightUpperLeg => "RightUpperLeg",
		RightLowerLeg => "RightLowerLeg",
		RightFoot => "RightFoot",
		RightToes => "RightToes",
	}
}

/// ARKit / VRM blendshape 名の Left ↔ Right を入れ替える。
///
/// 誤検出 (substring contamination) を避けるため、命名規則的に明確な
/// 「suffix パターン」のみを対象とする:
///
/// - ARKit: `eyeBlinkLeft` / `mouthSmileRight` (末尾の `Left` / `Right`)
/// - VRM などの underscore 形式: `Blink_L` / `Blink_R` (末尾の `_L` / `_R`)
///
/// それ以外 (中間に Left/Right を含むだけの名前など) はそのまま返す。
fn swap_left_right_in_expression_name(name: &str) -> String {
	if let Some(stem) = name.strip_suffix("Left") {
		return format!("{stem}Right");
	}
	if let Some(stem) = name.strip_suffix("Right") {
		return format!("{stem}Left");
	}
	if let Some(stem) = name.strip_suffix("_L") {
		return format!("{stem}_R");
	}
	if let Some(stem) = name.strip_suffix("_R") {
		return format!("{stem}_L");
	}
	name.to_string()
}

// ============================================================================
// Quaternion / Vec3 ヘルパー (modifier 内部用)
// ============================================================================

fn quat_dot(a: Quatf, b: Quatf) -> f32 {
	a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w
}

fn quat_from_array(rotation: [f32; 4]) -> Quatf {
	Quatf {
		x: rotation[0],
		y: rotation[1],
		z: rotation[2],
		w: rotation[3],
	}
}

#[cfg(test)]
fn quat_to_array(rotation: Quatf) -> [f32; 4] {
	[rotation.x, rotation.y, rotation.z, rotation.w]
}

fn quat_negate(q: Quatf) -> Quatf {
	Quatf {
		x: -q.x,
		y: -q.y,
		z: -q.z,
		w: -q.w,
	}
}

fn quat_inverse(q: Quatf) -> Quatf {
	let q = quat_normalize(q);
	Quatf {
		x: -q.x,
		y: -q.y,
		z: -q.z,
		w: q.w,
	}
}

fn quat_mul(a: Quatf, b: Quatf) -> Quatf {
	Quatf {
		x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
		y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
		z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
		w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
	}
}

fn quat_normalize(q: Quatf) -> Quatf {
	let len_sq = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
	if len_sq < 1e-12 {
		Quatf {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: 1.0,
		}
	} else {
		let inv_len = 1.0 / len_sq.sqrt();
		Quatf {
			x: q.x * inv_len,
			y: q.y * inv_len,
			z: q.z * inv_len,
			w: q.w * inv_len,
		}
	}
}

fn quat_local_x_component(q: Quatf) -> Quatf {
	quat_normalize(Quatf {
		x: q.x,
		y: 0.0,
		z: 0.0,
		w: q.w,
	})
}

fn scale_quat_local_x_component(q: Quatf, scale: f32) -> Quatf {
	let q = quat_normalize(q);
	let local_x = quat_local_x_component(q);
	let non_x = quat_mul(q, quat_inverse(local_x));
	let scaled_local_x = quat_slerp(
		Quatf {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: 1.0,
		},
		local_x,
		sanitize_torso_pitch_scale(scale),
	);
	quat_normalize(quat_mul(non_x, scaled_local_x))
}

/// 球面線形補間 (slerp)。`t` は `a` から `b` への補間係数で、`0.0 → a`, `1.0 → b`。
/// 内部で shortest-path 補正 (dot < 0 で b を反転) と高 dot 領域での lerp 近似を行う。
fn quat_slerp(a: Quatf, b: Quatf, t: f32) -> Quatf {
	let mut dot = quat_dot(a, b);
	let b = if dot < 0.0 {
		dot = -dot;
		quat_negate(b)
	} else {
		b
	};
	if dot > 0.9995 {
		// 高 dot 領域では sin による割り算で数値精度が悪化するので linear interpolate
		// + normalize で近似する。
		let result = Quatf {
			x: a.x + (b.x - a.x) * t,
			y: a.y + (b.y - a.y) * t,
			z: a.z + (b.z - a.z) * t,
			w: a.w + (b.w - a.w) * t,
		};
		return quat_normalize(result);
	}
	let theta_0 = dot.clamp(-1.0, 1.0).acos();
	let sin_theta_0 = theta_0.sin();
	let s0 = ((1.0 - t) * theta_0).sin() / sin_theta_0;
	let s1 = (t * theta_0).sin() / sin_theta_0;
	Quatf {
		x: a.x * s0 + b.x * s1,
		y: a.y * s0 + b.y * s1,
		z: a.z * s0 + b.z * s1,
		w: a.w * s0 + b.w * s1,
	}
}

fn vec3_lerp(a: Vec3f, b: Vec3f, t: f32) -> Vec3f {
	Vec3f {
		x: a.x + (b.x - a.x) * t,
		y: a.y + (b.y - a.y) * t,
		z: a.z + (b.z - a.z) * t,
	}
}

/// Scalar の線形補間 `a + (b - a) * t`。
fn lerp(a: f32, b: f32, t: f32) -> f32 {
	a + (b - a) * t
}

/// 1€ Filter (Casiez et al. 2012) のローパス α を計算する。
///
/// `α = 1 / (1 + τ / dt)`, `τ = 1 / (2π * cutoff)`。
///
/// `dt` および `cutoff` が異常値の場合 (≤ 0 または NaN) は `1.0`
/// (= pass-through) を返す。
fn one_euro_alpha(dt: f32, cutoff: f32) -> f32 {
	if !(dt > 0.0) || !(cutoff > 0.0) {
		return 1.0;
	}
	let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
	let alpha = 1.0 / (1.0 + tau / dt);
	alpha.clamp(0.0, 1.0)
}

fn sanitize_positive(value: f32, fallback: f32) -> f32 {
	if value.is_finite() && value > 0.0 { value } else { fallback }
}

fn sanitize_non_negative(value: f32, fallback: f32) -> f32 {
	if value.is_finite() && value >= 0.0 { value } else { fallback }
}

fn confidence_adjusted_min_cutoff(min_cutoff: f32, confidence: f32, enabled: bool) -> f32 {
	if !enabled {
		return min_cutoff;
	}
	let factor = 0.20 + 0.80 * confidence.clamp(0.0, 1.0);
	(min_cutoff * factor).max(0.01)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use un_motion_frame::{
		BodyMotion, BoneSample, ExpressionSample, FaceMotion, HumanoidPose, SampleState, TrackingState, TransformSample,
	};

	fn apply_modifier(frame: &mut UNMotionFrame, config: &ModifierConfig) {
		let mut pipeline = ModifierPipeline::from_config(config);
		pipeline.apply(frame);
	}

	fn make_bone(bone: HumanoidBone) -> BoneSample {
		BoneSample {
			bone,
			transform: TransformSample {
				translation: None,
				rotation: None,
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			},
			confidence: 1.0,
			source_index: Some(0),
			state: SampleState::Valid,
		}
	}

	fn make_frame_with_bones(bones: Vec<HumanoidBone>) -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: bones.into_iter().map(make_bone).collect(),
			}),
		});
		frame
	}

	// ----- ModifierPipeline の挙動 -----

	#[test]
	fn pass_through_is_noop() {
		let original = make_frame_with_bones(vec![HumanoidBone::Head, HumanoidBone::LeftHand, HumanoidBone::Hips]);
		let mut frame = original.clone();
		apply_modifier(&mut frame, &ModifierConfig::default());
		assert_eq!(frame, original);
	}

	#[test]
	fn hands_disabled_removes_left_and_right_hand() {
		let mut frame = make_frame_with_bones(vec![HumanoidBone::Head, HumanoidBone::LeftHand, HumanoidBone::RightHand]);
		let config = ModifierConfig {
			hands_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		let bones = frame
			.body
			.as_ref()
			.and_then(|b| b.humanoid.as_ref())
			.map(|h| h.bones.iter().map(|b| b.bone).collect::<Vec<_>>())
			.unwrap_or_default();
		assert_eq!(bones, vec![HumanoidBone::Head]);
	}

	#[test]
	fn torso_only_keeps_chest_but_removes_hips_and_lower_body() {
		let mut frame = make_frame_with_bones(vec![
			HumanoidBone::Hips,
			HumanoidBone::Chest,
			HumanoidBone::LeftUpperLeg,
			HumanoidBone::RightUpperLeg,
			HumanoidBone::LeftFoot,
			HumanoidBone::RightFoot,
		]);
		let config = ModifierConfig {
			torso_enabled: true,
			legs_enabled: false,
			feet_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		let bones = frame
			.body
			.as_ref()
			.and_then(|b| b.humanoid.as_ref())
			.map(|h| h.bones.iter().map(|b| b.bone).collect::<Vec<_>>())
			.unwrap_or_default();
		assert_eq!(bones, vec![HumanoidBone::Chest]);
	}

	#[test]
	fn legs_enabled_keeps_hips_even_when_torso_disabled() {
		let mut frame = make_frame_with_bones(vec![HumanoidBone::Hips, HumanoidBone::Chest, HumanoidBone::LeftUpperLeg]);
		let config = ModifierConfig {
			torso_enabled: false,
			legs_enabled: true,
			feet_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		let bones = frame
			.body
			.as_ref()
			.and_then(|b| b.humanoid.as_ref())
			.map(|h| h.bones.iter().map(|b| b.bone).collect::<Vec<_>>())
			.unwrap_or_default();
		assert_eq!(bones, vec![HumanoidBone::Hips, HumanoidBone::LeftUpperLeg]);
	}

	#[test]
	fn head_disabled_removes_head_and_neck_but_keeps_eyes_under_face() {
		let mut frame = make_frame_with_bones(vec![
			HumanoidBone::Head,
			HumanoidBone::Neck,
			HumanoidBone::LeftEye,
			HumanoidBone::RightEye,
			HumanoidBone::Jaw,
		]);
		let config = ModifierConfig {
			head_enabled: false,
			face_enabled: true,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		let bones = frame
			.body
			.as_ref()
			.and_then(|b| b.humanoid.as_ref())
			.map(|h| h.bones.iter().map(|b| b.bone).collect::<Vec<_>>())
			.unwrap_or_default();
		assert_eq!(bones, vec![HumanoidBone::LeftEye, HumanoidBone::RightEye, HumanoidBone::Jaw]);
	}

	#[test]
	fn face_disabled_clears_expressions() {
		let mut frame = make_frame_with_bones(vec![HumanoidBone::Head]);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: "jawOpen".to_string(),
				value: 0.5,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});
		let config = ModifierConfig {
			face_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		assert!(frame.face.as_ref().unwrap().expressions.is_empty());
	}

	#[test]
	fn torso_disabled_clears_humanoid_root() {
		use un_motion_frame::{Quatf, TransformSample as FrameTransform, Vec3f};
		let mut frame = make_frame_with_bones(vec![HumanoidBone::LeftHand]);
		if let Some(body) = frame.body.as_mut()
			&& let Some(humanoid) = body.humanoid.as_mut()
		{
			humanoid.root = Some(FrameTransform {
				translation: Some(Vec3f { x: 0.0, y: 1.5, z: 0.0 }),
				rotation: Some(Quatf {
					x: 0.0,
					y: 0.0,
					z: 0.0,
					w: 1.0,
				}),
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			});
		}
		let config = ModifierConfig {
			torso_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		let humanoid = frame.body.as_ref().unwrap().humanoid.as_ref().unwrap();
		assert!(humanoid.root.is_none(), "root should be cleared when torso disabled");
		assert_eq!(humanoid.bones.len(), 1, "non-torso bones should remain");
	}

	#[test]
	fn head_disabled_clears_face_head_and_eyes() {
		use un_motion_frame::{EyeMotion, Quatf};
		let mut frame = make_frame_with_bones(vec![HumanoidBone::LeftHand]);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: Some(TransformSample {
				translation: None,
				rotation: Some(Quatf {
					x: 0.0,
					y: 0.0,
					z: 0.0,
					w: 1.0,
				}),
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			}),
			expressions: vec![],
		});
		frame.eyes = Some(EyeMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			left_gaze: None,
			right_gaze: None,
			combined_gaze: None,
			blink_left: Some(0.5),
			blink_right: Some(0.5),
		});
		let config = ModifierConfig {
			head_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		assert!(frame.face.as_ref().unwrap().head.is_none(), "face.head should be cleared");
		assert!(frame.eyes.is_none(), "eyes should be cleared when head disabled");
	}

	#[test]
	fn hands_disabled_clears_top_level_hand_motions() {
		use un_motion_frame::HandMotion;
		let mut frame = make_frame_with_bones(vec![HumanoidBone::Head]);
		frame.left_hand = Some(HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			wrist: None,
			fingers: Vec::new(),
		});
		frame.right_hand = Some(HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			wrist: None,
			fingers: Vec::new(),
		});
		let config = ModifierConfig {
			hands_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		assert!(frame.left_hand.is_none(), "left_hand should be cleared when hands disabled");
		assert!(frame.right_hand.is_none(), "right_hand should be cleared when hands disabled");
	}

	#[test]
	fn all_bones_disabled_collapses_humanoid_to_none() {
		let mut frame = make_frame_with_bones(vec![HumanoidBone::Head, HumanoidBone::LeftHand]);
		let config = ModifierConfig {
			head_enabled: false,
			face_enabled: false,
			hands_enabled: false,
			arms_ik_enabled: false,
			torso_enabled: false,
			legs_enabled: false,
			feet_enabled: false,
			..ModifierConfig::default()
		};
		apply_modifier(&mut frame, &config);
		assert!(frame.body.as_ref().unwrap().humanoid.is_none());
	}

	// ----- Phase E-α-0: Pipeline 構造 / 遅延評価の検証 -----

	#[test]
	fn pipeline_omits_pass_through_bone_filter_stage() {
		// bone subset 全 ON / smoothing OFF / mirror Normal → Pipeline は空。
		let config = ModifierConfig::default();
		let pipeline = ModifierPipeline::from_config(&config);
		assert_eq!(pipeline.stage_count(), 0, "pass-through pipeline must contain zero stages");
		assert!(pipeline.stage_names().is_empty());
	}

	#[test]
	fn pipeline_contains_bone_filter_stage_when_subset_active() {
		let config = ModifierConfig {
			hands_enabled: false,
			..ModifierConfig::default()
		};
		let pipeline = ModifierPipeline::from_config(&config);
		assert_eq!(pipeline.stage_count(), 1);
		assert_eq!(pipeline.stage_names(), vec!["bone_filter"]);
	}

	#[test]
	fn pipeline_includes_smoothing_mirror_and_bone_filter() {
		// Smoothing + Mirror + BoneFilter の 3 stage が `Smoothing →
		// Mirror → BoneFilter` の順で並ぶ (default_stage_order)。
		let config = ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			mirror: MirrorConfig {
				mode: MirrorMode::MirrorOutput,
			},
			hands_enabled: false,
			..ModifierConfig::default()
		};
		let pipeline = ModifierPipeline::from_config(&config);
		assert_eq!(pipeline.stage_count(), 3);
		assert_eq!(pipeline.stage_names(), vec!["smoothing", "mirror", "bone_filter"]);
	}

	#[test]
	fn pipeline_includes_torso_pitch_stage_when_scale_is_below_identity_threshold() {
		let pipeline = ModifierPipeline::from_config(&ModifierConfig {
			torso_pitch_scale: 0.75,
			..ModifierConfig::default()
		});
		assert_eq!(pipeline.stage_names(), vec!["torso_pitch"]);
	}

	#[test]
	fn pipeline_omits_torso_pitch_stage_near_identity() {
		let pipeline = ModifierPipeline::from_config(&ModifierConfig {
			torso_pitch_scale: 0.999,
			..ModifierConfig::default()
		});
		assert!(pipeline.stage_names().is_empty());
	}

	#[test]
	fn pipeline_apply_is_noop_when_empty() {
		let original = make_frame_with_bones(vec![HumanoidBone::Head, HumanoidBone::LeftHand]);
		let mut frame = original.clone();
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig::default());
		pipeline.apply(&mut frame);
		assert_eq!(frame, original);
	}

	#[test]
	fn pipeline_respects_custom_stage_order() {
		// stage_order に Smoothing / Mirror を含めない場合でも BoneFilter が動くことを確認。
		let config = ModifierConfig {
			hands_enabled: false,
			stage_order: vec![StageKind::BoneFilter],
			..ModifierConfig::default()
		};
		let pipeline = ModifierPipeline::from_config(&config);
		assert_eq!(pipeline.stage_count(), 1);
		assert_eq!(pipeline.stage_names(), vec!["bone_filter"]);
	}

	#[test]
	fn pipeline_with_empty_stage_order_has_no_stages() {
		let config = ModifierConfig {
			hands_enabled: false,
			stage_order: Vec::new(),
			..ModifierConfig::default()
		};
		let pipeline = ModifierPipeline::from_config(&config);
		assert_eq!(pipeline.stage_count(), 0);
	}

	// ----- Phase E-α-1a: SmoothingStage の挙動検証 -----

	use un_motion_frame::{Quatf as FrameQuat, TransformSample as FrameTransform, Vec3f as FrameVec3};

	fn make_bone_with_rotation(bone: HumanoidBone, rotation: FrameQuat) -> BoneSample {
		BoneSample {
			bone,
			transform: FrameTransform {
				translation: None,
				rotation: Some(rotation),
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			},
			confidence: 1.0,
			source_index: Some(0),
			state: SampleState::Valid,
		}
	}

	fn frame_with_head_rotation(rotation: FrameQuat) -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![make_bone_with_rotation(HumanoidBone::Head, rotation)],
			}),
		});
		frame
	}

	fn head_rotation(frame: &UNMotionFrame) -> FrameQuat {
		frame
			.body
			.as_ref()
			.unwrap()
			.humanoid
			.as_ref()
			.unwrap()
			.bones
			.iter()
			.find(|b| b.bone == HumanoidBone::Head)
			.unwrap()
			.transform
			.rotation
			.unwrap()
	}

	fn rotation_for_bone(frame: &UNMotionFrame, bone: HumanoidBone) -> FrameQuat {
		frame
			.body
			.as_ref()
			.and_then(|body| body.humanoid.as_ref())
			.and_then(|humanoid| humanoid.bones.iter().find(|sample| sample.bone == bone))
			.and_then(|sample| sample.transform.rotation)
			.expect("bone rotation")
	}

	const IDENTITY_Q: FrameQuat = FrameQuat {
		x: 0.0,
		y: 0.0,
		z: 0.0,
		w: 1.0,
	};

	fn quat_axis_angle(axis: [f32; 3], angle: f32) -> FrameQuat {
		let len_sq = axis.iter().map(|v| v * v).sum::<f32>();
		let inv_len = if len_sq > 1e-12 { 1.0 / len_sq.sqrt() } else { 1.0 };
		let half = angle * 0.5;
		let sin = half.sin();
		FrameQuat {
			x: axis[0] * inv_len * sin,
			y: axis[1] * inv_len * sin,
			z: axis[2] * inv_len * sin,
			w: half.cos(),
		}
	}

	#[test]
	fn torso_pitch_scale_reduces_only_torso_local_x_component() {
		let pitch = quat_axis_angle([1.0, 0.0, 0.0], 0.8);
		let yaw = quat_axis_angle([0.0, 1.0, 0.0], 0.5);
		let input = quat_normalize(quat_mul(yaw, pitch));
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![
					make_bone_with_rotation(HumanoidBone::Spine, input),
					make_bone_with_rotation(HumanoidBone::Head, input),
				],
			}),
		});
		apply_modifier(
			&mut frame,
			&ModifierConfig {
				torso_pitch_scale: 0.25,
				..ModifierConfig::default()
			},
		);

		let expected_spine = quat_normalize(quat_mul(yaw, quat_axis_angle([1.0, 0.0, 0.0], 0.2)));
		assert_frame_quat_near(rotation_for_bone(&frame, HumanoidBone::Spine), expected_spine, 1e-5);
		assert_frame_quat_near(rotation_for_bone(&frame, HumanoidBone::Head), input, 1e-5);
	}

	#[test]
	fn torso_pitch_scale_zero_removes_pure_torso_pitch() {
		let pitch = quat_axis_angle([1.0, 0.0, 0.0], -0.6);
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![make_bone_with_rotation(HumanoidBone::Chest, pitch)],
			}),
		});
		apply_modifier(
			&mut frame,
			&ModifierConfig {
				torso_pitch_scale: 0.0,
				..ModifierConfig::default()
			},
		);

		assert_frame_quat_near(rotation_for_bone(&frame, HumanoidBone::Chest), IDENTITY_Q, 1e-5);
	}

	fn assert_frame_quat_near(actual: FrameQuat, expected: FrameQuat, epsilon: f32) {
		let actual = quat_to_array(actual);
		let expected = quat_to_array(expected);
		let same = actual
			.iter()
			.zip(expected.iter())
			.all(|(actual, expected)| (actual - expected).abs() <= epsilon);
		let negated = actual
			.iter()
			.zip(expected.iter())
			.all(|(actual, expected)| (actual + expected).abs() <= epsilon);
		assert!(same || negated, "actual={actual:?} expected={expected:?}");
	}

	#[test]
	fn neutral_calibration_subtracts_captured_head_rotation() {
		let neutral = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let mut rotations = HashMap::new();
		rotations.insert("Head".to_string(), [neutral.x, neutral.y, neutral.z, neutral.w]);
		let mut frame = frame_with_head_rotation(neutral);
		let config = ModifierConfig {
			neutral_calibration: NeutralCalibrationConfig { enabled: true, rotations },
			..ModifierConfig::default()
		};

		apply_modifier(&mut frame, &config);

		let rotation = head_rotation(&frame);
		assert!((rotation.x - IDENTITY_Q.x).abs() < 1e-5);
		assert!((rotation.y - IDENTITY_Q.y).abs() < 1e-5);
		assert!((rotation.z - IDENTITY_Q.z).abs() < 1e-5);
		assert!((rotation.w - IDENTITY_Q.w).abs() < 1e-5);
	}

	#[test]
	fn neutral_calibration_applies_head_baseline_to_face_head() {
		let neutral = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let mut rotations = HashMap::new();
		rotations.insert("Head".to_string(), [neutral.x, neutral.y, neutral.z, neutral.w]);
		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: Some(FrameTransform {
				translation: None,
				rotation: Some(neutral),
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			}),
			expressions: Vec::new(),
		});
		let config = ModifierConfig {
			neutral_calibration: NeutralCalibrationConfig { enabled: true, rotations },
			..ModifierConfig::default()
		};

		apply_modifier(&mut frame, &config);

		let rotation = frame.face.as_ref().unwrap().head.as_ref().unwrap().rotation.unwrap();
		assert!((rotation.x - IDENTITY_Q.x).abs() < 1e-5);
		assert!((rotation.y - IDENTITY_Q.y).abs() < 1e-5);
		assert!((rotation.z - IDENTITY_Q.z).abs() < 1e-5);
		assert!((rotation.w - IDENTITY_Q.w).abs() < 1e-5);
	}

	#[test]
	fn neutral_calibration_removes_local_delta_from_rotation_tail() {
		let expected_y_rotation = quat_axis_angle([0.0, 1.0, 0.0], 0.7);
		let neutral_delta = quat_axis_angle([1.0, 0.0, 0.0], 0.45);
		let captured = quat_normalize(quat_mul(expected_y_rotation, neutral_delta));
		let mut rotations = HashMap::new();
		rotations.insert("Head".to_string(), quat_to_array(neutral_delta));
		let mut frame = frame_with_head_rotation(captured);
		let config = ModifierConfig {
			neutral_calibration: NeutralCalibrationConfig { enabled: true, rotations },
			..ModifierConfig::default()
		};

		apply_modifier(&mut frame, &config);

		assert_frame_quat_near(head_rotation(&frame), expected_y_rotation, 1e-5);
	}

	#[test]
	fn neutral_calibration_ignores_limb_offsets() {
		let hand_rotation = quat_axis_angle([0.0, 0.0, 1.0], 0.8);
		let lower_offset = quat_axis_angle([0.0, 1.0, 0.0], 1.2);
		let mut rotations = HashMap::new();
		rotations.insert("LeftHand".to_string(), quat_to_array(quat_axis_angle([1.0, 0.0, 0.0], 1.4)));
		rotations.insert("LeftLowerArm".to_string(), quat_to_array(lower_offset));
		let mut frame = UNMotionFrame::new(1);
		frame.left_hand = Some(HandMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			wrist: Some(FrameTransform {
				translation: None,
				rotation: Some(hand_rotation),
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			}),
			fingers: Vec::new(),
		});
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![make_bone_with_rotation(HumanoidBone::LeftLowerArm, hand_rotation)],
			}),
		});
		let config = ModifierConfig {
			neutral_calibration: NeutralCalibrationConfig { enabled: true, rotations },
			..ModifierConfig::default()
		};

		apply_modifier(&mut frame, &config);

		let lower = frame
			.body
			.as_ref()
			.unwrap()
			.humanoid
			.as_ref()
			.unwrap()
			.bones
			.iter()
			.find(|bone| bone.bone == HumanoidBone::LeftLowerArm)
			.unwrap()
			.transform
			.rotation
			.unwrap();
		let wrist = frame.left_hand.as_ref().unwrap().wrist.as_ref().unwrap().rotation.unwrap();
		assert_frame_quat_near(lower, hand_rotation, 1e-5);
		assert_frame_quat_near(wrist, hand_rotation, 1e-5);
	}

	#[test]
	fn smoothing_first_frame_is_passthrough() {
		// 初回サンプルは前回値が無いので current をそのまま採用 (== passthrough)。
		let target = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let mut frame = frame_with_head_rotation(target);
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		pipeline.apply(&mut frame);
		let out = head_rotation(&frame);
		assert!((out.x - target.x).abs() < 1e-5);
		assert!((out.y - target.y).abs() < 1e-5);
		assert!((out.w - target.w).abs() < 1e-5);
	}

	#[test]
	fn smoothing_second_frame_interpolates_toward_current() {
		// 初回 identity を吸わせ、2 回目で 90° 回転を入れる。Medium (α=0.45) なので
		// 出力は identity と target の slerp 経過点になる。
		let target = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		let mut frame1 = frame_with_head_rotation(IDENTITY_Q);
		pipeline.apply(&mut frame1);
		let mut frame2 = frame_with_head_rotation(target);
		pipeline.apply(&mut frame2);
		let out = head_rotation(&frame2);
		// 出力は target に近付くが完全一致しない (= smoothing 効果)
		assert!((out.y - target.y).abs() > 1e-3, "should not snap to target");
		assert!(out.y > 0.0, "should rotate toward target direction");
		// 出力は単位 quaternion を維持
		let norm = (out.x * out.x + out.y * out.y + out.z * out.z + out.w * out.w).sqrt();
		assert!((norm - 1.0).abs() < 1e-4, "smoothed quaternion must stay unit-norm");
	}

	#[test]
	fn smoothing_skips_filtered_bones() {
		// hands_enabled=false で LeftHand は smoothing 対象外。LeftHand に
		// 強い回転を入れても smoothing は触らない (が、後段 BoneFilter で除去される)。
		let target_hand = FrameQuat {
			x: 0.7071,
			y: 0.0,
			z: 0.0,
			w: 0.7071,
		};
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![
					make_bone_with_rotation(HumanoidBone::Head, IDENTITY_Q),
					make_bone_with_rotation(HumanoidBone::LeftHand, target_hand),
				],
			}),
		});
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			hands_enabled: false,
			..ModifierConfig::default()
		});
		// 2 stage (smoothing + bone_filter)。Smoothing は LeftHand を skip、
		// BoneFilter が LeftHand を除外する。
		assert_eq!(pipeline.stage_count(), 2);
		pipeline.apply(&mut frame);
		let bones: Vec<_> = frame
			.body
			.as_ref()
			.unwrap()
			.humanoid
			.as_ref()
			.unwrap()
			.bones
			.iter()
			.map(|b| b.bone)
			.collect();
		assert_eq!(bones, vec![HumanoidBone::Head]);
	}

	#[test]
	fn smoothing_smooths_root_translation_and_rotation() {
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});

		fn make_root_frame(translation: FrameVec3, rotation: FrameQuat) -> UNMotionFrame {
			let mut frame = UNMotionFrame::new(1);
			frame.body = Some(BodyMotion {
				tracking_state: TrackingState::Valid,
				confidence: 1.0,
				humanoid: Some(HumanoidPose {
					root: Some(FrameTransform {
						translation: Some(translation),
						rotation: Some(rotation),
						scale: None,
						linear_velocity: None,
						angular_velocity: None,
					}),
					bones: Vec::new(),
				}),
			});
			frame
		}

		let mut frame1 = make_root_frame(FrameVec3 { x: 0.0, y: 0.0, z: 0.0 }, IDENTITY_Q);
		pipeline.apply(&mut frame1);
		let mut frame2 = make_root_frame(FrameVec3 { x: 1.0, y: 2.0, z: 3.0 }, IDENTITY_Q);
		pipeline.apply(&mut frame2);
		let root = frame2.body.as_ref().unwrap().humanoid.as_ref().unwrap().root.as_ref().unwrap();
		let translation = root.translation.unwrap();
		// α = 0.45 → 出力 = lerp(zero, target, 0.45)
		assert!((translation.x - 0.45).abs() < 1e-3);
		assert!((translation.y - 0.90).abs() < 1e-3);
		assert!((translation.z - 1.35).abs() < 1e-3);
	}

	#[test]
	fn smoothing_explicit_ema_and_one_euro_can_be_combined() {
		let pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				ema_enabled: true,
				ema_alpha: 0.5,
				one_euro_enabled: true,
				adaptive_min_cutoff_hz: 0.35,
				adaptive_beta: 0.08,
				adaptive_derivative_cutoff_hz: 1.0,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		assert_eq!(pipeline.stage_count(), 1);
		assert_eq!(pipeline.stage_names(), vec!["smoothing"]);
	}

	#[test]
	fn smoothing_confidence_adaptive_cutoff_lowers_min_cutoff() {
		assert!((confidence_adjusted_min_cutoff(1.0, 1.0, true) - 1.0).abs() < 1e-6);
		assert!((confidence_adjusted_min_cutoff(1.0, 0.0, true) - 0.2).abs() < 1e-6);
		assert!((confidence_adjusted_min_cutoff(1.0, 0.5, true) - 0.6).abs() < 1e-6);
		assert!((confidence_adjusted_min_cutoff(1.0, 0.0, false) - 1.0).abs() < 1e-6);
	}

	#[test]
	fn smoothing_preset_off_does_not_add_stage() {
		let pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Off,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		assert_eq!(pipeline.stage_count(), 0);
	}

	#[test]
	fn smoothing_preset_adaptive_adds_stage() {
		// SmoothingPreset::Adaptive は One-Euro Filter として SmoothingStage に組み込まれる。
		let pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Adaptive,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		assert_eq!(pipeline.stage_count(), 1);
		assert_eq!(pipeline.stage_names(), vec!["smoothing"]);
	}

	#[test]
	fn smoothing_adaptive_uses_configured_one_euro_parameters() {
		let stage = SmoothingStage::from_config(
			&SmoothingConfig {
				preset: SmoothingPreset::Adaptive,
				adaptive_min_cutoff_hz: 0.25,
				adaptive_beta: 0.12,
				adaptive_derivative_cutoff_hz: 1.7,
				..SmoothingConfig::default()
			},
			BoneSubsetConfig::default(),
		)
		.expect("adaptive stage");

		assert_eq!(
			stage.mode,
			SmoothingMode::Adaptive {
				min_cutoff: 0.25,
				beta: 0.12,
				d_cutoff: 1.7,
				confidence_adaptive_cutoff: false,
			}
		);
	}

	fn make_bone_frame_with_ts(rotation: FrameQuat, frame_ts_ns: u64) -> UNMotionFrame {
		let mut frame = frame_with_head_rotation(rotation);
		frame.header.frame_timestamp_ns = frame_ts_ns;
		frame
	}

	fn make_face_frame_with_expression(name: &str, value: f32) -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(1);
		frame.header.frame_timestamp_ns = 1_000_000_000;
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![ExpressionSample {
				name: name.to_string(),
				value,
				confidence: 1.0,
				source_index: Some(0),
				state: SampleState::Valid,
			}],
		});
		frame
	}

	fn make_face_frame_with_head(rotation: FrameQuat) -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(1);
		frame.header.frame_timestamp_ns = 1_000_000_000;
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: Some(FrameTransform {
				translation: None,
				rotation: Some(rotation),
				scale: None,
				linear_velocity: None,
				angular_velocity: None,
			}),
			expressions: Vec::new(),
		});
		frame
	}

	#[test]
	fn smoothing_adaptive_first_frame_is_passthrough() {
		// 初回フレームは「前回値」が無いので current をそのまま通す。
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Adaptive,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		let target = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_6).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_6).cos(),
		};
		let mut frame = make_bone_frame_with_ts(target, 1_000_000_000);
		pipeline.apply(&mut frame);
		let bone = &frame.body.as_ref().unwrap().humanoid.as_ref().unwrap().bones[0];
		let rotation = bone.transform.rotation.unwrap();
		assert!((rotation.x - target.x).abs() < 1e-5);
		assert!((rotation.y - target.y).abs() < 1e-5);
		assert!((rotation.w - target.w).abs() < 1e-5);
	}

	#[test]
	fn smoothing_adaptive_attenuates_low_speed_jitter() {
		// dt=33ms (約 30Hz)、静止付近では cutoff は adaptive_min_cutoff に近くなり、
		// 低速 jitter は強く抑えられる。
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Adaptive,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		let identity = FrameQuat {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: 1.0,
		};
		// 90° around Y axis
		let target = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let mut f1 = make_bone_frame_with_ts(identity, 1_000_000_000);
		pipeline.apply(&mut f1);
		// 33ms 後に target が来る。raw_speed が小さい状態 (= 初回 step の smoothed_speed=0
		// から d_alpha でやんわり追従) なので cutoff は min_cutoff にほぼ等しい。
		let mut f2 = make_bone_frame_with_ts(target, 1_033_000_000);
		pipeline.apply(&mut f2);
		let bone = &f2.body.as_ref().unwrap().humanoid.as_ref().unwrap().bones[0];
		let rotation = bone.transform.rotation.unwrap();
		// 期待: identity と target の間。w は cos(π/4)=0.707 と 1.0 の間に収まる。
		// jitter 抑制を確認したいので、target に「届かない」(= w > 0.85) ことを assert する。
		assert!(rotation.w > 0.85, "expected attenuated output but got w={}", rotation.w);
		assert!(
			rotation.w < 1.0,
			"expected some movement but got w={} (no smoothing applied)",
			rotation.w
		);
	}

	#[test]
	fn smoothing_adaptive_zero_dt_falls_back_to_fixed_alpha() {
		// timestamp が同一 (dt=0) のときは fixed 0.45 相当の fallback で smoothing する。
		// パイプラインが panic せず、かつ何らかの中間出力を返すことを確認。
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Adaptive,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		let identity = FrameQuat {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: 1.0,
		};
		let target = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let mut f1 = make_bone_frame_with_ts(identity, 1_000_000_000);
		pipeline.apply(&mut f1);
		let mut f2 = make_bone_frame_with_ts(target, 1_000_000_000);
		pipeline.apply(&mut f2);
		let bone = &f2.body.as_ref().unwrap().humanoid.as_ref().unwrap().bones[0];
		let rotation = bone.transform.rotation.unwrap();
		assert!(rotation.w > 0.707 && rotation.w < 1.0);
	}

	#[test]
	fn smoothing_face_expressions_blendshape_value_is_smoothed() {
		// face.expressions[*].value が EMA 平滑化されること。
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		let mut f1 = make_face_frame_with_expression("jawOpen", 0.0);
		pipeline.apply(&mut f1);
		let mut f2 = make_face_frame_with_expression("jawOpen", 1.0);
		pipeline.apply(&mut f2);
		let expression = &f2.face.as_ref().unwrap().expressions[0];
		// α=0.45 → lerp(0, 1, 0.45) = 0.45
		assert!((expression.value - 0.45).abs() < 1e-3);
	}

	#[test]
	fn smoothing_face_head_transform_is_smoothed() {
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		let target = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let mut f1 = make_face_frame_with_head(IDENTITY_Q);
		pipeline.apply(&mut f1);
		let mut f2 = make_face_frame_with_head(target);
		pipeline.apply(&mut f2);
		let rotation = f2.face.as_ref().unwrap().head.as_ref().unwrap().rotation.unwrap();

		assert!(rotation.y > 0.0, "face head should move toward target");
		assert!(rotation.y < target.y, "face head should not snap to target");
	}

	#[test]
	fn smoothing_face_expression_first_frame_is_passthrough() {
		// 初回フレームは入力をそのまま通す。
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::High,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		});
		let mut f1 = make_face_frame_with_expression("eyeBlinkLeft", 0.8);
		pipeline.apply(&mut f1);
		let expression = &f1.face.as_ref().unwrap().expressions[0];
		assert!((expression.value - 0.8).abs() < 1e-5);
	}

	#[test]
	fn quat_slerp_endpoints_match_inputs() {
		let a = FrameQuat {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: 1.0,
		};
		let b = FrameQuat {
			x: 0.0,
			y: (std::f32::consts::FRAC_PI_4).sin(),
			z: 0.0,
			w: (std::f32::consts::FRAC_PI_4).cos(),
		};
		let at_a = quat_slerp(a, b, 0.0);
		let at_b = quat_slerp(a, b, 1.0);
		assert!((at_a.x - a.x).abs() < 1e-5 && (at_a.w - a.w).abs() < 1e-5);
		assert!((at_b.x - b.x).abs() < 1e-5 && (at_b.w - b.w).abs() < 1e-5);
	}

	// ----- MirrorStage (E-α-2) -----

	fn make_lr_bone_frame() -> UNMotionFrame {
		let mut frame = UNMotionFrame::new(1);
		frame.body = Some(BodyMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			humanoid: Some(HumanoidPose {
				root: None,
				bones: vec![
					make_bone_with_rotation(
						HumanoidBone::LeftHand,
						FrameQuat {
							x: 0.0,
							y: 0.5,
							z: 0.3,
							w: 0.81240386,
						},
					),
					make_bone_with_rotation(HumanoidBone::Head, IDENTITY_Q),
				],
			}),
		});
		frame.body.as_mut().unwrap().humanoid.as_mut().unwrap().bones[0]
			.transform
			.translation = Some(FrameVec3 { x: 0.4, y: 1.2, z: 0.0 });
		frame
	}

	#[test]
	fn mirror_stage_disabled_when_normal_mode() {
		// MirrorMode::Normal は pipeline に入らない (from_config が None を返す)。
		let pipeline = ModifierPipeline::from_config(&ModifierConfig {
			mirror: MirrorConfig { mode: MirrorMode::Normal },
			..ModifierConfig::default()
		});
		assert_eq!(pipeline.stage_count(), 0);
	}

	#[test]
	fn mirror_output_swaps_left_right_and_flips_x_axis() {
		let mut frame = make_lr_bone_frame();
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			mirror: MirrorConfig {
				mode: MirrorMode::MirrorOutput,
			},
			..ModifierConfig::default()
		});
		pipeline.apply(&mut frame);
		let bones = &frame.body.as_ref().unwrap().humanoid.as_ref().unwrap().bones;
		// LeftHand は RightHand に swap される
		assert_eq!(bones[0].bone, HumanoidBone::RightHand);
		// translation.x は反転
		assert!((bones[0].transform.translation.unwrap().x - (-0.4)).abs() < 1e-5);
		assert!((bones[0].transform.translation.unwrap().y - 1.2).abs() < 1e-5);
		// rotation.y / z は符号反転、x / w は不変
		let rot = bones[0].transform.rotation.unwrap();
		assert!((rot.x - 0.0).abs() < 1e-5);
		assert!((rot.y - (-0.5)).abs() < 1e-5);
		assert!((rot.z - (-0.3)).abs() < 1e-5);
		assert!((rot.w - 0.81240386).abs() < 1e-5);
		// Head はそのまま (Left/Right pair ではない)
		assert_eq!(bones[1].bone, HumanoidBone::Head);
	}

	#[test]
	fn mirror_swap_sides_swaps_bone_names_only() {
		let mut frame = make_lr_bone_frame();
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			mirror: MirrorConfig {
				mode: MirrorMode::SwapSides,
			},
			..ModifierConfig::default()
		});
		pipeline.apply(&mut frame);
		let bones = &frame.body.as_ref().unwrap().humanoid.as_ref().unwrap().bones;
		assert_eq!(bones[0].bone, HumanoidBone::RightHand);
		// SwapSides では transform は触らない (translation.x は元のまま)
		assert!((bones[0].transform.translation.unwrap().x - 0.4).abs() < 1e-5);
		// rotation も不変
		let rot = bones[0].transform.rotation.unwrap();
		assert!((rot.y - 0.5).abs() < 1e-5);
		assert!((rot.z - 0.3).abs() < 1e-5);
	}

	#[test]
	fn mirror_output_skips_disabled_bones_via_subset() {
		// hands_enabled=false なら mirror も skip。後段 BoneFilter で消える。
		let mut frame = make_lr_bone_frame();
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			mirror: MirrorConfig {
				mode: MirrorMode::MirrorOutput,
			},
			hands_enabled: false,
			..ModifierConfig::default()
		});
		pipeline.apply(&mut frame);
		let bones = &frame.body.as_ref().unwrap().humanoid.as_ref().unwrap().bones;
		// hands_enabled=false なので LeftHand は filter で除外される。残るのは Head のみ。
		assert_eq!(bones.len(), 1);
		assert_eq!(bones[0].bone, HumanoidBone::Head);
	}

	#[test]
	fn mirror_output_swaps_expression_left_right_suffix() {
		let mut frame = UNMotionFrame::new(1);
		frame.face = Some(FaceMotion {
			tracking_state: TrackingState::Valid,
			confidence: 1.0,
			head: None,
			expressions: vec![
				ExpressionSample {
					name: "eyeBlinkLeft".to_string(),
					value: 0.7,
					confidence: 1.0,
					source_index: Some(0),
					state: SampleState::Valid,
				},
				ExpressionSample {
					name: "mouthSmileRight".to_string(),
					value: 0.3,
					confidence: 1.0,
					source_index: Some(1),
					state: SampleState::Valid,
				},
				ExpressionSample {
					name: "Blink_L".to_string(),
					value: 0.5,
					confidence: 1.0,
					source_index: Some(2),
					state: SampleState::Valid,
				},
				ExpressionSample {
					name: "jawOpen".to_string(),
					value: 0.2,
					confidence: 1.0,
					source_index: Some(3),
					state: SampleState::Valid,
				},
			],
		});
		let mut pipeline = ModifierPipeline::from_config(&ModifierConfig {
			mirror: MirrorConfig {
				mode: MirrorMode::MirrorOutput,
			},
			..ModifierConfig::default()
		});
		pipeline.apply(&mut frame);
		let names: Vec<_> = frame.face.as_ref().unwrap().expressions.iter().map(|e| e.name.as_str()).collect();
		assert_eq!(names, vec!["eyeBlinkRight", "mouthSmileLeft", "Blink_R", "jawOpen"]);
	}

	#[test]
	fn mirror_helper_swap_left_right_suffix_only() {
		assert_eq!(swap_left_right_in_expression_name("eyeBlinkLeft"), "eyeBlinkRight");
		assert_eq!(swap_left_right_in_expression_name("eyeBlinkRight"), "eyeBlinkLeft");
		assert_eq!(swap_left_right_in_expression_name("Blink_L"), "Blink_R");
		assert_eq!(swap_left_right_in_expression_name("Blink_R"), "Blink_L");
		// 末尾 suffix にマッチしないものは無変更
		assert_eq!(swap_left_right_in_expression_name("jawOpen"), "jawOpen");
		assert_eq!(swap_left_right_in_expression_name("leftEye"), "leftEye");
	}

	#[test]
	fn quat_slerp_handles_shortest_path() {
		// dot < 0 (反対側) のケースで shortest path を取ること。
		let a = FrameQuat {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: 1.0,
		};
		let b = FrameQuat {
			x: 0.0,
			y: 0.0,
			z: 0.0,
			w: -1.0,
		};
		let mid = quat_slerp(a, b, 0.5);
		// 中点は identity (or その negate) 付近のはず
		let norm = (mid.x * mid.x + mid.y * mid.y + mid.z * mid.z + mid.w * mid.w).sqrt();
		assert!((norm - 1.0).abs() < 1e-4);
	}

	#[test]
	fn modifier_config_is_pass_through_reports_correctly() {
		assert!(ModifierConfig::default().is_pass_through());
		let with_filter = ModifierConfig {
			hands_enabled: false,
			..ModifierConfig::default()
		};
		assert!(!with_filter.is_pass_through());
		let with_smoothing = ModifierConfig {
			smoothing: SmoothingConfig {
				preset: SmoothingPreset::Medium,
				..SmoothingConfig::default()
			},
			..ModifierConfig::default()
		};
		assert!(!with_smoothing.is_pass_through());
		let with_mirror = ModifierConfig {
			mirror: MirrorConfig {
				mode: MirrorMode::MirrorOutput,
			},
			..ModifierConfig::default()
		};
		assert!(!with_mirror.is_pass_through());
	}
}
