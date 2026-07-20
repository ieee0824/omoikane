---
number: 044-1
slug: event-dom-js-polyfills
status: closed
parent: 044
---

# イベント・DOM補完（JS側ポリフィル）

## 概要

JS側だけで完結するWeb APIのスタブ・ポリフィルを一括追加し、実サイトのJS初期化エラーを大幅に削減する。

## 追加 API

### イベント系
- `preventDefault()`, `stopImmediatePropagation()` on Event
- `CustomEvent` クラス（detail プロパティ）
- `MouseEvent`, `KeyboardEvent` クラス（基本プロパティ）
- `document.createEvent()` レガシーイベント生成

### DOM 補完
- `attributes` NamedNodeMap Proxy
- `dataset` Proxy（data-* 属性）
- `innerText`（textContent に委譲）
- `isConnected`
- `document.createComment()`
- `document.readyState`

### Window/Global スタブ
- `requestAnimationFrame` / `cancelAnimationFrame`（同期 stub）
- `alert` / `confirm` / `prompt`（no-op stub）
- `window.innerWidth` / `innerHeight`
- `matchMedia()`（stub: matches=false）
- `localStorage` / `sessionStorage`（in-memory stub）
- `console.warn` / `error` / `info` / `debug`
