# Omoikane - Headless Browser (Rust)

フルスクラッチで実装するヘッドレスブラウザ。Rustで基盤を構築し、CDP互換APIで他言語から利用可能にする。

## プロジェクト構成

```
issues/          タスク管理
  open/          未完了のissue
  closed/        完了したissue
```

## タスク管理ルール

- タスクは `issues/open/` にmarkdownファイルとして作成する
- 完了時は `issues/closed/` へ移動する
- ファイル名は `{番号}-{slug}.md` 形式（例: `001-http-client.md`）
- 大きいタスクは子issueに分割する
  - 親: `{番号}-{slug}.md` — 子issueへのリンクを持つ（例: `001-http-client.md`）
  - 子: `{番号}-{連番}-{slug}.md` — frontmatterに `parent` を持つ（例: `001-1-http-basic.md`）
  - 子issueがすべてclosedになったら親もclosedへ移動する

## 開発フェーズ

1. HTTPクライアント + HTMLパーサー + DOM構築
2. CSSパーサー + スタイル計算
3. レイアウトエンジン
4. JavaScriptエンジン統合
5. CDP互換API + 他言語連携（C FFI, WebSocket JSON-RPC）

## 相談ルール

- 実装上の相談がある場合、該当issueのmdファイルに `## 相談` セクションを追記する
- やり取りはスレッド形式で記録する（日付・発言者・内容）
- 他の開発者（AIエージェント含む）はissueの相談セクションを確認し、返答を追記する

## タスク着手ルール

- タスクに着手する前に必ず実装プランを提示し、ユーザーの承認を得る
- プランの規模が大きい場合はタスクをさらに子issueに分割してから着手する

## 実装の流れ

1. **テストを書く** — 期待する振る舞いをテストで定義する
2. **ドキュメントを書く** — 公開APIのdocコメントを書く
3. **実装を書く** — テストが通るように実装する

## コミットルール

- コミット前に `cargo test` と `cargo build` が通ることを確認する

## PR・マージルール

- PR 作成後、GitHub Copilot の自動レビューが完了するまでマージしない
- Copilot のレビューコメントを確認し、指摘があれば修正してから push する
- 即修正できない指摘は `issues/open/` に issue を作成して追跡する

## 設計ルール

- データ構造を設計・変更する際は、関連する他のissueを必ず確認し、後工程との整合性を考慮すること

## 技術方針

- 言語: Rust
- 他言語連携: CDP(Chrome DevTools Protocol)互換 + C FFI薄レイヤー
- テスト: Web Platform Testsを参考にする
- 依存: 可能な限り最小限に抑え、コア部分は自前実装
- コア部分の定義: HTMLパーサー、CSSパーサー、レイアウトエンジン、DOM、JavaScriptバインディング等のブラウザエンジン固有機能
- インフラ層（TLS、暗号、圧縮等）はpure Rustクレートの利用を許可する
