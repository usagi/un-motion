# VRC (VRCFT) / OSC 出力仕様

この文書は issue-4 の実装範囲として、U.N. Motion から VRChat へ VRCFaceTracking 互換の表情パラメータを直接送信する output layer を定義する。

## 目的

U.N. Motion の `UNMotionFrame` に含まれる Face 信号を VRCFaceTracking の Unified Expressions 互換パラメータへ変換し、VRChat の OSC Avatar Parameters API へ送信する。

対象は Perfect Sync / face tracking 表情。U.N. Motion の分類では Face のみを扱う。

issue-4 の MVP では body pose、hand pose、OSC trackers、VRChat input controller、chatbox、world OSC は扱わない。

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
- VRCFaceTracking parameter は Float、同名 Bool、Binary Bool の形を取り得る。issue-4 では Face tracking の連続値 Float を主信号にしつつ、OSCQuery で avatar が公開している Bool / Binary Bool も送信対象にする。

## Output 名

ユーザー向け表示名は次。

```text
VRC (VRCFT) / OSC
```

内部識別子は将来の VRChat OSC 拡張を考慮し、`vrc-osc` / `VrcOsc` を使う。

理由:

- issue-4 の実体は VRCFaceTracking 互換 Face parameter 出力。
- 送信先 protocol は VRChat OSC Avatar Parameters。
- 将来 tracker や別の VRChat OSC API を追加しても、`vrc-osc` という transport 名は拡張可能。
- UI では VRCFT 互換であることを明示し、ユーザーが VMC/UDP や UNMF/Z と区別しやすくする。

## 正式経路

VRC (VRCFT) / OSC は UNMF/Z、VMC/UDP と同じく output layer。

```text
Input -> Engine -> UNMotionFrame -> Modifier -> Output
                                           +-> UNMF/Z
                                           +-> VMC/UDP
                                           +-> VRC (VRCFT) / OSC
```

VRC (VRCFT) / OSC output は Modifier 適用後の `UNMotionFrame` だけを読む。MediaPipe raw output、MediaPipe post-process 内部値、VMC packet、iFacialMocap raw packet は直接読まない。

UNMF/Z、VMC/UDP、VRC (VRCFT) / OSC は同時出力可能。各 output worker は同じ post-modifier frame を受け取り、独立した送信先と telemetry を持つ。

## スコープ

### MVP で実装するもの

- `UNMotionFrame.face.expressions` から VRCFaceTracking Unified Expressions parameter への変換。
- `UNMotionFrame.signals` 内の Face / eye 系 scalar signal から VRCFaceTracking Unified Expressions parameter への変換。
- OSC address `/avatar/parameters/<parameter>` への Float / Bool / Binary Bool 送信。
- 送信先 IP / port の profile 設定。
- VRChat OSCQuery / avatar parameter が検出できる場合だけ送信する option。
- UNMF/Z、VMC/UDP との同時出力。
- runtime telemetry。
- unit test と UDP receiver smoke test。

### MVP で実装しないもの

- body / hand / finger pose の VRChat OSC tracker 送信。
- VRChat input controller 送信。
- VRChat chatbox 送信。
- avatar / VRChat 側 OSC config JSON の自動生成や編集。
- VRCFaceTracking アプリ本体への送信。

## 送信先

既定値:

```text
target_addr = "127.0.0.1:9000"
```

VRChat の default input port に合わせる。VMC/UDP と同様に profile から任意の IP / port を指定可能にする。

VRChat の起動引数 `--osc=inPort:senderIP:outPort` で input port が変更されている場合、ユーザーは `vrcOscTargetAddr` を変更する。

## 設定

Profile runtime settings に次の field を追加する。

```text
vrcOscEnabled = false
vrcOscTargetAddr = "127.0.0.1:9000"
vrcOscSendOnlyWhenVrchatRunning = true
vrcOscProcessPollIntervalSecs = 10
vrcOscParameterPrefix = "FT"
```

意味:

