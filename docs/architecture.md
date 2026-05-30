# アーキテクチャ

U.N. Motion は desktop-first の motion capture / motion routing application です。現在の GUI entrypoint は `apps/un-motion-supervisor` で、local Core HTTP API を持つ `un-motion-capturer` process を起動・管理します。

## CoreSplit の目標

Project CoreSplit は、リアルタイムの motion 処理を Tauri GUI から分離します。

```text
un-motion-capturer.exe
  選択 profile を所有し、local Core HTTP API、capture、inference、output を実行する

un-motion-supervisor.exe
  Tauri settings、profile editor、capturer manager、logs を提供する
```

core crate と runtime worker は Tauri から独立させます。Tauri は window と settings view を持ってよいですが、UDP socket、motion frame stream tick、output worker、packet-level telemetry を所有しません。

## プロダクト境界

現在の runtime shape は CoreSplit です。

- `un-motion-capturer.exe` は runtime lifecycle、local Core HTTP API、input acquisition、Engine 実行、`UNMotionFrame` 生成、Modifier、Output worker、telemetry snapshot を所有します。
- `un-motion-supervisor.exe` は settings、profile editing、Capturer process の launch / stop / restart、logs、monitoring を所有します。
- GUI は UDP socket、output worker、packet-level telemetry、MediaPipe runtime state を直接持ちません。

## 推定エンジン方針

U.N. Motion の主経路は **MediaPipe Native** による姿勢推定です。Capturer には MediaPipe Web 経路を残しません。すべての入力は `UNMotionFrame` 出力契約に正規化してから Modifier / Output に渡します。

```text
Input -> ImageFrame -> input-buffer -> MediaPipe Native DLL
      -> MediaPipe post-process -> UNMotionFrame -> Modifier -> Output
```

代替 engine を追加する場合も、次の境界を実装します。

```text
ImageInferenceEngine<ImageFrame -> raw output>
FrameProcessor<(ImageFrame, raw output) -> UNMotionFrame>
```

MediaPipe post-process (`un-motion-engine-mediapipe-post-process`) は `UNMotionFrame` v1.1 header の `version_minor = 1`、`stream_id`、`coordinate_space = UNMotion` を一貫して埋めます。`stream_id` には Capturer の `source_id` を入れるため、`un-motion/frame/v1/<stream-id>` の topic で複数 Capturer を区別できます。

古い `PoseEngine` interface、Dummy engine、ONNX RTMPose placeholder は product architecture ではありません。代替 engine を再導入する場合は component pipeline と明示的な post-process contract を使います。

## データフロー

Capturer の正式な内部経路は次の 1 本です。

```text
Input -> Engine(MediaPipe + post-process) -> UNMotionFrame -> Modifier -> Output
```

`UNMotionFrame` が内部の単一 frame 契約です。`UNMF/Z` は Zenoh transport、`VMC/UDP` は OSC transport にすぎません。どちらの Output も同じ Modifier 適用後の `UNMotionFrame` を受けます。

Output が MediaPipe raw output や post-process 内部値を直接読む経路、または VMC だけが `UNMotionFrame` を迂回する経路は正式経路ではありません。

MediaPipe Native の場合:

```text
Input component
  webcam-directshow | webcam-mediafoundation | file-image | file-video
        |
        v
ImageFrame
        |
        v
[Engine] MediaPipe Native
        |
        v
MediaPipe post-process (Engine 内部)
        |  (UNMotionFrame v1.1: stream_id = Capturer source_id)
        v
UNMotionFrame
        |
        v
[Modifier] Smoothing / Mirror / Calibration / BoneFilter
        |
        v
[Output]
        +---- UNMF/Z  : UNMotionFrame を publish
        +---- VMC/UDP : 最後の境界で UNMotionFrame を OSC packet へ変換
        +---- VRC (VRCFT) / OSC : Face signal を VRChat OSC Avatar Parameters へ変換
```

VRC (VRCFT) / OSC output は issue-4 の範囲では Face signal のみを扱います。詳細は [VRC (VRCFT) / OSC Output](vrc-osc-output.md) に固定します。

## Web カメラ backend 方針

Windows では `webcam-directshow` と `webcam-mediafoundation` を並行して扱います。

DirectShow (`ccap-rs`) は OBS 仮想カメラなどの DirectShow device 互換性と、解像度 / FPS / PixelFormat の明示指定を重視する現行の実用既定です。

MediaFoundation (`nokhwa`) は DirectShow と互換でない device や将来の Windows SDK 変化に備える予備経路です。両者は同じ「Windows webcam」の置き換え関係ではなく、扱える device と format 報告の性質が異なります。

