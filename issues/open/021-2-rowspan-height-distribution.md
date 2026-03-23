---
number: 021-2
slug: rowspan-height-distribution
parent: 021-table-rowspan-multicolumn-layout
status: open
---

# rowspan の行高分配

## 概要

rowspan=N のセルの高さを N 行に正しく分配する。
現状は rowspan セルの高さが row 0 の全高になり、他セルの配置に影響する。

## 背景調査（CSS 2.1 §17.5.3 + ブラウザ実装）

- 仕様では rowspan セルの高さが span 行合計を超える場合の分配は **実装依存**
- Chromium: **均等分配**（推奨）、Gecko: **最後の行に集約**、WebKit: **比例分配**
- 仕様上 **2パスレイアウトが必須**（rowspan 制約はデータ依存）

## 対応内容

### Phase 1: 2パスレイアウトへの変更
- `layout_table_container` を2パスに変更
- 1パス目: 全行をレイアウトし `row_heights[]` を収集（rowspan=1 セルのみで初期高さ決定）
- 2パス目: rowspan セルの高さ > span 行合計 → 差分を **均等分配**（Chromium 方式）

### Phase 2: rowspan セルの高さ・位置再計算
- 行高さ変更後にセルの content height と y 座標を再計算
- rowspan セルの高さ = span する全行の高さ合計 + spacing

### Phase 3: vertical-align 調整 (021-3 で実施)
- 行高さ拡張後のセルで valign に応じてコンテンツをオフセット

## 受け入れ条件

- rowspan=2 のセルと同じ行の他セルが正しい高さで配置される
- 2行目のセルが1行目の直下に正しく配置される
- rowspan セルの高さが span 行合計より大きい場合、差分が均等分配される
- 阿部寛ページ (top.htm) で写真カラムが2行にまたがり、右カラムの最新情報が横並びになる
