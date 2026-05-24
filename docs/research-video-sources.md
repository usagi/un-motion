# 研究用動画ソース

UNMotion のフィルタ評価では、実データ本体を Git に入れません。公開用 repo には、Git で管理されている fixture、再取得できる dataset の手順、評価 command だけを残します。

失われた local dataset の運用文書は残しません。`target/` 配下の capture や reference は消える前提の作業領域です。

## pose fixture

Git 管理されている静止画回帰確認は `tests/pose` を使います。手の平向き、T-wrist-front、U/Twf/I calibration など、v1 で重要な姿勢はここに置きます。

```sh
cargo xtask mediapipe pose-fixtures
cargo xtask mediapipe pose-fixtures --head-diagnostics
```

## Penn Action

Penn Action は University of Pennsylvania の action dataset で、2326 個の画像シーケンスと全フレームの 2D joint annotation を含む。公式ページは <https://dreamdragon.github.io/PennAction/>。UNMotion の上半身 VMC 評価には遠景、全身、手の小ささが合わないため、主評価 dataset にはしない。

取得、展開、manifest 作成、先頭 sequence の 320px 幅 mp4 化:

```sh
cargo xtask research penn-action prepare
```

mp4 化には `ffmpeg` が必要。これは研究用のローカル変換補助であり、リリース配布物には
同梱しない。Windows 開発環境では必要に応じて repo の `target/tools` に用意できる:

```sh
cargo xtask research ffmpeg prepare
```

まず dataset 本体と manifest だけ用意する場合は次でよい:

```sh
cargo xtask research penn-action prepare --skip-videos
```

出力先:

```text
target/research/penn-action/
  archive/Penn_Action.tar.gz
  raw/Penn_Action/
  videos/320w/
  manifest.json
```

`manifest.json` は各 sequence の frame 数、先頭/末尾 frame、生成済み mp4 を記録する。動画変換は既定で先頭 16 sequence だけ行う。全件変換は重いので、必要な評価 subset が決まってから増やす。

よく使うオプション:

```sh
cargo xtask research penn-action prepare --video-limit 32 --fps 30
cargo xtask research penn-action prepare --skip-download --skip-extract --video-limit 64
cargo xtask research penn-action prepare --skip-videos
cargo xtask research ffmpeg prepare --force-extract
```

腕、肩、胴体の揺れ評価に使う action subset だけを mp4 化する:

```sh
cargo xtask research penn-action prepare --skip-download --skip-extract --video-limit 6 --video-actions jumping_jacks,squat,tennis_forehand,baseball_swing,clean_and_jerk,strum_guitar
```

manifest には `action`、`pose`、`labelFile` も入る。Penn Action の `.mat` 内では
README と少し違う action 名が使われるものがある:

```text
bowling -> bowl
pull_ups -> pullup
push_ups -> pushup
sit_ups -> situp
squats -> squat
strumming_guitar -> strum_guitar
```

action ごとの件数と mp4 化済み sequence は summary で確認できる:

```sh
cargo xtask research penn-action summary
cargo xtask research penn-action summary --action jumping_jacks
```

file-video 評価用の desktop config を生成する:

```sh
cargo xtask research penn-action desktop-config --sequence 0001
cargo xtask research penn-action desktop-config --action jumping_jacks
cargo xtask research penn-action desktop-config --sequence 0016 --width 640 --height 480 --fps 30
```

既定の出力先は `target/research/penn-action/desktop-file-video.toml`。tracked な
雛形は `configs/research-penn-action-file-video.example.toml` に置く。実行時に
使う `conf.toml` はローカル状態なので Git には入れない。

生成した config を実行用 `conf.toml` に入れる場合は `--install` を付ける。既存
`conf.toml` は `target/research/penn-action/conf-backups/` に退避される:

```sh
cargo xtask research penn-action desktop-config --sequence 0001 --install
```

元に戻す場合は最新 backup を `conf.toml` に戻す:

```powershell
Copy-Item target/research/penn-action/conf-backups/conf.<timestamp>.toml conf.toml
```

## 評価方針

- 固定ベンチは短い subset から始める。
- `jumping_jacks`、`squats`、`tennis_forehand`、`baseball_swing`、`clean_and_jerk` のような腕、肩、胴体が動く sequence を優先する。
- Penn Action の annotation は後で action label や joint 基準評価に使えるが、初期段階では file-video 入力の再現性確保を主目的にする。
- repo 公開時は `target/vmc-captures` と `target/research` の実データを含めない。
- local にしかない dataset を前提にした docs は残さない。必要になった時に、Git 管理 fixture または再取得可能な dataset として作り直す。
