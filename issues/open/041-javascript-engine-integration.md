---
number: 041
slug: javascript-engine-integration
status: open
---

# JavaScript エンジン統合

## 概要

JS エンジンを統合し、`<script>` タグの実行とDOM操作を可能にする。

## 背景

- html5test.co 等のJS必須サイトが全く動作しない
- モダンサイトの多くがJSでDOM操作（クラス追加、要素生成）を行う
- animation の JS トリガー（IntersectionObserver 等）が動かない

## フェーズ

### Phase 1: 基本的なDOM API
- `document.getElementById`, `querySelector`
- `element.className`, `classList.add/remove`
- `element.style` の読み書き

### Phase 2: イベントとタイマー
- `addEventListener`
- `setTimeout`, `setInterval`
- `DOMContentLoaded` イベント

### Phase 3: 高度なAPI
- `IntersectionObserver`
- `fetch` API
- `localStorage`

## 優先度

高 — 開発フェーズ4に相当する大規模タスク
