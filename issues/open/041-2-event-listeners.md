---
number: 041-2
slug: event-listeners
parent: 041-javascript-engine-integration
status: open
---

# addEventListener と DOMContentLoaded

## 概要

イベントリスナーの登録・発火機構と DOMContentLoaded イベントを実装する。

## 対象 API

### addEventListener
- `element.addEventListener(type, callback)`
- `element.removeEventListener(type, callback)`
- イベントバブリング（target → parent → ... → document）

### イベント発火
- `DOMContentLoaded` — DOM 構築完了時に document で発火
- `load` — ページ読み込み完了時に window で発火
- `click` — CDP 経由のクリックイベント

## 修正箇所

- `src/js/mod.rs` — イベントリスナーストレージ、dispatch 関数
- `src/dom/mod.rs` — ノードにイベントリスナーを保持する仕組み（または js モジュール内で管理）
- `src/cdp/mod.rs` — navigate 後に DOMContentLoaded を発火
