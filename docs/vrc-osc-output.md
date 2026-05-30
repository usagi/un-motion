# VRC (VRCFT) / OSC 出力仕様

この文書は issue-4 の実装範囲として、U.N. Motion から VRChat へ VRCFaceTracking 互換の表情パラメータを直接送信する output layer を定義します。

## 目的

U.N. Motion の `UNMotionFrame` に含まれる Face 信号を VRCFaceTracking の Unified Expressions 互換パラメータへ変換し、VRChat の OSC Avatar Parameters API へ送信します。

対象は Perfect Sync / face tracking 表情です。U.N. Motion の分類では Face のみを扱います。

issue-4 の MVP では body pose、hand pose、OSC trackers、VRChat input controller、chatbox、world OSC は扱いません。

## 参照仕様

- VRChat OSC overview: https://docs.vrchat.com/docs/osc-overview
- VRChat OSC avatar parameters: https://docs.vrchat.com/docs/osc-avatar-parameters
- VRCFaceTracking parameters: https://docs.vrcft.io/docs/tutorial-avatars/tutorial-avatars-extras/parameters
- VRCFaceTracking Unified Expressions: https://docs.vrcft.io/docs/tutorial-avatars/tutorial-avatars-extras/unified-blendshapes

参照仕様から置く前提:

- VRChat の OSC input 既定 port は `9000`、output 既定 port は `9001`。
- 外部 sender は通常 `127.0.0.1:9000` へ送信する。
- VRChat の avatar parameter input は `/avatar/parameters/<parameter>` 形式の OSC address を使う。
- VRCFaceTracking は `v2/JawOpen` のような Unified Expressions parameter を avatar expression parameter として使う。
- VRCFaceTracking parameter は Float と Binary の形を取り得るが、issue-4 MVP は Face tracking の連続値 Float を主対象にする。

## Output 名

ユーザー向け表示名は次とします。

```text
VRC (VRCFT) / OSC
```

内部識別子は将来の VRChat OSC 拡張を考慮し、`vrc-osc` / `VrcOsc` を使います。

理由:

- issue-4 の実体は VRCFaceTracking 互換 Face parameter 出力です。
- 送信先 protocol は VRChat OSC Avatar Parameters です。
- 将来 tracker や別の VRChat OSC API を追加しても、`vrc-osc` という transport 名は拡張できます。
- UI では VRCFT 互換であることを明示し、ユーザーが VMC/UDP や UNMF/Z と区別しやすくします。

## 正式経路

VRC (VRCFT) / OSC は UNMF/Z、VMC/UDP と同じく output layer です。

```text
Input -> Engine -> UNMotionFrame -> Modifier -> Output
                                           +-> UNMF/Z
                                           +-> VMC/UDP
                                           +-> VRC (VRCFT) / OSC
```

VRC (VRCFT) / OSC output は Modifier 適用後の `UNMotionFrame` だけを読みます。MediaPipe raw output、MediaPipe post-process 内部値、VMC packet、iFacialMocap raw packet を直接読みません。

UNMF/Z、VMC/UDP、VRC (VRCFT) / OSC は同時出力可能です。各 output worker は同じ post-modifier frame を受け取り、独立した送信先と telemetry を持ちます。

## スコープ

### MVP で実装するもの

- `UNMotionFrame.face.expressions` から VRCFaceTracking Unified Expressions parameter への変換。
- `UNMotionFrame.signals` 内の Face / eye 系 scalar signal から VRCFaceTracking Unified Expressions parameter への変換。
- OSC address `/avatar/parameters/<parameter>` への Float 送信。
- 送信先 IP / port の profile 設定。
- VRChat process 起動時のみ送信する option。
- UNMF/Z、VMC/UDP との同時出力。
- runtime telemetry。
- unit test と UDP receiver smoke test。

### MVP で実装しないもの

- body / hand / finger pose の VRChat OSC tracker 送信。
- VRChat input controller 送信。
- VRChat chatbox 送信。
- avatar OSC config JSON の自動生成や編集。
- VRChat 側 OSC 有効状態、avatar load 状態、parameter 存在確認。
- VRCFaceTracking アプリ本体への送信。

## 送信先

既定値:

```text
target_addr = "127.0.0.1:9000"
```

VRChat の default input port に合わせます。VMC/UDP と同様に profile から任意の IP / port を指定可能にします。

VRChat の起動引数 `--osc=inPort:senderIP:outPort` で input port が変更されている場合、ユーザーは `vrcOscTargetAddr` を変更します。

