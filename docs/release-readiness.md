# v1 リリース準備

この文書は v1 のリリース境界を記録する。リリース直前に研究課題や大きな設計変更を再開しないための基準。

## プロダクト境界

- 対象 OS: Windows 10/11 x86_64。
- 主 runtime: `un-motion-supervisor` が `un-motion-capturer` を起動する構成。
- 正式経路:

```text
Input -> Engine(MediaPipe Native + post-process) -> UNMotionFrame -> Modifier -> Output(UNMF/Z, VMC/UDP, VRC (VRCFT) / OSC)
```

- `UNMotionFrame` が内部 frame 契約。
- `UNMF/Z`、`VMC/UDP`、`VRC (VRCFT) / OSC` は、同じ Modifier 適用後 frame から出る output transport。
- `VRC (VRCFT) / OSC` は issue-4 / v1.1.0 では Face output のみを完了条件とする。姿勢や tracker は対象外。
- Output が MediaPipe raw output や post-process 専用データを直接読む経路は v1 対象外。

## v1 既定値

- Webcam backend: Windows では DirectShow (`ccap-rs`) を実用上の既定にする。
- MediaFoundation (`nokhwa`) は device 互換性と将来の Windows API 変化に備え、UI から選べる代替 backend として残す。ただし Windows のリリース品質は DirectShow を基準に判断する。
- MediaPipe Native delegate: XNNPACK が既定、CPU は fallback。
- MediaPipe Native GPU delegate: Windows v1 では未対応。Supervisor UI に出さない。
- 新規 profile の Filter 既定: Head / Face / Hands / Arms / Torso は on、Legs / Feet は off。
- 新規 profile の Output 既定: UNMF/Z は on、VMC/UDP と VRC (VRCFT) / OSC は off。
- Smoothing 既定: 弱い One Euro は on、弱い EMA は設定を持つが off。

## リリース前チェック

リポジトリ全体の検証経路を実行する。

```sh
cargo xtask verify
```

Native MediaPipe を含む package readiness では、native build artifact と model が揃っていることも確認する。

```sh
cargo xtask mediapipe build-native --skip-fetch
cargo xtask make-release-package --version <version>
```

配布物の license readiness も確認する。

```sh
cargo xtask license-report
```

- `THIRD_PARTY_NOTICES.md` と `LICENSES/` が package に含まれている。
- package root に `THIRD_PARTY_DEPENDENCIES.md` が生成され、同梱されている。
- `models/*.task` を同梱する場合、MediaPipe / Apache-2.0 notice が含まれている。
- `opencv_world3410.dll` を同梱する場合、OpenCV 3.4.10 / BSD-3-Clause notice が含まれている。
- `third_party/ccap-rs` を source distribution に含める場合、ccap-rs / MIT notice が含まれている。
- `THIRD_PARTY_DEPENDENCIES.md` で `UNKNOWN` や強い copyleft がないか release 前に再確認する。

実機カメラ profile で motion output を確認する。

- Input FPS が選択した camera setting に近い。
- camera と CPU が許す状況で Engine FPS が 30 または 60 に固定されていない。
- UNMF/Z output FPS が Engine または明示した Output FPS limit に追従する。
- VMC/UDP を有効化した場合、UNMF/Z と同じ Modifier 適用後 body subset が届く。

## v1 以降へ送るもの

- motion level の仮想 collider / collision guard。
- DWT image prefiltering、adaptive Kalman、学習型 landmark stabilizer。
- 下半身の walking / root reconstruction。
- Windows MediaFoundation capture を DirectShow の release-quality replacement として仕上げる作業。
- Windows MediaPipe GPU delegate support。
