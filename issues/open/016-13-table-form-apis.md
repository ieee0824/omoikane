---
number: 016-13
slug: table-form-apis
parent: 016
status: open
---

# HTMLTableElement / Form / Input / Select / Button API

## 目的

table 系および form 系の DOM API を実装する。bucket4、最大 12 点。

## 背景（GAP_ANALYSIS.md セクション3 領域K/L）

- Table API（領域K）: test 29,49,50,51（4 点）
- Form/Input/Select/Button（領域L）: test 52〜59（8 点）
- 実測でも test 49「tBodies」、test 53「name attribute wrong」、test 59「<button> type=submit」等で失敗。

## スコープ

- HTMLTableElement: `rows` / `tBodies` / `caption` / `createCaption` / `insertRow` /
  `deleteRow` / `rowIndex` + thead/tfoot/tbody 自動振り分け
- HTMLFormElement: `elements`（live, 名前/index アクセス）/ `length`
- HTMLInputElement: name/type/value の IDL 反映、checked のライブ状態、radio group 排他
- HTMLSelectElement: `add()` / `options` / `selectedIndex`、HTMLOptionElement.defaultSelected
- HTMLButtonElement: 既定 type=submit、value ≠ textContent 分離

## 受け入れ条件

- 上記 API が仕様どおり動く単体テスト
- Acid3 test 29,49-59 の前提を満たす

## 進捗（部分実装 — closed にはしない）

### 完了済み（`src/js/dom_bootstrap.js`、テストは `src/js/mod.rs`）

`wrapNode` を要素タグ名で専用サブクラスへ振り分ける仕組み（`ELEMENT_CTORS`）を追加し、以下を実装:

- **HTMLTableElement**: `caption`/`tHead`/`tFoot`/`tBodies`/`rows` ゲッタ、`createCaption`/`createTHead`/`createTFoot`、`deleteCaption`/`deleteTHead`/`deleteTFoot`。setter は Acid3 の自己代入（no-op）のみ対応。
- **HTMLFormElement**: `elements`（live collection、index/`length`/名前・id アクセス、未存在の名前は null）、`length`。
- **HTMLInputElement**: `name` 反射、`type`（既定 text、小文字化）、`value` を JS 保持の dirty value 化（content 属性へ反射しない）＋ `defaultValue`。
- **HTMLButtonElement**: `type` 既定 "submit"（submit/reset/button/menu のみ許容）。
- **HTMLLabelElement**: `htmlFor` ↔ `for` 属性反射。
- **HTMLMetaElement**: `httpEquiv` ↔ `http-equiv` 属性反射。
- **HTMLSelectElement**: `add(element, before)`/`options`/`selectedIndex`/`length`/`remove`。
- **HTMLOptionElement**: `defaultSelected`(↔selected 属性)/`selected`(dirty)/`value`/`text`。
- Node 共通に `name` 反射と `hasChildNodes()` を追加。
- 結果: Acid3 test **49,53,57,58,59,62 が PASS**（test 62 は label.htmlFor / meta.httpEquiv 補完で通過）。

### 残項目

- **insertRow / deleteRow / rowIndex / sectionRowIndex**: test 50/51 が要求。`table.insertRow(0)` や `tSection.insertRow(0)` が未実装で「not a callable function」。test 51 は thead/tfoot/tbody をまたいだ行のツリー順序を厳密にアサートするため、単なる insertRow 追加だけでなく section 振り分け・rowIndex/sectionRowIndex・rows の正確なツリー順序が必要（M〜L 規模）。今回は未着手。
- **HTMLTableSectionElement**（thead/tbody/tfoot の `rows`/`insertRow`/`deleteRow`）: 上記に付随。
- radio group 排他・checkbox の dirty checkedness（test 55/56）: iframe 依存（016-9）でもあり未着手。
- test 52/54: `document.write` で生成される parsed form/input に依存（016-7）。
