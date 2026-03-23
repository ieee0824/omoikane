---
number: 041-3
slug: script-tag-execution
parent: 041-javascript-engine-integration
status: open
---

# `<script>` タグの自動実行

## 概要

HTML パース時に `<script>` タグの内容を JS エンジンで自動実行する。

## 対象

### インラインスクリプト
- `<script>alert('hello')</script>` — テキスト内容を実行
- `<script type="text/javascript">` — type 属性の確認

### 外部スクリプト
- `<script src="app.js"></script>` — HTTP フェッチして実行
- 相対 URL の解決（base URL 使用）

### 実行タイミング
- パーサーがスクリプト要素を検出した時点で同期実行
- `defer` 属性: DOM 構築完了後に実行
- `async` 属性: フェッチ完了後に即実行（初期実装では defer と同じ扱い）

## 修正箇所

- `src/paint/mod.rs` or `src/paint/stylesheet.rs` — render_document 内で JS 実行フェーズを追加
- `src/js/mod.rs` — スクリプト収集・実行のエントリポイント
- `src/cdp/mod.rs` — navigate 時に JS を実行

## 注意

- 無限ループ防止（実行タイムアウト）
- XSS/SSRF 防止（fetch の same-origin チェック）
