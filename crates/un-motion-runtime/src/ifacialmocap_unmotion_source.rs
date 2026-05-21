//! Phase E-α-7: iFacialMocap 受信エンジン (`MotionFrameSource` trait 実装)。
//!
//! iFacialMocap (iPhone / iPad アプリ) が UDP で送出する face / eye / blendshape
//! データを `un_motion_input_ifacialmocap::IfacialMocapInputSource` でデコードし、
//! `IfacialMocapFrame` → `UNMotionFrame` 変換を行って `MotionFrameStreamWorker` に渡す。
//!
//! これにより iFacialMocap 入力も Capturer 正式経路
//! `Input → Engine(decoder) → UNMotionFrame → Modifier → Output` に乗る。Engine Type
//! (`runtime_selection.engine`) が `"ifacialmocap"` のときに選択される。
//!
//! # データフロー
//!
//! ```text
//! iFacialMocap UDP listen → IfacialMocapInputSource (decode text/| 区切り)
//!                              ↓ IfacialMocapFrame { head, left_eye, right_eye, expressions }
//! IfacialMocapFrame::to_unmotion_frame()
//!                              ↓ UNMotionFrame { body.head, eyes, face.expressions, signals }
//! IfacialMocapUnmotionSource::next_frame() → UNMotionFrame → Modifier → [UNMF/Z, VMC/UDP]
//! ```
//!
//! # VMC との違い
//!
//! VMC は 1 論理フレームを 4 つの OSC bundle (Bone / Val ×2 / Apply) に分割して
//! 送ってくるため accumulator が必要 (`VmcUnmotionSource` 参照)。一方
//! iFacialMocap は 1 UDP datagram = 1 完全な frame (改行ベースの text プロトコル)
//! なので accumulator は不要で、batch 内の最新 frame を返すだけで十分。
//!
//! # スコープ
//!
//! - **対応 transport**: UDP listen のみ。Phase E redesign の "Engine Type = ifacialmocap"
//!   は iPhone/iPad アプリの最も一般的なセットアップ (PC 側で UDP を listen) に絞る。
//! - **出力経路**: Capturer ごとに 1 つの iFacialMocap input を `UNMotionFrame` に正規化し、
//!   Modifier → [UNMF/Z, VMC] 出力構成に乗せる。複数の face source を同居させたい場合は
//!   Capturer を分け、UN Avatar の Producer 同名重複ルールで対応する。

use std::net::SocketAddr;
use std::sync::atomic::Ordering;

use anyhow::Context;
use tracing::{info, warn};
use un_motion_frame::UNMotionFrame;
use un_motion_input_ifacialmocap::{IfacialMocapInputConfig, IfacialMocapInputSource, IfacialMocapTransport};

use crate::{MotionFrameSource, SourceTelemetryHandle};

/// iFacialMocap UDP 受信を `MotionFrameSource` として提供する。
///
/// `next_frame()` は 1 poll サイクルあたり最大 1 frame の最新値を返す。
/// 1 batch に複数 frame が入っていた場合は最新 (= 末尾) を採用し、
/// 中間の古い frame は捨てる (毎フレーム全 channel を上書きする protocol なので
/// 中間値は実用上不要)。中間 frame の数は `frames_emitted` には含めず
/// `bundles_merged` (= iFacialMocap では受信 frame 総数) として記録する。
///
/// # Telemetry counters
///
/// `SourceStageAtomics` を共有しており Capturer から `load(Relaxed)` で読める。
/// - `raw_received`     — iFacialMocap UDP datagrams 受信総数 (decode 失敗込み)
/// - `bundles_merged`   — 受信した IfacialMocapFrame 総数 (decode 成功)
/// - `frames_emitted`   — `next_frame()` で UNMotionFrame として下流に流した総数
/// - `decode_errors`    — テキスト/区切りパースで失敗した datagram 総数
/// - `non_vmc_dropped`  — iFacialMocap には該当無し (常に 0)
pub struct IfacialMocapUnmotionSource {
	source: IfacialMocapInputSource,
	source_id: String,
	sequence: u64,
	telemetry: SourceTelemetryHandle,
	announced_first_datagram: bool,
	announced_first_frame: bool,
}

