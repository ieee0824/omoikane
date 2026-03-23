---
number: 041-1
slug: classlist-style-bindings
parent: 041-javascript-engine-integration
status: open
---

# className/classList/style バインディング

## 概要

JS から要素のクラスとインラインスタイルを操作するための DOM API を実装する。

## 対象 API

### className
- `element.className` getter/setter
- class 属性の読み書き

### classList
- `element.classList.add(cls)`
- `element.classList.remove(cls)`
- `element.classList.toggle(cls)`
- `element.classList.contains(cls)`

### style
- `element.style.property = value` setter
- `element.style.property` getter
- camelCase ↔ kebab-case 変換（`backgroundColor` ↔ `background-color`）

## 背景

- モダンサイトで `element.classList.add('on')` が fade-in アニメーションのトリガー
- `element.style.display = 'none'` で要素の表示/非表示切り替え

## 修正箇所

- `src/js/mod.rs` — ネイティブバインディング追加
- DOM_BOOTSTRAP の Node/Document クラスにプロパティ追加
