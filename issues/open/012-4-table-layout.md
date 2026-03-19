---
number: 012-4
slug: table-layout
parent: 012-acid2-official-conformance
status: open
---

# `display: table` / `table-cell` レイアウト

## 概要

CSS の `display: table` / `display: table-cell` 等のテーブル関連 display 値によるレイアウトを実装する。

## 仕様参照

- CSS 2.1 §17 Tables
- CSS 2.1 §17.2.1 Anonymous table objects

## スコープ

- `display: table` の要素をテーブルとしてレイアウトする
- `display: table-row` / `display: table-cell` による行・セルの配置
- anonymous table object の生成（table-cell が table-row の外にある場合等）
- セルの幅・高さの計算（固定テーブルレイアウト）
- 行内のセル高さの揃え（最大セル高さに合わせる）
- `border-spacing` プロパティ

## スコープ外

- `display: table-caption` / `table-header-group` / `table-footer-group`
- 自動テーブルレイアウトアルゴリズム（複雑な幅分配）
- `border-collapse: collapse` モデル
- `vertical-align` によるセル内容の垂直配置（初期値 baseline のみ）
