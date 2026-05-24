# Pose fixtures

このディレクトリには、MediaPipe Native + post-process の静止画回帰確認に使うポージング画像を置く。

| ファイル | 用途 |
| --- | --- |
| `pose-1.png` | 正面Headの基本姿勢。 |
| `pose-2.png` - `pose-4.png` | やや右上を向いたHead姿勢の診断用。 |
| `pose-5.png` | 正面Head + 指の確認用。 |
| `pose-6.png` | Face由来のHead推定が限界に近づく左向き姿勢。親指が緩やかに開いた診断用。 |
| `pose-7.png` | 両手の手の平をカメラへ向けた姿勢。`palm.normal.z` と Hand 出力の手の平軸が左右とも正になり、親指 spread が中立付近になることを確認する。 |
| `pose-8.png` | `pose-7` から手の平を顔側へ回した診断用姿勢。MediaPipe が手の平向きを誤認しやすいため、手の平向きの厳格チェックは置かない。 |
| `pose-9.png` | `pose-7` / `pose-8` の系列で手の平を後ろへ向けた姿勢。`palm.normal.z` と Hand 出力の手の平軸が左右とも負になることを確認する。 |
| `pose-10.png` | 両手の親指を限界まで開いた L 字姿勢。左右とも親指 spread が正の開きとして出ることを確認する。 |
| `pose-I.png` | I ポーズ。腕を体側へ下ろした calibration / neutral 確認用。 |
| `pose-T.png` | T ポーズ。腕を左右水平に伸ばした calibration 確認用。 |
| `pose-T-wrist-front.png` | T ポーズ派生。手首と手の向きの確認用。 |
| `pose-U.png` | U ポーズ。デスクトップ/Webcam 向けの calibration 確認用。 |

`fixtures.toml` で各画像の用途とHead信号の確認条件を管理する。正面画像は `head.yaw` / `head.pitch` / `head.roll` の絶対値を確認し、横向きや右上向きの画像は診断値として出力する。

MediaPipe Native DLL と model がローカルにある環境では、次のコマンドで一括 probe できる。

```powershell
cargo xtask mediapipe pose-fixtures
```

`fixtures.toml` がない場合、既定では各画像の `head.pitch` が `±0.25` を超えたら失敗する。manifest 内で `max-abs-head-pitch` を持つ画像の閾値は必要に応じて変更できる。

```powershell
cargo xtask mediapipe pose-fixtures --max-abs-head-pitch 0.35
```

Head推定の調査では、`--head-diagnostics` を付けると `all` / `face` / `pose` の source 別に `head.yaw` / `head.pitch` / `head.roll` と Head quaternion 由来の Front ベクトルを出力できる。

```powershell
cargo xtask mediapipe pose-fixtures --head-diagnostics
```

この確認は native MediaPipe 依存のため、通常の unit test には含めない。