## 設定

Profile runtime settings に次の field を追加します。

```text
vrcOscEnabled = false
vrcOscTargetAddr = "127.0.0.1:9000"
vrcOscSendOnlyWhenVrchatRunning = true
vrcOscProcessPollIntervalSecs = 10
vrcOscParameterPrefix = "FT"
```

意味:

- `vrcOscEnabled`: VRC (VRCFT) / OSC output を有効化します。
- `vrcOscTargetAddr`: OSC datagram の送信先です。
- `vrcOscSendOnlyWhenVrchatRunning`: VRChat process が見つかった場合だけ送信します。
- `vrcOscProcessPollIntervalSecs`: process 一覧を再確認する間隔です。
- `vrcOscParameterPrefix`: VRCFaceTracking parameter prefix です。既定は一般的な VRCFT avatar で使われる `FT` です。明示的に空文字を指定した場合は `v2/...` をそのまま送ります。

`vrcOscSendOnlyWhenVrchatRunning` は既定 ON とします。VRChat は起動から avatar が動作する scene に到達するまで数十秒から数分かかることが多いため、10 秒間隔の polling で実用上十分です。毎 frame の process 列挙は禁止します。

## Process gate

`vrcOscSendOnlyWhenVrchatRunning = true` のとき、output worker は `vrcOscProcessPollIntervalSecs` ごとに VRChat process の存在を確認します。

Windows では少なくとも次の process 名を検出対象にします。

```text
VRChat.exe
VRChat
```

process が見つからない間は frame を破棄し、UDP 送信しません。これは無駄な localhost UDP traffic を抑えるための gate であり、VRChat が OSC input を受け付ける状態であることの保証ではありません。

検出できないもの:

- VRChat の OSC が disabled。
- VRChat の OSC input port が既定以外。
- avatar が未ロード。
- avatar に該当 expression parameter がない。
- world / scene transition 中。

## OSC packet

送信 address:

```text
/avatar/parameters/<parameter>
```

例:

```text
/avatar/parameters/FT/v2/JawOpen
/avatar/parameters/FT/v2/EyeLeftX
/avatar/parameters/FT/v2/EyeRightY
```

OSC argument は Float と Binary Bool を送ります。

```text
Float(value)
Bool(value) for <parameter>Negative / <parameter>1 / <parameter>2 / <parameter>4 / <parameter>8
```

値域:

- 通常の expression は `0.0..=1.0` に clamp します。
- `EyeLeftX` / `EyeLeftY` / `EyeRightX` / `EyeRightY` / `JawX` / `JawZ` など、VRCFaceTracking 仕様上 negative range を持つ parameter は `-1.0..=1.0` を許容します。

送信単位:

- 1 frame の Face parameter 群を 1 OSC bundle または複数 message として 1 UDP datagram にまとめます。
- 空の frame、Face disabled 後に送信対象がない frame、process gate で blocked の frame は送信しません。

## Parameter prefix

`vrcOscParameterPrefix` は avatar 側 parameter の namespace に対応するための option です。

既定:

```text
vrcOscParameterPrefix = "FT"
parameter = "v2/JawOpen"
address = "/avatar/parameters/FT/v2/JawOpen"
```

prefix あり:

```text
vrcOscParameterPrefix = "ExamplePrefix"
parameter = "ExamplePrefix/v2/JawOpen"
address = "/avatar/parameters/ExamplePrefix/v2/JawOpen"
```

prefix の前後 slash は実装側で正規化します。

## Mapping 方針

実装は VMC output の blendshape map と同じ発想で、入力 signal 名から VRCFT parameter 名への route を持ちます。VRCFT avatar が Float parameter を持つ基礎項目は Float を主信号として送ります。一方で、VRCFT avatar には `TongueOut1/2/4`、`SmileFrownLeft1/2/4`、`CheekPuffLeft1/2/4` のように binary Bool 群だけで公開される項目があるため、既知の binary-only 互換項目には Bool 群も送ります。

MVP の既定 mapping は次を優先します。

1. 入力名が `v2/` を含む VRCFT parameter として扱える場合は pass-through。
2. `face.<ARKitName>` または `<ARKitName>` を既知の Unified Expressions parameter へ mapping。
3. `eye.left.yaw` / `eye.left.pitch` / `eye.right.yaw` / `eye.right.pitch` を `v2/EyeLeftX` などへ mapping。

初期候補:

