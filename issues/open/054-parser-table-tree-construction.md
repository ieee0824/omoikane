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

- `<table><tr><td>` のパース結果が `table > tbody > tr > td` になる
- Acid3 test 29（cloneNode + tBodies）と test 5（TreeWalker）が PASS する
- 既存テスト（特に paint の Acid2 ベースライン比較と実サイト系）が全て通る

## 関連

- 016 Acid3 対応（test 5, 29。test 4 への波及も実測で確認）
- 016-13 HTMLTableElement API（DOM 側は実装済み、残項目としてパーサ側を本 issue に切り出し）
