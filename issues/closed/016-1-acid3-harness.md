---
number: 016-1
slug: acid3-harness
parent: 016
status: closed
---

# Acid3 ローカル実行ハーネス

## 目的

Acid3 をローカルで再現実行し、スコア／失敗ログを継続的に観測できる基盤を用意する。
以降の子issueの効果を実測で検証するための土台。

## 背景

- Acid3 は各リソースの HTTP ステータス／Content-Type の正確な値に依存する
  （`empty.css` を `text/html` で返す、`support-a.png` は 404 等）。
- 本物のソースをベンダリングし、`manifest.json` を真実の source としてローカル配信する。

## スコープ

- `examples/acid3.rs` — フィクスチャ配信サーバ + ドライバ + スコア表示（Faithful / DirectDrive）
- `tests/acid3_harness.rs` — 配信ヘッダ・DOM パースの回帰テスト（スコア非依存）
- `tests/acid3_common/harness.rs` — 共有ハーネス（`#[path]` 取り込み）
- `tests/fixtures/acid3/` — 本物のフィクスチャ一式 + `manifest.json` + `README.md`

## 受け入れ条件

- `cargo run --example acid3` でスコア取得を試行できる
- `cargo test --test acid3_harness` が配信ヘッダ・パースを検証する

## 結果

- コミット `9aee4d4`「Acid3 実行ハーネスと本物のフィクスチャ一式を追加」で完了。
- ベースライン実測（エンジン無改変時点）は **0/100**（スコア表示到達不可）と記録。
  根本原因はトークナイザに script-data/raw-text 状態が無く、`<script>` 本体JS(約173KB)が
  複数断片に分割され SyntaxError となること（→ 016-2 で解消）。
