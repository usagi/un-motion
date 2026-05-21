use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};
use un_motion_interfaces::{ImageFrame, ImageInputSource};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebcamDeviceInfo {
	pub id: String,
	pub name: String,
}

/// 1 つの `CameraFormat` 相当の情報 (resolution + fps + pixel format)。
/// MediaFoundation (Windows) / AVFoundation (macOS) / V4L2 (Linux) いずれの
/// backend でも nokhwa の `Camera::compatible_camera_formats()` から取れる
/// 共通 view。DirectShow 側の `DirectShowCaptureFormatInfo` と並ぶ Phase 4b
/// 形式列挙の戻り型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebcamFormatInfo {
	pub width: u32,
	pub height: u32,
	pub fps: Option<u32>,
	/// 例: `"YUYV"`, `"MJPEG"`, `"NV12"`, `"GRAY"`, `"RAWRGB"`。
	/// nokhwa の `FrameFormat::Debug` がそのまま `"MJPEG"` のような短い文字列を
	/// 返すのでそれを採用する。
	pub pixel_format: String,
}

impl WebcamFormatInfo {
	pub fn native_label(&self) -> String {
		match self.fps {
			Some(fps) => format!("{}x{}@{} {}", self.width, self.height, fps, self.pixel_format),
			None => format!("{}x{} {}", self.width, self.height, self.pixel_format),
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebcamDiagnosticReport {
	pub backend: &'static str,
	pub device_count: usize,
	pub devices: Vec<WebcamDeviceInfo>,
}

impl WebcamDiagnosticReport {
	pub fn to_text_lines(&self) -> Vec<String> {
		let mut lines = vec![format!("{} webcam devices: {}", self.backend, self.device_count)];
		for device in &self.devices {
			lines.push(format!("- {} ({})", device.name, device.id));
		}
		lines
	}
}

pub trait WebcamCaptureBackend {
	fn list_devices(&mut self) -> anyhow::Result<Vec<WebcamDeviceInfo>>;
	fn capture_next_image(&mut self, device_id: &str) -> anyhow::Result<Option<ImageFrame>>;
}

#[derive(Default)]
pub struct NokhwaWebcamBackend;

impl WebcamCaptureBackend for NokhwaWebcamBackend {
	fn list_devices(&mut self) -> anyhow::Result<Vec<WebcamDeviceInfo>> {
		let cameras = nokhwa::query(ApiBackend::Auto)?;
		Ok(cameras
			.into_iter()
			.enumerate()
			.map(|(fallback_index, camera)| {
				let id = match camera.index() {
					nokhwa::utils::CameraIndex::Index(index) => format!("cam{index}"),
					nokhwa::utils::CameraIndex::String(value) if value.trim().is_empty() => format!("cam{fallback_index}"),
					nokhwa::utils::CameraIndex::String(value) => format!("cam:{value}"),
				};
				WebcamDeviceInfo {
					id,
					name: camera.human_name(),
				}
			})
			.collect())
	}

	fn capture_next_image(&mut self, _device_id: &str) -> anyhow::Result<Option<ImageFrame>> {
		Ok(None)
	}
}

pub fn diagnose_devices<B: WebcamCaptureBackend>(backend: &mut B) -> anyhow::Result<WebcamDiagnosticReport> {
	let devices = backend.list_devices()?;
	Ok(WebcamDiagnosticReport {
		backend: "nokhwa",
		device_count: devices.len(),
		devices,
	})
}

/// MediaFoundation (Windows) / AVFoundation (macOS) / V4L2 (Linux) ベースの Webcam
/// デバイス一覧を返す Supervisor GUI 向け公開 API。
///
/// GUI 上は "Webcam (MediaFoundation)" として表示し、`nokhwa` という実装詳細を
/// ユーザーに見せない (ユーザー要望: 「nokhwa を使っているかどうかはユーザーには
/// 本質情報ではない」)。
pub fn list_mediafoundation_devices() -> anyhow::Result<Vec<WebcamDeviceInfo>> {
	let mut backend = NokhwaWebcamBackend;
	backend.list_devices()
}

/// 与えた `device_id_or_name` で識別される MediaFoundation (Windows) / AVFoundation
/// (macOS) / V4L2 (Linux) ベースの Webcam が報告する `CameraFormat` 一覧を返す。
///
/// `device_id_or_name` は次のいずれかとして解釈される:
/// * `list_mediafoundation_devices()` が生成する `id` (`"cam0"` のような `"cam"`
///   + 整数 → `CameraIndex::Index(n)` / `"cam:..."` → `CameraIndex::String(...)`)
/// * 上記いずれにも一致しない場合は、デバイス一覧から `name` で部分一致を試みる
///   (Supervisor 側で device の `label` で投げ込んでも動くようにするため)。
///
/// # 注意点
///
/// `compatible_camera_formats()` は `Camera::new` 経由で実デバイスを一時的に
/// 開く必要があるため、別アプリ (Discord / OBS / iFacialMocap 等) がカメラを
/// 占有していると `NokhwaError` で失敗する。これは正常な動作 (= "排他デバイスを
/// 列挙できない") なので、GUI 側は warn / hint メッセージで表示することを想定。
///
/// experimental: nokhwa 0.10 の MSMF backend は一部の仮想カメラ
/// (OBS Virtual Camera など) で 0 件を返す既知の癖がある。Phase 4b はあくまで
/// GUI の dropdown を埋める目的で導入しており、空 vec が返って来る分には致命的
/// ではない (フリーフォームの width/height/fps 入力 UI に fallback できる)。
pub fn list_mediafoundation_capture_formats(device_id_or_name: &str) -> anyhow::Result<Vec<WebcamFormatInfo>> {
	let index = resolve_camera_index(device_id_or_name)?;
	// `RequestedFormatType::None` は「とりあえず開けたら何でもいい」相当。
	// fulfill しないで `compatible_camera_formats()` を呼ぶだけなので、最後に
	// fps の高い RGB 形式を取りに行く `AbsoluteHighestFrameRate` でも結果は同じ。
	let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
	let mut camera = nokhwa::Camera::new(index, format).map_err(|error| anyhow::anyhow!("Camera::new failed: {error}"))?;
	let formats = camera
		.compatible_camera_formats()
		.map_err(|error| anyhow::anyhow!("Camera::compatible_camera_formats failed: {error}"))?;
	let mut out: Vec<WebcamFormatInfo> = formats
		.into_iter()
		.map(|f| WebcamFormatInfo {
			width: f.width(),
			height: f.height(),
			// nokhwa の frame_rate() は u32 を返すが、0 を「fps 未報告」とみなす
			// (一部 backend / 仮想カメラで報告されないことがある)。
			fps: Some(f.frame_rate()).filter(|fps| *fps > 0),
			pixel_format: format!("{:?}", f.format()),
		})
		.collect();
	// GUI 表示では width 降順 → height 降順 → fps 降順の順がもっとも分かりやすい。
	// 同じ resolution の中で fps が大→小に並ぶと「最大スペック」を上から拾える。
	out.sort_by(|a, b| {
		b.width
			.cmp(&a.width)
			.then(b.height.cmp(&a.height))
			.then(b.fps.unwrap_or(0).cmp(&a.fps.unwrap_or(0)))
			.then(a.pixel_format.cmp(&b.pixel_format))
	});
	// 完全重複 (width, height, fps, pixel_format) は除く。
	out.dedup_by(|a, b| a.width == b.width && a.height == b.height && a.fps == b.fps && a.pixel_format == b.pixel_format);
	Ok(out)
}

/// `device_id_or_name` → `CameraIndex` の解決。
///
/// 1. `"cam<digits>"` (例: `"cam0"`) → `CameraIndex::Index(digits.parse())`
/// 2. `"cam:..."` → `CameraIndex::String("...")`
/// 3. それ以外 → `list_mediafoundation_devices()` を引いて `name` の部分一致 →
///    マッチしたデバイスの `id` に対して再帰呼び出し。
/// 4. デバイス一覧が空 / マッチしないときは `0` を試行する (= 最初のデバイス)。
fn resolve_camera_index(device_id_or_name: &str) -> anyhow::Result<CameraIndex> {
	let trimmed = device_id_or_name.trim();
	if trimmed.is_empty() {
		return Ok(CameraIndex::Index(0));
	}
	if let Some(rest) = trimmed.strip_prefix("cam:") {
		return Ok(CameraIndex::String(rest.to_string()));
	}
	if let Some(rest) = trimmed.strip_prefix("cam") {
		if let Ok(index) = rest.parse::<u32>() {
			return Ok(CameraIndex::Index(index));
		}
	}
	// name based fuzzy match (case-insensitive substring match).
	let lower = trimmed.to_ascii_lowercase();
	let devices = list_mediafoundation_devices().unwrap_or_default();
	if let Some(found) = devices
		.iter()
		.find(|d| d.name.to_ascii_lowercase().contains(&lower) || d.id == trimmed)
	{
		return resolve_camera_index(&found.id);
	}
	Ok(CameraIndex::Index(0))
}

pub struct WebcamImageInputSource<B: WebcamCaptureBackend> {
	backend: B,
	device_id: String,
}

impl<B: WebcamCaptureBackend> WebcamImageInputSource<B> {
	pub fn new(backend: B, device_id: impl Into<String>) -> Self {
		Self {
			backend,
			device_id: device_id.into(),
		}
	}
}

impl<B: WebcamCaptureBackend> ImageInputSource for WebcamImageInputSource<B> {
	fn next_image_frame(&mut self) -> anyhow::Result<Option<ImageFrame>> {
		self.backend.capture_next_image(&self.device_id)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Default)]
	struct MockBackend {
		devices: Vec<WebcamDeviceInfo>,
	}

	impl WebcamCaptureBackend for MockBackend {
		fn list_devices(&mut self) -> anyhow::Result<Vec<WebcamDeviceInfo>> {
			Ok(self.devices.clone())
		}

		fn capture_next_image(&mut self, _device_id: &str) -> anyhow::Result<Option<ImageFrame>> {
			Ok(None)
		}
	}

	#[test]
	fn diagnostic_report_renders_nokhwa_devices() {
		let mut backend = MockBackend {
			devices: vec![WebcamDeviceInfo {
				id: "cam0".to_string(),
				name: "Camera".to_string(),
			}],
		};
		let report = diagnose_devices(&mut backend).unwrap();
		assert_eq!(report.device_count, 1);
		assert_eq!(report.to_text_lines()[0], "nokhwa webcam devices: 1");
	}

	#[test]
	fn webcam_format_info_native_label_includes_fps_when_present() {
		let info = WebcamFormatInfo {
			width: 1280,
			height: 720,
			fps: Some(30),
			pixel_format: "MJPEG".to_string(),
		};
		assert_eq!(info.native_label(), "1280x720@30 MJPEG");
	}

	#[test]
	fn webcam_format_info_native_label_omits_fps_when_none() {
		let info = WebcamFormatInfo {
			width: 1920,
			height: 1080,
			fps: None,
			pixel_format: "NV12".to_string(),
		};
		assert_eq!(info.native_label(), "1920x1080 NV12");
	}

	/// `resolve_camera_index` の文字列パース部分のみ単体テスト。
	/// "cam<n>" / "cam:..." / 空文字列を直接決定論的に処理する経路を確認する。
	/// name fuzzy match は実デバイス依存になるのでテスト対象外。
	#[test]
	fn resolve_camera_index_parses_id_formats() {
		// cam<digits> → Index(n)
		match resolve_camera_index("cam0").unwrap() {
			CameraIndex::Index(n) => assert_eq!(n, 0),
			other => panic!("unexpected {other:?}"),
		}
		match resolve_camera_index("cam7").unwrap() {
			CameraIndex::Index(n) => assert_eq!(n, 7),
			other => panic!("unexpected {other:?}"),
		}
		// "cam:foo" → String("foo")
		match resolve_camera_index("cam:OBS Virtual Camera").unwrap() {
			CameraIndex::String(s) => assert_eq!(s, "OBS Virtual Camera"),
			other => panic!("unexpected {other:?}"),
		}
		// 空文字列 → Index(0) fallback
		match resolve_camera_index("").unwrap() {
			CameraIndex::Index(n) => assert_eq!(n, 0),
			other => panic!("unexpected {other:?}"),
		}
	}
}
