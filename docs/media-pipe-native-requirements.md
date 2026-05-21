# MediaPipe Native 要件

この文書は U.N. Motion 側で持つ MediaPipe Native DLL 経路の契約です。

U.N. Motion repo が所有するものは ABI、loader、最小 C++ bridge、pin file、xtask build surface、docs です。MediaPipe source checkout、Bazel cache、model、生成 DLL は local artifact とし、明示的な配布判断がない限り Git へ入れません。

## 役割

- U.N. Motion の主な高速経路は Tauri desktop pipeline から MediaPipe Native DLL を使う形です。
- MediaPipe Web は Capturer runtime path には含めません。
- desktop app は build 済み DLL を runtime に読み込みます。配布 package が整えば、ユーザー側に Bazel や MediaPipe source checkout は不要です。
- CLI probe は batch 比較や signal inspection 用の開発者ツールとして残します。

## 必要な成果物

local build command は次を用意します。

- `native/mediapipe/un-motion-mediapipe.dll`
- `models/pose_landmarker_lite.task`
- `models/hand_landmarker.task`

Rust ABI と dynamic loader は `crates/un-motion-mediapipe-native` にあります。DLL は repository root から既定 model path で動作し、別 process から使う場合は model environment variable で path を指定できるようにします。

## 必須 C ABI

DLL は次を export します。

- `void* un_motion_mediapipe_create(void)`
- `void un_motion_mediapipe_destroy(void* context)`
- `int32_t un_motion_mediapipe_process_rgb(...)`
- `int32_t un_motion_mediapipe_process_rgb_pose_and_hands(...)`

`un_motion_mediapipe_process_rgb_pose_and_hands` の要件:

- tightly packed RGB888 input を受け取る。
- 入力 frame を 1 回だけ作り、pose / hands をその frame から実行する。
- 最大 2 hands を扱う。
- 検出 hand ごとに 21 個の normalized landmark を返す。
- handedness は `0 = Left`、`1 = Right`、`255 = unknown`。
- pose が失敗しても hand が成功している場合は hand output を埋め、pose error も返す。

U.N. Motion probe が使う現在の return code:

- `0`: requested branch success
- `10`: invalid arguments
- `11`: missing backend / context branch
- `13`: pose detection failed
- `20`: no pose landmarks
- `21`: insufficient pose landmarks
- `52`: hand detection failed

## 環境変数

DLL は次を扱えるようにします。

- `UN_MOTION_MEDIAPIPE_MODEL`
- `UN_MOTION_MEDIAPIPE_HAND_MODEL`
- `UN_MOTION_MEDIAPIPE_QUIET`

`UN_MOTION_MEDIAPIPE_QUIET` は TensorFlow Lite / MediaPipe の informational stderr を抑制するためのものです。正しさには必須ではありませんが、CLI JSON summary や regression log を読みやすくします。

probe は既定の repository-local path でも動く必要があります。

- `models/pose_landmarker_lite.task`
- `models/hand_landmarker.task`

## smoke test の期待値

静止画回帰確認は Git 管理されている `tests/pose` fixture を使います。

```sh
cargo xtask mediapipe pose-fixtures
```

特に次を確認します。

- `pose-7.png`: 両手の手の平が camera 側を向く。
- `pose-9.png`: 両手の手の平が後ろ側を向く。
- `pose-T-wrist-front.png`: T-wrist-front calibration 用の手首・手の向きが破綻しない。
- `pose-U.png` / `pose-I.png` / `pose-T.png`: calibration 姿勢の head と body が安定する。

VIDEO / LIVE_STREAM smoke test では、同じ静止画像を繰り返し入力する test を含めます。安定した still image で hand や head rotation が大きく frame-to-frame jump してはいけません。

既知の Windows failure mode は MediaPipe FrameBuffer ROI conversion です。OpenCV を無効化した場合、upstream FrameBuffer conversion が 90 度単位の rotation しか扱えず、VIDEO / LIVE tracker の任意角度 ROI で破綻します。`native/mediapipe/patches/image_to_tensor_converter_frame_buffer.cc` の U.N. Motion vendor patch は native VIDEO / LIVE のために有効である必要があります。

