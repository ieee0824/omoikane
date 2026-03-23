---
number: 043
slug: dom-api-expansion-for-jquery
status: open
---

# DOM API 拡張（jQuery 互換）

## 概要

jQuery 1.x が最低限動作するために必要な DOM API バインディングを追加する。

## 背景

- 042 で JS 実行をレンダリングパイプラインに統合済み
- kasaneteto.jp で jQuery が読み込まれるが `TypeError: cannot convert 'null' or 'undefined' to object` で失敗
- jQuery が使う基本 DOM API が不足

## 追加すべき API

### Node プロパティ
- `textContent` getter/setter
- `innerHTML` getter/setter
- `childNodes`, `children`, `firstChild`, `lastChild`
- `nextSibling`, `previousSibling`
- `nodeType`
- `tagName` (uppercase)
- `ownerDocument`

### Node メソッド
- `removeChild(child)`
- `insertBefore(newNode, refNode)`
- `cloneNode(deep)`
- `querySelectorAll(selector)`
- `hasAttribute(name)`
- `removeAttribute(name)`

### Document メソッド
- `createDocumentFragment()`
- `createTextNode(text)`
- `getElementsByTagName(tag)`
- `getElementsByClassName(cls)`
- `document.body`, `document.head`, `document.documentElement`

### Window API
- `getComputedStyle()` (stub)
- `setTimeout`/`clearTimeout` (同期実行 stub)

### ネイティブバインディング（Rust 側）
- `__omoikane_child_node_ids`
- `__omoikane_clone_node`
- `__omoikane_create_text_node`
- `__omoikane_get_text_content`
- `__omoikane_set_text_content`
- `__omoikane_set_inner_html`
- `__omoikane_insert_before`
- `__omoikane_next_sibling`
- `__omoikane_previous_sibling`
- `__omoikane_query_selector_all`
- `__omoikane_remove_attribute`
- `__omoikane_remove_child`
- `__omoikane_node_type`

## 注意

- cbindgen が js/mod.rs 内の大きな JS 文字列でパースエラーになるため、DOM_BOOTSTRAP を別ファイル (dom_bootstrap.js) に外出しする必要がある
- Copilot レビュー指摘: JS 実行後のスタイルシート再抽出失敗時のフォールバック、Web フォント再取得が未対応（042 の残課題）