impl IfacialMocapUnmotionSource {
	/// iFacialMocap UDP listener を bind し、ソースを構築する。
	///
	/// `listen_addr` は `0.0.0.0:49983` (iFacialMocap UDP デフォルトポート)
	/// 形式で指定する。iPhone/iPad 側はネットワーク経由で当該 PC の IP + port
	/// 49983 に送信するよう設定する必要がある。
	///
	/// 失敗ケース: bind 失敗 (port 競合、permission denied)、socket 設定失敗。
	pub fn bind(source_id: impl Into<String>, listen_addr: SocketAddr) -> anyhow::Result<Self> {
		let source_id = source_id.into();
		let config = IfacialMocapInputConfig {
			source_id: source_id.clone(),
			bind_addr: listen_addr,
			remote_addr: None,
			transport: IfacialMocapTransport::Udp,
			// iFacialMocap iOS app が「PC 側からの 'iFacialMocap_sahne' を受け取って
			// 自動で配信を開始する」プロトコル拡張がある (公式 docs 記載)。
			// この経路は基本的に「iPhone 側で送信先 IP/port を手動設定する運用」を想定する
			// ので start_command は送らない (送信先 IP を知らずに broadcast すると
			// 周辺機器に予期せぬ packet を撒く副作用がある)。
			start_command: None,
		};
		let source =
			IfacialMocapInputSource::bind(config).with_context(|| format!("iFacialMocap receive engine bind failed: {listen_addr}"))?;
		let local = source.local_addr().ok();
		info!(
			target: "un_motion_runtime::ifacialmocap_unmotion_source",
			source_id = %source_id,
			requested = %listen_addr,
			bound = ?local,
			"iFacialMocap receive engine bound and listening (waiting for iPhone/iPad iFacialMocap app to send)",
		);
		Ok(Self {
			source,
			source_id: source_id.clone(),
			sequence: 0,
			telemetry: SourceTelemetryHandle::new("ifacialmocap-receive", source_id),
			announced_first_datagram: false,
			announced_first_frame: false,
		})
	}

	pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
		self.source.local_addr()
	}

	pub fn source_id(&self) -> &str {
		&self.source_id
	}

	pub fn telemetry_handle(&self) -> SourceTelemetryHandle {
		self.telemetry.clone()
	}
}

