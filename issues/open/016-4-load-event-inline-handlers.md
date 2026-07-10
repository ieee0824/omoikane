---
number: 016-4
slug: load-event-inline-handlers
parent: 016
status: open
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