非 Windows は DirectShow を使えないため `nokhwa` 系 backend を候補にします。ただし Windows リリース品質の基準は `webcam-directshow` を中心に置きます。

## protocol 入力

VMC / iFacialMocap 受信を Capturer 入力として使う場合も同じです。protocol decoder が Engine 境界として動作し、`UNMotionFrame` を生成します。

```text
VMC input          -> VMC decoder          -> UNMotionFrame -> Modifier -> Output
iFacialMocap input -> iFacialMocap decoder -> UNMotionFrame -> Modifier -> Output
```

Modifier は Zenoh / VMC のどちらの出力経路にも同じ意味で適用されます。Profile の `runtime_selection.modifier.*_enabled` を single source of truth とし、Engine は必要な情報を `UNMotionFrame` へ詰めてから、Modifier が出力直前に subset を切ります。

Capturer は runtime mux / fusion state を持ちません。複雑な合成、優先度、blend、TTL は将来の UNMotionSynthesizer 側で扱い、Capturer には `UNMotionFrame` stream として入力します。

## UNMF/Z

UNMotionFrame/Zenoh (UNMF/Z) 出力は `un-motion/frame/v1` を既定 key とします。`runtime_selection.zenohTopicMode` で次の 3 モードを切り替えます。

- `TopicMode::Frame`: 1 key に集約する。
- `ByPrimarySource`: primary source ごとに分ける。
- `ByStreamId`: `stream_id` ごとに分ける。

payload は MessagePack です。`stream_id` は publish する Capturer の `source_id` を使います。`expected_dt_ns = 1_000_000_000 / fps` は worker 側で詰めます。

## VTuber 向けの優先経路

実用上の VTuber path は Head / Face / Hands / Arms を優先します。

Hands は MediaPipe HandLandmarker output と設定された camera diagonal view angle から camera-space hand target を推定します。Arms は shoulders を anchor とし、elbow / wrist の arm motion を出します。Torso は shoulder / hip body landmarks を持ち、`Chest` を動かします。Legs は hip / knee / ankle から hips と upper/lower leg bones を動かします。Feet は ankle / heel / foot-index から foot / toe bones を動かします。

下半身 modifier は、v1 では上半身 VTuber path より優先度を下げます。

VMC output では、lower-arm の主方向を elbow-to-wrist geometry に合わせ、MediaPipe hand `palm.normal` は forearm 軸まわりの wrist twist としてだけ混ぜます。Hand orientation は `palm.forward` を主軸、`palm.normal` を hand bone local `-Y` の手の平軸として扱います。

## 複数 source の境界

Capturer の責務は 1 profile の入力を `UNMotionFrame` stream に正規化し、Modifier と Output へ渡すことです。複数 Capturer / 複数 source の優先度合成や blend は Capturer 外の上位層で扱います。

## crate の役割

- `un-motion-frame`: crates.io から使う portable frame schema。
- `un-motion-core`: UI runtime に依存しない frame logic と API runtime。
- `un-motion-config`: persistent config と validation。
- `un-motion-interfaces`: image、queue、processor、output interface。
- `un-motion-pipeline`: queue 実装と image inference pipeline。
- `un-motion-input-file-image`: still image input。
- `un-motion-input-file-video`: ffmpeg-backed video input。
- `un-motion-input-ifacialmocap`: iFacialMocap UDP/TCP receiver と `UNMotionFrame` conversion。
- `un-motion-input-webcam-directshow`: Windows DirectShow webcam input。
- `un-motion-input-webcam-nokhwa`: MediaFoundation / AVFoundation / V4L2 webcam enumeration と format probing。
- `un-motion-engine-mediapipe-types`: Native MediaPipe raw output type。
- `un-motion-engine-mediapipe-native`: Native DLL image inference adapter。
- `un-motion-engine-mediapipe-post-process`: MediaPipe raw output から frame を生成する。
- `un-motion-mediapipe-native`: C++ DLL 用 Rust ABI と dynamic loader。
- `un-motion-input-vmc`: OSC over UDP receiver と external mocap stream decoder。
- `un-motion-output-vmc`: VMC OSC packet output。
- `un-motion-frame-zenoh`: `un-motion-frame` workspace で管理する外部 crate。Zenoh Pub/Sub の wire convention と `Publisher` / `Subscriber` utility を提供します。

MediaPipe source checkout、Bazel cache、生成 DLL、download 済み native model は local artifact です。明示的な配布判断がない限り MIT source tree には含めません。

## Tauri 依存境界

core crate に Tauri を入れません。

```text
apps/un-motion-supervisor
  tauri::AppHandle と GUI lifecycle を所有する

crates/un-motion-*
  portable data、input adapter、output adapter、pure logic を所有する
```
