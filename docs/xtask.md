# xtask

`crates/xtask` は、この repository 内で使う補助 command の正式な置き場所です。

product binary ではないが repository 内で再現したい script、実験、研究 command は `cargo xtask <command>` として追加します。

## ルール

- 新しい orchestration は `crates/xtask` に追加します。
- docs では `cargo xtask <command>` を標準導線にします。
- PowerShell script を公式導線として増やしません。
- third-party source checkout や生成 native binary は、明示的な配布判断がない限り repository に入れません。
- Native MediaPipe 関連の fetch / build / probe orchestration は `xtask` に置けますが、checkout、model、Bazelisk、DLL は ignored local artifact のままにします。

## command 一覧

```sh
cargo xtask verify
cargo xtask verify --skip-frontend
cargo xtask fmt
cargo xtask check
cargo xtask test
cargo xtask frontend build
cargo xtask desktop dev
cargo xtask core smoke
cargo xtask core lifecycle-smoke
cargo xtask core vmc-smoke
cargo xtask core vmc-mirror-smoke
cargo xtask core external-vmc-smoke --listen 127.0.0.1:39550 --target 127.0.0.1:39551 --observe 127.0.0.1:39571 --label warudo-to-vseeface
cargo xtask core external-ifacialmocap-smoke --listen 192.168.13.13:49983 --target 127.0.0.1:39551 --observe 127.0.0.1:39571 --label ifacialmocap-to-vseeface
cargo xtask make-release-package
cargo xtask make-release-package --version 1.2.3.beta-1
cargo xtask make-release-package --version 1.2.3.beta-1 --skip-build --keep-staging
cargo xtask license-report
cargo xtask research penn-action prepare
cargo xtask research penn-action summary
cargo xtask research penn-action desktop-config --sequence 0001
cargo xtask research ffmpeg prepare
cargo xtask image resize --input input.png --output output.png --width 320 --height 240
cargo xtask mediapipe build-native
cargo xtask mediapipe native-probe -- --image image.png
cargo xtask mediapipe pose-fixtures
cargo xtask mediapipe pose-fixtures --head-diagnostics
cargo xtask vmc capture-frame --addr 127.0.0.1:39551 --collect-ms 25 --label waidayo-vmc
cargo xtask vmc stability --addr 127.0.0.1:39560 --duration-ms 5000 --source-id wmc --output target/vmc-captures/runs/stability/wmc.json
cargo xtask vmc stability-summary --dir target/vmc-captures/runs/stability --output target/vmc-captures/runs/stability/summary.md
```

## verify

`verify` は frontend と Rust check を意図的に直列実行します。1 つの検証結果が欲しい時に、frontend build、Tauri asset generation、Rust workspace check を手動で並列実行しません。

## release package

`make-release-package` は zip artifact を ignored path の `release-packages/` に書きます。portable package は一度 `target/release/package/` に stage してから zip 化します。

`--version` を省略した場合は `apps/un-motion-supervisor/package.json` を使います。`--skip-build` は build 済み artifact を package する場合に使い、`--keep-staging` は stage tree を確認したい時に使います。

zip には Supervisor client、README、license、third-party notices、`LICENSES/`、dependency license report、config example、Native MediaPipe DLL、MediaPipe task model、Windows 用 `Start UN Motion Supervisor.bat` launcher、小さな package manifest が含まれます。launcher は Supervisor client を起動します。

Native runtime file は `un-motion-supervisor.exe` と同じ package root に置きます。`opencv_world3410.dll` が入る release では、OpenCV 3.4.10 の license / notice も配布物に含めます。

- `un-motion-mediapipe.dll`
- `opencv_world3410.dll`
- `models/*.task`

## license report

`license-report` は `Cargo.lock` / `cargo metadata` と `apps/un-motion-supervisor/package-lock.json` から `THIRD_PARTY_DEPENDENCIES.md` を生成します。

```sh
cargo xtask license-report
cargo xtask license-report --output target/license-report/THIRD_PARTY_DEPENDENCIES.md
```

`make-release-package` は同じ report を package root に自動生成して同梱します。この report は配布前 review 用の一覧であり、法的判断そのものではありません。

## MediaPipe native

`mediapipe build-native` は `native/mediapipe/mediapipe-pin.toml` を読みます。MediaPipe source、model file、Bazelisk、Python shim、生成 DLL / import library は ignored local path に置きます。

tracked source of truth は U.N. Motion bridge、pin file、Rust ABI / loader、xtask orchestration です。

## core smoke

`core smoke` は isolated temporary workspace で real `un-motion-core` API process を起動し、desktop launch API を短命の xtask child で確認します。その後 no-input smoke profile を書き、start / snapshot / stop を実行して process を終了します。

packaged または release build 済み core binary を確認する場合は `--core-exe` を使います。

`core lifecycle-smoke` は同じ workspace を閉じて再度開き、active profile の永続化と desktop launch を確認します。manual tray check 前の install / run / exit / reopen guard として使います。

