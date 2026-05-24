# UN Motion サウンドアセット

このディレクトリには、UN Motion 用に作成したサウンドアセットのマスターを配置する。

- `un-calibration-sound-pun.flac` - キャリブレーションのカウントダウン通知音。
- `un-calibration-sound-pon.flac` - キャリブレーションのサンプリング開始通知音。

Supervisor フロントエンドは Vite ビルド前に、このディレクトリのファイルを
`apps/un-motion-supervisor/public/sounds/` へコピーし、パッケージ用の Web アセットへ含める。
このディレクトリを正本として扱い、public 側のコピーは配布用の生成物として扱う。
