---
number: 055
slug: document-forms-links-collections
parent: 016
status: open
---

# document.forms / document.links の HTMLCollection

## 概要

`Document` の HTMLCollection である `document.forms` と `document.links`（および関連の
`document.images` / `document.anchors`）を実装する。名前付きアクセス（例: `document.forms.form`）も
含む。

## 背景

- 054（パーサの暗黙 tbody 生成）実装後、Acid3 test 29 は PASS したが、test 4 / test 5 は
  `document.forms` / `document.links` が `undefined` のため残存すると実測で特定された。
  - **test 4**（NodeIterator の空白スキップ walk）: expectation 25 の `document.forms[0]` で
    `cannot convert 'null' or 'undefined' to object` を throw。tbody 参照（expectation 28 付近）に
    到達する前に失敗する。
  - **test 5**（TreeWalker の空白スキップ walk）: 054 で暗黙 tbody 前提（expectation 11）は通過し、
    最後の expectation 23 `document.links[1].firstChild` で同じ throw に到達する。
- 現状 `src/js/mod.rs` には `document.getElementsByTagName` はあるが、`document.forms` /
  `document.links` プロパティ自体が未定義。

## スコープ

- `document.forms`: 文書内の全 `<form>` を tree 順で返す live HTMLCollection。
  index アクセスと名前アクセス（`name` / `id`）の両方。
- `document.links`: `href` 属性を持つ `<a>` および `<area>` を tree 順で返す live HTMLCollection。
- （任意）`document.images`（`<img>`）, `document.anchors`（`name` を持つ `<a>`）も同時整備。
- iframe の contentDocument でも同様に解決できること。

## 受け入れ条件

- `document.forms.length` / `document.forms[0]` / `document.forms.form`（名前アクセス）が動く。
- `document.links.length` / `document.links[1]` が `href` を持つ `<a>`/`<area>` を tree 順で返す。
- Acid3 test 4 / test 5 が PASS する（88 → 90 を見込む）。

## 関連

- 016 Acid3 対応（test 4, 5）
- 054 table tree construction（本 issue の前提。tbody 側は解消済み）