impl MotionFrameSource for IfacialMocapUnmotionSource {
	fn next_frame(&mut self) -> anyhow::Result<Option<UNMotionFrame>> {
		let batch = self.source.poll_batch()?;
		const RECV_LOG_INTERVAL: u64 = 300;
		const FRAME_LOG_INTERVAL: u64 = 300;

		let decoded = batch.frames.len() as u64;
		let received_total = decoded.saturating_add(batch.decode_errors);

		if received_total > 0 {
			let before = self.telemetry.atomics.raw_received.fetch_add(received_total, Ordering::Relaxed);
			let after = before.saturating_add(received_total);
			if !self.announced_first_datagram {
				self.announced_first_datagram = true;
				info!(
					target: "un_motion_runtime::ifacialmocap_unmotion_source",
					source_id = %self.source_id,
					received_datagrams = received_total,
					decoded_frames = decoded,
					decode_errors = batch.decode_errors,
					"iFacialMocap receive engine received first inbound UDP datagram",
				);
			}
			if after / RECV_LOG_INTERVAL > before / RECV_LOG_INTERVAL {
				let snap = self.telemetry.atomics.snapshot();
				info!(
					target: "un_motion_runtime::ifacialmocap_unmotion_source",
					source_id = %self.source_id,
					total_received = snap.raw_received,
					total_frames_emitted = snap.frames_emitted,
					total_bundles_merged = snap.bundles_merged,
					total_decode_errors = snap.decode_errors,
					"iFacialMocap receive engine cumulative receive counters",
				);
			}
		}

		if batch.decode_errors > 0 {
			self.telemetry
				.atomics
				.decode_errors
				.fetch_add(batch.decode_errors, Ordering::Relaxed);
			for example in batch.decode_error_examples.iter() {
				warn!(
					target: "un_motion_runtime::ifacialmocap_unmotion_source",
					source_id = %self.source_id,
					%example,
					"iFacialMocap decode error (datagram discarded)",
				);
			}
		}

		if decoded > 0 {
			self.telemetry.atomics.bundles_merged.fetch_add(decoded, Ordering::Relaxed);
		}

		// batch 内に複数 frame が積まれていた場合は最新 (末尾) を採用。
		// iFacialMocap は frame 全体を毎 datagram で送り直してくる protocol
		// なので中間 frame は捨てて差し支えない。
		let Some(latest) = batch.frames.into_iter().last() else {
			return Ok(None);
		};

		let unmotion = latest.to_unmotion_frame(self.sequence);
		let before_frames = self.telemetry.atomics.frames_emitted.fetch_add(1, Ordering::Relaxed);
		let after_frames = before_frames.saturating_add(1);
		if !self.announced_first_frame {
			self.announced_first_frame = true;
			let snap = self.telemetry.atomics.snapshot();
			info!(
				target: "un_motion_runtime::ifacialmocap_unmotion_source",
				source_id = %self.source_id,
				bundles_merged = snap.bundles_merged,
				expressions = latest.expressions.len(),
				"iFacialMocap receive engine produced first UNMotionFrame from received face packet",
			);
		}
		if after_frames / FRAME_LOG_INTERVAL > before_frames / FRAME_LOG_INTERVAL {
			let snap = self.telemetry.atomics.snapshot();
			info!(
				target: "un_motion_runtime::ifacialmocap_unmotion_source",
				source_id = %self.source_id,
				total_frames_emitted = snap.frames_emitted,
				total_bundles_merged = snap.bundles_merged,
				"iFacialMocap receive engine cumulative frame emission",
			);
		}
		self.sequence = self.sequence.saturating_add(1);
		Ok(Some(unmotion))
	}

	fn telemetry_handle(&self) -> Option<SourceTelemetryHandle> {
		Some(self.telemetry.clone())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::net::UdpSocket;
	use std::thread;
	use std::time::Duration;

	/// 1 つの iFacialMocap UDP datagram を送信したら、`next_frame()` が
	/// `Some(UNMotionFrame)` を返し、`face.expressions` に `eyeBlinkLeft` が
	/// 入っていることを確認する。
	#[test]
	fn next_frame_returns_unmotion_frame_for_single_datagram() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
		let mut source = IfacialMocapUnmotionSource::bind("ifacialmocap-test", listen_addr).expect("bind");
		let target = source.local_addr().expect("local addr");
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender");

		// iFacialMocap protocol: `|` 区切りの ASCII。先頭の `head#yaw,pitch,roll` ＋
		// 各 blendshape は `name#value` の形式。
		let datagram = b"head#10.0,-5.0,2.0|leftEye#1.0,-0.5,0.0|rightEye#1.0,-0.5,0.0|eyeBlinkLeft#0.42|confidence#0.95";
		sender.send_to(datagram, target).expect("send");
		thread::sleep(Duration::from_millis(20));

		let frame = source.next_frame().expect("poll").expect("frame should be produced");
		let face = frame.face.as_ref().expect("face motion populated");
		let blink = face
			.expressions
			.iter()
			.find(|e| e.name == "eyeBlinkLeft")
			.expect("eyeBlinkLeft expression");
		assert!((blink.value - 0.42).abs() < 1e-4, "blink.value = {}", blink.value);

		let snap = source.telemetry.atomics.snapshot();
		assert_eq!(snap.raw_received, 1, "raw_received = {}", snap.raw_received);
		assert_eq!(snap.bundles_merged, 1, "bundles_merged = {}", snap.bundles_merged);
		assert_eq!(snap.frames_emitted, 1, "frames_emitted = {}", snap.frames_emitted);
		assert_eq!(snap.decode_errors, 0);
	}

