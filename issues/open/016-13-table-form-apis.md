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

### 完了済み（追加分・PR 未マージ、`issue/016-13-table-row-apis`、83→86）

- **HTMLTableElement.insertRow / deleteRow**: HTML 仕様の挿入先決定（空 table は tbody 自動生成、tbody ありは最後の tbody へ、index/-1/末尾は rows コレクション基準の位置決め）。IndexSizeError(code 1) の境界検証も実装。
- **rows ゲッタのツリー順修正**: thead 行 → body 行（table 直下 tr と tbody 内 tr をツリー順で混在）→ tfoot 行 の順序に修正。
- **HTMLTableSectionElement**（thead/tbody/tfoot）: `rows` / `insertRow` / `deleteRow`。
- **HTMLTableRowElement**（tr）: `cells`（td/th をツリー順）/ `rowIndex`（所属 table の rows コレクション内 index、非所属は -1）/ `sectionRowIndex`（親 section 内 index）/ `insertCell` / `deleteCell`。
- **submit ボタン click のフォーム送信**（test 54）: `click()` に活性化挙動を追加し、submit/reset ボタンの click が伝播中止されなければ祖先 `<form>` へ cancelable な `submit`/`reset` イベントを同期発火。
- **on* イベントハンドラ IDL 属性**: `onclick`/`onsubmit`/`onreset` 等を汎用実装（代入で単一リスナー登録・再代入で置換・null で解除）。`onload` は Window 反射のため従来どおり Node 側に個別定義。
- 結果: Acid3 test **50 / 51 / 54 が新規 PASS**（FAITHFUL/DIRECT 両モード 86/100、実測）。test 55/56（checkbox 移動・radio clone の状態保持）は 016-10 のライブ選択状態モデルで既に PASS 済みと実測確認（本 issue では未変更）。

### 残項目

- ~~**test 29**~~: 解消済み（054, 87→88）。原因は DOM API 側ではなく **HTML パーサの table tree construction**（`src/html/tree_builder.rs`）が `<tr>` を tbody へ入れず table 直下に配置しており（"in table body" 挿入モード未実装）、`tBodies` が空になっていた。054 で "in table" / "in table body" 挿入モードと暗黙 `<tbody>` 生成を実装し、`<table><tr><td><p></tbody> </table>` が `table > tbody > tr > td > p`（+ 末尾空白テキスト）になったことで解放。DOM 側の section/cell API は cloneNode 経由でも正しく動くことは本 issue で確認済み。
- test 52: `document.write` で生成される parsed form/input に依存（016-7 は実装済みだが、実測では PASS。念のため経過観察）。
