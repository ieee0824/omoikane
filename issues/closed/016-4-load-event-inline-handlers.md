---
number: 016-4
slug: load-event-inline-handlers
parent: 016
status: closed
---

# load イベント発火と on* インラインハンドラ配線

## 目的

`load` イベントをパイプラインから発火し、`on*` インライン属性（最低限 `body onload`）を
リスナに配線する。Acid3 ドライバ起動の最後のピース。

## 背景（GAP_ANALYSIS.md セクション1 P1、セクション4 フェーズ0）

- `<body onload="update()">` が評価されず、そもそも `update()` が 1 回も呼ばれない。
- `on*` 属性をリスナに配線する処理が皆無で、`load` もパイプラインから発火されない
  （`src/paint/mod.rs` は `execute_document_scripts` を 1 回呼ぶのみ）。

## スコープ

- `load` イベントのパイプライン発火
- `on*` インライン属性（最低限 `body onload`）のリスナ配線
- Text/Comment ノードの `.data` アクセサ
- `document.defaultView`
- `Node` 定数群（`ELEMENT_NODE` 等）と `localName`

## 受け入れ条件

- `<body onload="...">` が読み込み時に実行される
- `node.data` / `document.defaultView` / `Node.ELEMENT_NODE` 等が参照できる
- Acid3 が onload 経由で自走起動する（016-3 と併せて）

## 完了メモ

- **on* インライン属性配線** (`wire_inline_event_handlers`): DOM を走査し、`on`
  で始まる属性値を `function (event) { ... }` のボディとしてコンパイル（`new Function`）
  してリスナ登録する。`<body>`/`<frameset>` の window 反射イベント
  （load/unload/resize/scroll/blur/focus/error/hashchange 等）は window に、
  それ以外は要素自身に登録する。属性名の列挙のためネイティブ primitive
  `__omoikane_attribute_names` を追加。
- **load イベント発火** (`fire_load`): `window.dispatchEvent(new Event('load', {bubbles:false}))`。
  HTML 順序（スクリプト実行 → DOMContentLoaded → load）に従い、
  `execute_document_scripts`（DOMContentLoaded 発火）の後、タイマーポンプの前に
  パイプライン（`src/paint/mod.rs`）とハーネスから呼ぶ。ハーネスの手動 `update()`
  エミュレーションは撤去し、実 load イベント経由の自走起動に置き換えた。
- **CharacterData.data**: `Text`/`Comment` クラスを新設（`wrapNode` が nodeType
  3/8 で生成）し、`CharacterData` に `data` get/set（textContent 相当）と `length`
  を実装。Element には `.data` を露出しない（`HTMLObjectElement.data` 等と衝突させない）。
- **document.defaultView** → `globalThis`。
- **Node 定数群** (`ELEMENT_NODE`〜`NOTATION_NODE`) を Node コンストラクタと
  プロトタイプ両方に。**localName**: 要素は小文字タグ名、非要素は null。

### スコープ外にした事項（残タスク）
- `element.onclick` 等の **on* IDL プロパティ**（属性ではなくプロパティ経由の
  ハンドラ get/set）は未対応。今回は content 属性のみ配線。
- インラインハンドラのスコープチェーン（HTML 仕様の document/element/form を
  スコープに含める挙動）は未実装。`new Function` によりグローバル + `event` 引数
  のみ参照可能。Acid3 の `onload="update()"` / `onclick="report(event)"` には十分。
- Acid3 スコア: 26 → 27（test 99 が `.data` により PASS）。test 19 は定数チェックを
  通過するようになったが、DOMException（`e.code`/`HIERARCHY_REQUEST_ERR`）未実装のため
  依然 fail（016-12 スコープ）。
