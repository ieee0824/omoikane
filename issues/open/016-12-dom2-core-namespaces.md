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

## 進捗（部分実装 — closed にはしない）

### 完了済み（`src/js/dom_bootstrap.js`、テストは `src/js/mod.rs`）

- **DOMException**: `code`/`name`/`message` + 全定数（INDEX_SIZE_ERR=1 〜 DATA_CLONE_ERR=25）をコンストラクタと prototype 両方に付与。`name → code` マップで正しい code を導出。`globalThis.DOMException` として公開。
- **名前検証**: XML 1.0 `NameStartChar`/`NameChar` 準拠の `isValidXmlName`。`createElement` は不正名（`<div>`・`0div`・NUL バイト等）で `InvalidCharacterError`(5) を throw。
- **createElementNS**: DOM3 「validate and extract」実装。`validateQualifiedName`（不正 Name → 5、malformed QName → NamespaceError(14)）＋ prefix/namespace 整合チェック（xml/xmlns の名前空間規則）。生成要素に `namespaceURI`/`prefix`/`localName`/`tagName`/`nodeName` をインスタンスプロパティで付与（大文字化せず修飾名を保持）。
- **appendChild/insertBefore の循環検出**: `__ensureNotAncestor` で自己/祖先挿入時に `HierarchyRequestError`(3) を throw（従来の native no-op を JS 例外化）。
- **createEvent 補完**: `UIEvent` クラス（`initUIEvent(type,bubbles,cancelable,view,detail)`）と `Event.initEvent` を追加。`createEvent('UIEvents'/'MouseEvents'/'KeyboardEvent'/'CustomEvent'/...)` が型別イベントを返す。
- **document.implementation**: `createDocumentType`（malformed QName で NamespaceError）/`hasFeature`/`createHTMLDocument`。
- 結果: Acid3 test **19,20,21,22,23,25,30 が PASS**（28→の増分に寄与）。

### 残項目

- `implementation.createDocument`（独立した Document ツリー生成）: test 8/26/27/98 が要求。native に Document 生成バインディングが無く、Range API（016-11）とも絡むため未実装。test 8/26 は現在「createDocument is not a callable function」で失敗。
- test 98（XHTML/XML DOM）: 016-14 と重複。