```text
face.jawOpen        -> v2/JawOpen
face.eyeBlinkLeft   -> v2/EyeLidLeft       (openness 形式へ反転)
face.eyeBlinkRight  -> v2/EyeLidRight      (openness 形式へ反転)
face.mouthSmileLeft -> v2/SmileFrownLeft
face.mouthSmileRight -> v2/SmileFrownRight
eye.left.yaw        -> v2/EyeLeftX
eye.left.pitch      -> v2/EyeLeftY
eye.right.yaw       -> v2/EyeRightX
eye.right.pitch     -> v2/EyeRightY
```

正確な既定 mapping は実装時に VRCFaceTracking current documentation の parameter list を再確認して固定します。mapping は profile で上書き可能にする余地を残しますが、MVP では built-in default map と最小設定に留めます。

## Crate 構成

新規 crate:

```text
crates/un-motion-output-vrc-osc
```

責務:

- `UNMotionFrame` から VRC OSC packet list を生成する pure conversion。
- VRCFT parameter mapping。
- VRCFT Binary Bool parameter (`...Negative`, `...1`, `...2`, `...4`, `...8`)。Float がない avatar 互換項目の fallback として使う。
- prefix 正規化。
- value clamp。
- unit tests。

依存は軽く保ちます。

```text
rosc
un-motion-frame
anyhow
serde
```

`un-motion-output-vmc` には入れません。VMC/UDP と VRChat OSC は同じ OSC over UDP でも address schema と semantic contract が異なるため、crate を分けます。

## Runtime worker

`un-motion-runtime` に VRC OSC worker を追加します。

想定型:

```text
VrcOscOutputConfig
VrcOscOutputFrame
VrcOscOutputCommand
VrcOscOutputStats
VrcOscOutputEvent
VrcOscOutputWorker
VrcOscOutputWorkerHandle
spawn_vrc_osc_output_worker
```

worker の責務:

- UDP socket bind。
- process gate。
- Modifier 適用後 frame の受信。
- VRC OSC packet 生成。
- OSC encode。
- UDP send。
- telemetry event 発行。

stats:

```text
sent_datagrams
sent_packets
skipped_frames
process_gate_blocked_frames
error_count
```

telemetry:

```text
target_addr
vrchat_detected
sent_datagrams
sent_packets
skipped_frames
error_count
last_error
```

## Core / Supervisor 接続

Core runtime:

- `CoreRuntimeConfig` に `vrc_osc_output` を追加。
- `core_runtime_config_from_document` で profile runtime settings から parse。
- `CoreRuntimeWorkers::start` で VRC OSC worker を起動。
- frame dispatch で VMC / Zenoh と同じ post-modifier frame を送る。
- `RuntimeSnapshot.output_telemetry` に VRC OSC telemetry を追加。

Profile schema:

- `ProfileRuntimeSettings` に `vrc_osc_*` fields を追加。
- TOML / JSON roundtrip test を追加。

Supervisor:

- profile detail view に `vrcOscEnabled`、`vrcOscTargetAddr`、`vrcOscSendOnlyWhenVrchatRunning`、`vrcOscProcessPollIntervalSecs`、`vrcOscParameterPrefix` を追加。
- Output section に `VRC (VRCFT) / OSC` group を追加。
- telemetry panel に VRC OSC target、VRChat detected、sent/skipped/error を表示。

Config examples:

- `configs/desktop.example.toml` に disabled 既定を記載。
- 必要なら `configs/vrc-osc.example.toml` を追加。

## テスト計画

Unit tests:

- prefix 正規化。
- ARKit / PerfectSync face expression から VRCFT parameter への mapping。
- `v2/...` pass-through。
- negative range parameter の clamp。
- normal parameter の `0.0..=1.0` clamp。
- empty frame で packet なし。

Runtime tests:

- UDP receiver に `/avatar/parameters/v2/JawOpen` が届く。
- process gate disabled では VRChat process なしでも送信される。
- process gate enabled かつ VRChat process なしでは送信されず skipped counter が増える。
- output worker stop event が出る。

Integration / smoke:

- `cargo xtask verify`。
- 必要なら `cargo xtask core vrc-osc-smoke` を追加し、synthetic Face frame から OSC receiver へ送る。

## 既知の制約

- VRChat process の存在は OSC 受信可能性を保証しません。
- avatar parameter が存在しない場合、VRChat 側で値は実質的に使われません。
- VRCFaceTracking documentation は更新される可能性があるため、既定 mapping は実装時と release 前に current documentation を再確認します。
- issue-4 では Face output のみを完了条件とします。姿勢や tracker は別作業です。