`core vmc-smoke` は VMC input source と VMC output target を設定し、synthetic Head と ARKit blendshape OSC bundle を送ります。output に Head、`eyeBlinkLeft`、`jawOpen` が含まれなければ失敗します。

`core vmc-mirror-smoke` は VMC mirror correction を有効にした経路です。Waidayo 風の mirrored root、Head、LeftHand、left-eye blink bundle を送り、output に flipped root/head X、RightHand、`eyeBlinkRight`、`jawOpen` が含まれることを確認します。

## live interop bench

`core external-vmc-smoke` は live interop bench 用です。isolated core を起動し、外部 VMC sender を `--listen` で受け、`--target` へ出力し、`--observe` で観測した VMC を検証します。

現在の Warudo / VMC / Waidayo to VSeeFace bench では、`--target` は VSeeFace input `39551`、`--observe` は VSeeFace output `39571` です。

```sh
cargo xtask core external-vmc-smoke --listen 127.0.0.1:39550 --target 127.0.0.1:39551 --observe 127.0.0.1:39571 --label warudo-to-vseeface
cargo xtask core external-vmc-smoke --listen 127.0.0.1:39560 --target 127.0.0.1:39551 --observe 127.0.0.1:39571 --label wmc-to-vseeface
cargo xtask core external-vmc-smoke --listen 192.168.13.13:39540 --target 127.0.0.1:39551 --observe 127.0.0.1:39571 --label waidayo-to-vseeface --mirror
cargo xtask core external-vmc-stability --listen 127.0.0.1:39560 --duration-ms 5000 --label unmotion-wmc --output target/vmc-captures/runs/stability/unmotion-wmc.json
cargo xtask core external-vmc-compare --listen 127.0.0.1:39560 --duration-ms 5000 --label wmc
cargo xtask core external-ifacialmocap-smoke --listen 192.168.13.13:49983 --target 127.0.0.1:39551 --observe 127.0.0.1:39571 --label ifacialmocap-to-vseeface
```

`core external-vmc-stability` は isolated core output を tool-owned UDP port に送り、標準の `vmc stability` JSON report を出します。VSeeFace など receiver 側 smoothing や再出力を測定経路から外したい時に使います。

`core external-vmc-compare` は外部 sender の datagram を受け、自分で isolated core input port へ転送し、同じ input stream から direct / core-output の paired stability report を記録します。source が run 間で変わるため別 capture 比較が危険な時に使います。

`core external-ifacialmocap-smoke` は起動済み iFacialMocap UDP sender 用の live interop guard です。iFacialMocap source を設定し、正式な Modifier / Output path を通して VSeeFace で観測した VMC signal を検証します。

## VMC capture と stability

`vmc capture-frame --collect-ms 25` は最初の VMC frame boundary 後も短時間記録を続けます。logical frame を複数 UDP datagram に分ける sender を確認する時、特に blendshape や Perfect Sync name を見る時に使います。

`vmc stability` は一定時間 live VMC/OSC を記録し、packet count、decoded frame count、per-bone sample rate、interval jitter、最大 position step、最大 rotation step、per-blendshape value step を含む JSON report を出します。

```sh
cargo xtask vmc stability --addr 127.0.0.1:39550 --duration-ms 5000 --source-id warudo --output target/vmc-captures/runs/stability/warudo.json
cargo xtask vmc stability --addr 127.0.0.1:39560 --duration-ms 5000 --source-id wmc --output target/vmc-captures/runs/stability/wmc.json
cargo xtask core external-vmc-stability --listen 127.0.0.1:39560 --duration-ms 5000 --label unmotion-wmc --output target/vmc-captures/runs/stability/unmotion-wmc.json
cargo xtask core external-vmc-compare --listen 127.0.0.1:39560 --duration-ms 5000 --label wmc
cargo xtask vmc stability-summary --dir target/vmc-captures/runs/stability --output target/vmc-captures/runs/stability/summary.md
```

`unmf stability` は live UNMotionFrame/Zenoh output を直接記録し、VMC/OSC 変換を通さず同種の step-size report を出します。MediaPipe stability で reference path が `Engine -> UNMotionFrame -> UNMF/Z` の場合に使います。

```sh
cargo xtask unmf stability --key un-motion/frame --duration-ms 5000 --output target/unmf-stability/dev1.json
```

`vmc stability-summary` は複数の stability JSON report を読み、source-to-source comparison 用の Markdown table を出します。filter tuning 前に、悪い bone と timing jitter を一覧化するために使います。

## research command

`research penn-action prepare` は Penn Action を `target/research/penn-action` に download / extract し、`manifest.json` を書き、repeatable file-video filter evaluation 用に小さい subset を 320px 幅 mp4 へ変換します。詳細は [research-video-sources.md](research-video-sources.md)。

`research ffmpeg prepare` は研究用 media conversion の開発者向け補助です。release build に FFmpeg は同梱しません。通常の file-video user は Supervisor Settings で自分の `ffmpeg(.exe)` path を指定します。
