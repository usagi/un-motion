# UN Motion ブランドアセット

| ファイル | 用途 |
| --- | --- |
| `un-motion-artwork-supervisor.png` | Supervisor Console、トレイ、アプリ外枠、プロファイルのフォールバックアイコンで使う UN Motion のメインアートワーク。 |
| `un-motion-artwork-capturer.png` | Capturer Process 用のアートワーク。Capturer の実行ファイルやトレイの識別に使い、Supervisor の外枠には使わない。 |
| `../icons/un-motion-capturer.ico` | Capturer Process 実行ファイルへ埋め込む Windows アイコン。 |

アートワークのファイル名は、小文字の kebab-case で `un-motion-artwork-<role>.png` の形式にそろえる。

## アイコン一式の再生成

リポジトリルートから、マスター PNG をソースに Tauri 公式の `icon` コマンドで再生成できる。

**Supervisor**（`bundle.icon` は `icons/icon.ico` のみ参照）:

```powershell
Push-Location apps\un-motion-supervisor
npx @tauri-apps/cli@2 icon ..\..\assets\brand\un-motion-artwork-supervisor.png
Pop-Location
```

Supervisor のウィンドウ／トレイ画像は `apps/un-motion-supervisor/src-tauri/src/lib.rs` が `assets/brand/un-motion-artwork-supervisor.png` を `include_bytes!` している。
