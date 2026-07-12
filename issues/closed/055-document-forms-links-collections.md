---
number: 055
slug: document-forms-links-collections
parent: 016
status: closed
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

## 実装メモ（実装済み・2026-07-12）

- `src/js/dom_bootstrap.js` の `Document` に live HTMLCollection getter `forms` / `links` /
  `images` / `anchors` を追加。共通ヘルパ `collectElements(root, predicate)`（tree 順走査）と
  `makeHTMLCollection(collect)`（`collect()` を毎アクセス再実行する Proxy）で構成。
  - `length` / 整数 index / `item(i)` / `namedItem(name)` / iteration に対応。
  - 名前アクセスは `id`（全要素）優先、`name`（`COLLECTION_NAME_TAGS`: A/AREA/FORM/IMG/OBJECT/EMBED/IFRAME/INPUT/MAP）を次点で解決。
  - `collect()` を毎回呼ぶため、コレクション参照を保持していても DOM 変更が反映される（live）。
  - `links` は `hasAttribute("href")` の `<a>`/`<area>`、`anchors` は `hasAttribute("name")` の `<a>` のみ。
  - 走査 root は `this`（当該 Document）なので iframe contentDocument / `createDocument` 由来の
    独立文書でも自文書スコープで解決し、メイン文書と混在しない。
- テスト（`src/js/mod.rs`）: `document_forms_indexed_named_and_live` /
  `document_links_filters_href_and_keeps_tree_order` / `document_images_and_anchors_collections` /
  `document_collections_scoped_to_each_document` / `acid3_test4_and_test5_document_forms_and_links_regression`。
- 併せて FAITHFUL harness（`tests/acid3_common/harness.rs`）の stall 判定を
  `has_pending_timers()` を条件に追加して修正。`document.links` 解決で新規発火した test 80 の
  retry ループにより FAITHFUL が index 80 で停止する退行を解消（両モード index 100 到達）。
- 実測: `cargo run --example acid3` は FAITHFUL / DIRECT 両モードで 90/100。test 4/5 が PASS。
  test 80 は retry を経て `timeout -- could be a networking issue`（linktest onload 未発火, 016-14 系）で残存。

## 関連

- 016 Acid3 対応（test 4, 5）
- 054 table tree construction（本 issue の前提。tbody 側は解消済み）
