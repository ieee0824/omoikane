---
number: 016-12
slug: dom2-core-namespaces
parent: 016
status: open
---

# DOM2 Core / 名前空間 / DOMException

## 目的

createElementNS、document.implementation、DOMException（`.code` + 定数）、名前検証を実装する。9 点。

## 背景（GAP_ANALYSIS.md セクション3 領域I、セクション1 P9）

- test 8,19,20,21,22,23,25,26,98（9 点）が該当。
- **DOMException（`.code` + 定数）は 8/11/19/20/22/23/25 に横断的に効く基盤**なので先行実装推奨。
- 実測でも test 19「DOCUMENT_FRAGMENT_NODE constant missing」、test 20/22/23「例外が投げられない/違う」、
  test 25「exceptions don't have all the constants」で失敗している。
- なお 016-2 の副次対応で appendChild/insertBefore に**循環検出（HierarchyRequest）**を追加済み
  （`src/dom/mod.rs`、DOM が cyclic になるスタックオーバーフローを防止）。本issueでは
  これを DOMException として JS に throw する形へ発展させる。

## スコープ

- `createElementNS` + prefix / localName / namespaceURI
- `implementation.createDocument` / `createDocumentType`
- `DOMException`（`.code` + 定数群: `HIERARCHY_REQUEST_ERR` 等）
- 名前検証（NUL バイト・不正タグ名で `INVALID_CHARACTER_ERR` 等）
- appendChild 循環検出を HIERARCHY_REQUEST_ERR の throw に接続

## 受け入れ条件

- DOMException が `.code` と定数群を持ち、各操作が正しい例外を投げる
- createElementNS / implementation.createDocument が動く
- Acid3 test 8,19-23,25,26,98 の前提を満たす
