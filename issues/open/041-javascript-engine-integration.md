---
number: 041
slug: javascript-engine-integration
status: open
---

# JavaScript エンジン統合

## 概要

boa_engine (0.21.0) による JS エンジンは既に統合済み。残りの DOM/Web API バインディングを実装し、`<script>` タグの自動実行を可能にする。

## 実装済み

- boa_engine 依存追加済み、`src/js/mod.rs` (949行)
- `document.getElementById`, `querySelector`
- `element.getAttribute`, `setAttribute`
- `element.appendChild`, `createElement`
- `element.parentNode`, `nodeName`
- `console.log`
- `setTimeout`, `setInterval`, `clearTimer`
- `fetch` API
- `location.href`, `navigator.userAgent`
- `Event` クラス、DOM ツリー登録・キャッシュ

## 子 issue

- [041-1](041-1-classlist-style-bindings.md): className/classList/style バインディング
- [041-2](041-2-event-listeners.md): addEventListener と DOMContentLoaded
- [041-3](041-3-script-tag-execution.md): `<script>` タグの自動実行
- [041-4](041-4-intersection-observer.md): IntersectionObserver（モダンサイト必須）

## 優先度

高 — 041-1 → 041-2 → 041-3 の順で、041-3 完了後にモダンサイトの JS が動く
