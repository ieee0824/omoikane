---
number: 042
slug: integrate-js-into-render-pipeline
status: open
---

# JS 実行をレンダリングパイプラインに統合

## 概要

`render_document_with_url` および `screenshot` のフローで、HTML パース後・レイアウト前に `execute_document_scripts()` を呼び出し、`<script>` タグを自動実行する。

## 背景

- 041 で JS エンジン・DOM API・`<script>` 実行・IntersectionObserver を実装済み
- しかし `render_document_with_url` のフローに JS 実行フェーズがなく、レンダリング時に JS が動かない
- html5test.co で「ENABLE JAVASCRIPT」と表示される

## 修正箇所

### render_document_with_url (src/paint/mod.rs)
1. HTML パース → スタイルシート抽出の後、レイアウトの前に JS 実行フェーズを挿入
2. `JsRuntime::with_document(document)` でランタイム作成
3. `execute_document_scripts(base_url)` でスクリプト実行
4. JS による DOM 変更（classList.add 等）がレイアウトに反映される

### screenshot (src/screenshot/mod.rs)
- `render_document_or_frameset_canvas` 内でも同様に JS 実行

### CDP navigate (src/cdp/mod.rs)
- 既に `JsRuntime` を持っているが、navigate 後に `execute_document_scripts` を呼んでいるか確認

## 注意

- JS 実行はオプショナルにする（`--no-js` フラグ）
- タイムアウト保護（sandbox config）
- JS エラーでレンダリングを止めない（エラーはログに記録）
