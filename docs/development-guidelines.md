# 開発ガイドライン

本ファイルは、UNMotion 開発における基本ルールを定義する。

## プロジェクト識別

- author signature: usagi / USAGI.NETWORK
- license: MIT

## 言語方針

- 文書とコードコメントは日本語を基準とする。
- 明確性や相互運用性のために必要な場合は English を許容する。

## スタイルとワークフローの参照元

参照リポジトリ:

- usagi/virtual-avatar-connect（v2 ブランチを最優先）
- usagi/un-virtual-eye-tracker

本リポジトリのルールは、上記を基に初期フェイズ向けに調整している。

ブランチ差異がある場合は、active 開発である virtual-avatar-connect の v2 を優先する。

## 基本ルール

- 変更は小さく焦点を絞る。
- 1 commit あたり 1 つの機能目的を推奨する。
- 互換レイヤー実装では clean-room 互換境界を尊重する。
- proprietary SDK header、流出コード、ライセンス不明ソースを含めない。

## ブランチと push 方針（現フェイズ）

- 当面は main へ直接作業する。
- 小さな単位で commit/push する。
- チーム規模や release リスク増加時に運用を再評価する。

## フォーマットとファイルルール

- Rust フォーマットはリポジトリの .rustfmt.toml を使用する。
- Rust 以外のインデントは簡潔かつ一貫性を保つ。
- 文書とソースは UTF-8 エンコードを使用する。
- テキストファイルの改行は LF で統一する。

## 必須ローカルチェック

push 前に以下を実行する:

```sh
cargo xtask verify
```

`xtask verify` は frontend build -> Rust format check -> Rust workspace tests を直列実行する。
frontend build と Rust/Tauri 系コマンドを手で並列実行しない。

## リポジトリ内補助コマンド

リポジトリ内部で完結する小規模なスクリプト、実験、研究用コマンドは `crates/xtask` に追加する。

- 標準導線は `cargo xtask <command>` とする。
- PowerShell は公式導線に追加しない。
- Node.js script は解析や比較処理の実体として残してよいが、複数 step の orchestration は `xtask` に寄せる。

## アーキテクチャ境界

モジュール境界を明確に保つ:

- input adapters
- core frame と normalization logic
- mapping / filtering stages
- output adapters
- desktop runtime

runtime 固有詳細を portable frame 型へ漏らさない。

## 文書化要件

挙動変更や不具合修正時は以下を行う:

- テスト追加または更新
- 変更理由とトレードオフを記録
- docs 配下の関連文書を更新

互換レイヤー変更時は以下を明記する:

- 観測ソース
- 置いた仮定
- 意図的に未実装とした項目