## Native build の管理範囲

- Native fetch / build / probe orchestration は PowerShell entrypoint ではなく `xtask` の command surface に置きます。
- `cargo xtask mediapipe build-native` は `native/mediapipe/mediapipe-pin.toml` を読みます。
- MediaPipe source は ignored path の `third_party/mediapipe` に用意します。
- downloaded model、Bazelisk、Python shim、生成 DLL、import library は ignored local artifact です。
- model fetch logic は `[build.bazelisk]` など後続 TOML section を model field と誤認してはいけません。
- Bazelisk verification は file exists だけでなく `tools/bazelisk/bazelisk.exe` が executable であることを確認します。
- Bazelisk が壊れている場合は rebuild 前に強制 redownload します。

## runtime 性能調整メモ

この数値は開発環境での sample であり、製品保証ではありません。profile-level の自動おすすめ設定や benchmark flow を作るときの初期候補として使います。

条件:

- Date: 2026-05-21 JST
- Camera: Global Shutter Camera, DirectShow, 640x480 at requested 90 fps
- Profile: Head / Face / Hands / Arms / Torso enabled, Legs / Feet disabled
- Capturer: release build
- Sampling: 約 2 秒 warmup、約 5 秒 sample
- Metric: `/api/runtime/snapshot` telemetry delta

delegate / thread sample:

| delegate | threads | input raw fps | native callback / emitted fps |
| --- | ---: | ---: | ---: |
| CPU | 1 | 88.40 | 75.63 |
| CPU | 2 | 88.20 | 75.03 |
| CPU | 4 | 88.50 | 75.72 |
| XNNPACK | 1 | 88.39 | 75.76 |
| XNNPACK | 2 | 88.21 | 88.01 |
| XNNPACK | 4 | 88.41 | 88.21 |

観測:

- この sample では CPU delegate は 1 から 4 threads へ増やしても明確には伸びませんでした。
- XNNPACK は 2 threads で 90 fps class に届きました。
- 640x480 at 90 fps では XNNPACK 4 threads は 2 threads より明確に速くありませんでした。そのため profile 既定は 2 threads です。
- GPU delegate は `GPU processing is disabled in build flags` により benchmark できませんでした。

Windows GPU delegate note:

- `--define=MEDIAPIPE_DISABLE_GPU=1` を `0` に変える短い build-flag 実験では、NVIDIA SDK など明確な vendor SDK を要求されず DLL target は build できました。
- しかし runtime Holistic creation は `ImageCloneCalculator: GPU processing is disabled in build flags` で失敗しました。
- 原因は upstream MediaPipe の `mediapipe/gpu/BUILD` にある Windows guard です。`//mediapipe/gpu:disable_gpu` は `@platforms//os:windows` と明示 flag の両方に一致し、framework code に `MEDIAPIPE_DISABLE_GPU=1` を定義します。
- Windows v1 では GPU delegate は unsupported と扱います。Supervisor UI には出さず、XNNPACK を supported fast path にします。

Holistic FlowLimiter sample (`delegate = XNNPACK`, `threads = 4`):

| FlowLimiter | max in flight | max queue | emitted fps |
| --- | ---: | ---: | ---: |
| on | 1 | 1 | 88.21 |
| off | 1 | 1 | 88.41 |
| on | 2 | 1 | 88.11 |
| on | 4 | 1 | 88.39 |
| on | 1 | 0 | 62.40 |
| on | 1 | 2 | 88.21 |
| on | 2 | 0 | 88.42 |

観測:

- この sample では FlowLimiter on/off は throughput の主要因ではありませんでした。
- `max_in_flight = 1` かつ `max_queue = 0` は悪い組み合わせで、約 62 fps まで落ちました。
- `max_in_flight = 1`、`max_queue = 1` を安全な baseline とします。
- `max_in_flight = 2`、`max_queue = 0` では同じ throughput drop は見えませんでした。ただし queue-less setting は default recommendation ではなく latency 実験として扱います。

## handoff rule

Native MediaPipe が ready でない場合は、不足要件をこの文書または session handoff に記録し、native pipeline か packaging surface を直接直します。