- `vrcOscEnabled`: VRC (VRCFT) / OSC output を有効化する。
- `vrcOscTargetAddr`: OSC datagram の送信先。
- `vrcOscSendOnlyWhenVrchatRunning`: VRChat OSCQuery から avatar parameter が読める場合だけ送信する。既存 profile 互換のため field 名は維持する。
- `vrcOscProcessPollIntervalSecs`: OSCQuery / avatar parameter を再確認する間隔。既存 profile 互換のため field 名は維持する。
- `vrcOscParameterPrefix`: VRCFaceTracking parameter prefix。既定は一般的な VRCFT avatar で使われる `FT`。明示的に空文字を指定した場合は `v2/...` をそのまま送る。

`vrcOscSendOnlyWhenVrchatRunning` は既定 ON。VRChat は起動から avatar が動作する scene に到達するまで数十秒から数分かかることが多いため、10 秒間隔の polling で実用上十分。毎 frame の OSCQuery は行わない。

## OSCQuery gate

`vrcOscSendOnlyWhenVrchatRunning = true` のとき、output worker は `vrcOscProcessPollIntervalSecs` ごとに VRChat OSCQuery server を探し、`/avatar/parameters` を取得する。

VRChat の OSCQuery HTTP server port は可変。理想形は mDNS Service Discovery だが、MVP 実装では Windows 上で VRChat process の localhost listen port を候補にし、`/avatar/parameters` が HTTP 200 を返す port を OSCQuery server とみなす。process 名だけで gate を開くことはしない。

OSCQuery server が見つからない、または avatar parameter tree を取得できない間は frame を破棄し、UDP 送信しない。これは無駄な localhost UDP traffic を抑え、かつ avatar 側に存在しない Float / Bool parameter を送らないための gate。

検出できないもの:

- VRChat の OSC が disabled。
- VRChat の OSC input port が既定以外。
- world / scene transition 中。

検出して使うもの:

- avatar が公開している Float parameter。
- avatar が公開している Bool parameter。
- `...1` / `...2` / `...4` / `...8` / `...Negative` の存在から決まる binary parameter bit 幅と sign。

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

OSC argument は Float、同名 Bool、Binary Bool。

```text
Float(value)
Bool(value < 0.5) for <parameter> when avatar exposes the same parameter as Bool
Bool(value) for <parameter>Negative / <parameter>1 / <parameter>2 / <parameter>4 / <parameter>8
```

値域:

- 通常の expression は `0.0..=1.0` に clamp する。
- `EyeLeftX` / `EyeLeftY` / `EyeRightX` / `EyeRightY` / `JawX` / `JawZ` など、VRCFaceTracking 仕様上 negative range を持つ parameter は `-1.0..=1.0` を許容する。
- Binary Bool は avatar が公開している `1/2/4/8` の bit 幅に合わせて量子化する。低振幅 jitter を避けるため、絶対値 `0.15` 未満は 0 扱い。

送信単位:

- 1 frame の Face parameter 群を 1 OSC bundle または複数 message として 1 UDP datagram にまとめる。
- 空の frame、Face disabled 後に送信対象がない frame、OSCQuery gate で blocked の frame は送信しない。

## Parameter prefix

`vrcOscParameterPrefix` は avatar 側 parameter の namespace に対応するための option。

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

prefix の前後 slash は実装側で正規化する。

## Mapping 方針

実装は VMC output の blendshape map と同じ発想で、入力 signal 名から VRCFT parameter 名への route を持つ。VRCFT avatar が Float parameter を持つ基礎項目は Float を主信号として送る。同名 Bool parameter が avatar 側に存在する場合は、VRCFT の `EParam` と同じく `value < 0.5` を Bool として送る。一方で、VRCFT avatar には `TongueOut1/2/4`、`SmileFrownLeft1/2/4`、`CheekPuffLeft1/2/4` のように binary Bool 群だけで公開される項目があるため、OSCQuery で実際に存在する Bool 群を見て Bool 出力を決める。固定の 3bit / 4bit 決め打ちは fallback のみに限定する。

