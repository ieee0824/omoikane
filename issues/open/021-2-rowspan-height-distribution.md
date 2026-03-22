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

## 対応内容

- rowspan=N のセルの高さを N 行にまたがって分配
- 各行の高さは非 rowspan セルの intrinsic height で初期決定
- rowspan セルの高さが sum(行高) より大きい場合は差分を均等分配
- rowspan セルの内容はスパン全体にわたって配置

## 受け入れ条件

- rowspan=2 のセルと同じ行の他セルが正しい高さで配置される
- 2行目のセルが1行目の直下に正しく配置される
