---
number: 021-3
slug: cell-content-placement
parent: 021-table-rowspan-multicolumn-layout
status: open
---

# テーブルセル内容の配置改善

## 概要

rowspan セルの内容が全スパン行にわたって正しく配置されるようにする。
他セルの垂直配置（vertical-align）が正しく機能することを確認。

## 対応内容

- rowspan セルの content box が全スパン行の高さを使う
- vertical-align: top/middle/bottom がスパン全体に対して動作
- セル内のブロックレイアウトがスパン高さを containing block として使う

## 受け入れ条件

- rowspan=2 のセル内容が2行分の高さで正しく配置される
- 同行の他セルの垂直配置が正しい