	/// 何も送信していない状態で `next_frame()` を呼ぶと、`Ok(None)` を返し
	/// counter は変動しない (idle tick での重複 frame emit を起こさない)。
	#[test]
	fn next_frame_returns_none_when_no_datagram_in_flight() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
		let mut source = IfacialMocapUnmotionSource::bind("ifacialmocap-test-idle", listen_addr).expect("bind");
		assert!(source.next_frame().expect("poll").is_none());

		let snap = source.telemetry.atomics.snapshot();
		assert_eq!(snap.raw_received, 0);
		assert_eq!(snap.frames_emitted, 0);
		assert_eq!(snap.decode_errors, 0);
	}

	/// batch 内に複数 datagram が積まれていても `next_frame()` は 1 frame だけ
	/// emit する (最新を採用)。`bundles_merged` は受信総数を反映する。
	#[test]
	fn next_frame_emits_latest_when_multiple_datagrams_queued() {
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
		let mut source = IfacialMocapUnmotionSource::bind("ifacialmocap-test-multi", listen_addr).expect("bind");
		let target = source.local_addr().expect("local addr");
		let sender = UdpSocket::bind("127.0.0.1:0").expect("sender");

		// iFacialMocap の parser は head / leftEye / rightEye 全てが必須なので
		// `parse_ifacialmocap_frame` が `Err("leftEye field is missing")` 等を
		// 返さないよう、テスト用 datagram にも全 field を入れる。
		// 1 つめの datagram: eyeBlinkLeft=0.1
		sender
			.send_to(
				b"head#0.0,0.0,0.0|leftEye#0.0,0.0,0.0|rightEye#0.0,0.0,0.0|eyeBlinkLeft#0.10|confidence#0.9",
				target,
			)
			.expect("send 1");
		// 2 つめの datagram: eyeBlinkLeft=0.9 (これが最新で採用される)
		sender
			.send_to(
				b"head#0.0,0.0,0.0|leftEye#0.0,0.0,0.0|rightEye#0.0,0.0,0.0|eyeBlinkLeft#0.90|confidence#0.9",
				target,
			)
			.expect("send 2");
		thread::sleep(Duration::from_millis(40));

		let frame = source.next_frame().expect("poll").expect("frame should be produced");
		let face = frame.face.as_ref().expect("face populated");
		let blink = face.expressions.iter().find(|e| e.name == "eyeBlinkLeft").expect("blink");
		assert!((blink.value - 0.90).abs() < 1e-4, "latest datagram should win; got {}", blink.value);

		let snap = source.telemetry.atomics.snapshot();
		assert_eq!(snap.raw_received, 2);
		assert_eq!(snap.bundles_merged, 2);
		// 複数 datagram を 1 つの emit にまとめるので frames_emitted = 1。
		assert_eq!(snap.frames_emitted, 1);
	}

	/// `telemetry_handle()` (trait method) は `Some(handle)` を返し、同じ
	/// `Arc<SourceStageAtomics>` を指す (= Capturer 側 snapshot と
	/// 一致する).
	#[test]
	fn telemetry_handle_exposes_same_atomics_as_internal_writer() {
		use std::sync::Arc;
		let listen_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
		let source = IfacialMocapUnmotionSource::bind("ifacialmocap-test-tlm", listen_addr).expect("bind");
		let handle = MotionFrameSource::telemetry_handle(&source).expect("Some");
		assert!(Arc::ptr_eq(&handle.atomics, &source.telemetry.atomics));
	}
}
