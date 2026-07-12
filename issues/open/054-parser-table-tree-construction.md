---
number: 054
slug: parser-table-tree-construction
status: open
---

# HTML パーサの table tree construction（暗黙 tbody 生成）

## 概要

HTML パーサ（`src/html/tree_builder.rs`）に HTML 仕様の "in table" / "in table body" 挿入モード相当の
振る舞いを実装し、`<table><tr>` のような マークアップで `<tr>` を暗黙生成した `<tbody>` の下に配置する。

## 背景

- 016-13 の残件調査（PR #116）で、Acid3 test 29 の失敗原因が DOM API ではなく
  パーサが `<tr>` を table 直下に配置していること（`tBodies` が空になる）と特定された
- Acid3 test 5（TreeWalker の空白テキスト検証）も 016-11 実測時に「暗黙 tbody 未生成」が
  原因と記録されている
- DOM 側の HTMLTableSectionElement / rows / cells API は 016-13 で実装済みで、
  cloneNode 経由でも正しく動くことがテストで確認済み。残るはパーサ側のみ

## スコープ

- `<table>` 直下に現れた `<tr>` の前に `<tbody>` を暗黙生成して配置する（HTML 仕様の
  "in table" 挿入モードで `<tr>` を見たときの挙動）
- `<thead>`/`<tfoot>`/`<tbody>` 明示時は従来どおりその下へ
- table 内の空白テキストの配置は仕様の挙動に近づける（test 5 が空白 Text ノードの
  位置に依存するため、acid3.html の期待に合わせて検証）
- foster parenting（table 内の不正要素の吐き出し）は本 issue では**対象外**
  （必要になったら別 issue。既存挙動を変えない）

## 受け入れ条件

- `<table><tr><td>` のパース結果が `table > tbody > tr > td` になる ✅
- Acid3 test 29（cloneNode + tBodies）が PASS する ✅
- test 5（TreeWalker）は暗黙 tbody 前提（expectation 11）を通過 ✅（後続は 055 の API 依存で残存）
- 既存テスト（特に paint の Acid2 ベースライン比較と実サイト系）が全て通る ✅

## 実装結果（2026-07-12）

`src/html/tree_builder.rs` に HTML 仕様の "in table" / "in table body" 挿入モードを実装した。

- `InsertionMode::InTableBody` を追加し、`handle_in_table_body` を新設。
- **"in table" モード**:
  - `<tbody>`/`<thead>`/`<tfoot>`（明示 section）→ table 直下に挿入し InTableBody へ。
  - `<tr>` / `<td>` / `<th>` → 暗黙 `<tbody>` を生成して InTableBody へ切り替え、同トークンを再処理
    （`<td>`/`<th>` は続けて "in table body" で暗黙 `<tr>` も生成）。
- **"in table body" モード**: `<tr>` を section 直下に配置。`<td>`/`<th>` は暗黙 `<tr>` を生成。
  section/`</table>` 系タグで section を閉じて "in table" へ戻す。
- **"clear the stack back to a table (body) context"** に相当するスタック巻き戻しヘルパを追加。
- **section 閉じ後の table 内空白テキスト**: `</tbody>` 等でスタックが table まで巻き戻った後、
  空白テキストは table 直下に置かれる（Acid3 の `<table><tr><td><p></tbody> </table>` で
  `table > [tbody > tr > td > p, #text " "]` となり、test 29 の `t2.childNodes.length===2` /
  `t2.lastChild.data===" "` を満たす）。
- body 文脈に直接現れた `<tr>`/`<td>`/`<th>` も暗黙 `<table>` 経由で同じ経路に統一。
- **foster parenting は対象外**（既存挙動を維持）。

### 対応範囲

- `<table><tr>` に加えて `<table><td>`/`<table><th>`（tbody + tr の二重暗黙生成）にも対応。
- 明示 `<thead>`/`<tbody>`/`<tfoot>` は二重ラップせず従来どおり。
- caption / col / colgroup は parser では未対応（Acid3 は DOM API 経由で生成するため不要）。

### Acid3 実測（FAITHFUL / DIRECT 両モード）

- 87/100 → **88/100**（両モード）。
- **test 29**: FAIL → **PASS**（cloneNode + tBodies）。
- **test 5**: 変化あり（`expectation 11 failed` → 後続の `document.links[1].firstChild` で throw）。
  暗黙 tbody 前提は解消。残るのは `document.links` 未実装（055）。
- **test 4**: 変化なし（`document.forms[0]` で throw、tbody 参照より前で失敗するため）。055 依存。

### 既存テストの更新（仕様準拠側へ）

- `table_modes_create_rows_and_cells`: `table.child_nodes()[0]` が `tr` → `tbody` に変わったため、
  tbody 経由で tr/td を辿るよう更新。
- `nested_table_close_keeps_following_rows_in_outer_table`: 外側 table の行が暗黙 tbody 配下に
  入るため、`table > tbody > tr` を辿るよう更新。

## 関連

- 016 Acid3 対応（test 29 解放。test 4/5 は 055 の DOM API 待ち）
- 016-13 HTMLTableElement API（DOM 側は実装済み、パーサ側を本 issue で実装完了）
- 055 `document.forms` / `document.links`（test 4/5 の残ブロッカー、別スコープ）