`EyeSquintLeft` / `EyeSquintRight` は例外として、ARKit `eyeSquint` から binary Bool fallback を作らない。Webcam / MediaPipe 系の `eyeSquint` は頬上げと相関しやすく、VRCFT avatar 側では `EyeSquint*1/2/4` が強い閉眼として実装されていることがあるため。avatar が `EyeSquint*1/2/4` しか持たない場合は、`EyeLidLeft` / `EyeLidRight` の閉眼量から Bool を作り、頬由来の squint を閉眼へ混ぜない。

MVP の既定 mapping は次を優先する。

1. 入力名が `v2/` を含む VRCFT parameter として扱える場合は pass-through。
2. `face.<ARKitName>` または `<ARKitName>` を既知の Unified Expressions parameter へ mapping。
3. `eye.left.yaw` / `eye.left.pitch` / `eye.right.yaw` / `eye.right.pitch` を `v2/EyeLeftX` などへ mapping。

VRM / VMC の combined `Blink` は左右両方の `v2/EyeLidLeft` / `v2/EyeLidRight` へ展開する。`Blink_L` / `Blink_R` と ARKit `eyeBlinkLeft` / `eyeBlinkRight` は片側の `EyeLid` へ mapping する。

`EyeLidLeft` / `EyeLidRight` は VRCFT と同じ openness 形式。ARKit / VRM の blink 入力は `0.75 * (1.0 - blink)` に反転し、複数入力が同じ eyelid に来た場合はより閉じている低い値を優先する。

avatar が simplified `v2/EyeX` / `v2/EyeY` だけを公開している場合は、左右 `EyeLeftX/Y` と `EyeRightX/Y` の平均から combined parameter を補完する。

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

既定 mapping は VRCFaceTracking current documentation と実装を確認して built-in default map として固定済み。profile で上書き可能にする余地は残すが、issue-4 では最小設定に留める。

## Crate 構成

新規 crate:

```text
crates/un-motion-output-vrc-osc
```

責務:

- `UNMotionFrame` から VRC OSC packet list を生成する pure conversion。
- VRCFT parameter mapping。
- VRCFT Binary Bool parameter (`...Negative`, `...1`, `...2`, `...4`, `...8`)。OSCQuery の avatar parameter capability に合わせて bit 幅と sign を決める。
- prefix 正規化。
- value clamp。
- unit tests。

依存は軽く保つ。

```text
rosc
un-motion-frame
anyhow
serde
```

`un-motion-output-vmc` には入れない。VMC/UDP と VRChat OSC は同じ OSC over UDP でも address schema と semantic contract が異なるため、crate を分ける。

## Runtime worker

`un-motion-runtime` に VRC OSC worker を追加する。

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
- OSCQuery gate / avatar parameter capability refresh。
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
- 同名 Bool の `value < 0.5` 送信。
- OSCQuery capability に基づく Float / Binary Bool 選択。
- `EyeLid` 由来の `EyeSquint*` Binary fallback。
- empty frame で packet なし。

Runtime tests:

- UDP receiver に `/avatar/parameters/v2/JawOpen` が届く。
- OSCQuery gate disabled では VRChat OSCQuery なしでも送信される。
- OSCQuery gate enabled かつ VRChat OSCQuery / avatar parameters が読めない場合は送信されず skipped counter が増える。環境依存のため automated unit test ではなく runtime / manual test で確認する。
- output worker stop event が出る。

Integration / smoke:

- `cargo xtask verify`。
- synthetic Face frame から UDP receiver へ送る smoke test。

## 既知の制約

- Windows 版 MVP は OSCQuery port 発見時に VRChat process の localhost listen port を候補にする。mDNS Service Discovery は将来改善。
- avatar parameter が存在しない場合、その parameter は送信しない。
- `EyeTrackingActive` / `ExpressionTrackingActive` / `LipTrackingActive` は VRCFT と同じ tracking active parameter として送信する。avatar 側に存在しない場合は VRChat が無視する。
- Binary Bool の低振幅 deadzone と `EyeSquint*` fallback は、Webcam 系入力の jitter と頬上げ由来の誤閉眼を抑えるための UNMotion 側の実用差分。
- VRCFaceTracking documentation は更新される可能性があるため、既定 mapping は実装時と release 前に current documentation を再確認する。
- issue-4 では Face output のみを完了条件とする。姿勢や tracker は別作業。
