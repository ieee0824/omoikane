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
