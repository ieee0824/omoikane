---
number: 021-1
slug: table-column-width-algorithm
parent: 021-table-rowspan-multicolumn-layout
status: open
---

# テーブルカラム幅の自動レイアウトアルゴリズム改善

## 概要

CSS 2.1 §17.5.2 の自動テーブルレイアウトアルゴリズムに基づき、
各セルの min-content width と max-content width を計算してカラム幅を決定する。

## 現状の問題

- `intrinsic_width` がテキストの最長行幅を返すため、折り返し可能なテキストの
  セルが不当に広い幅を要求する（例: 1143px）
- 画像含有セルとテキストセルを同じ比率で圧縮するため、画像が縮小される

## 対応内容

### min-content width と max-content width の導入
- **min-content width**: 折り返し不可の最小幅（画像幅、単語幅の最大値等）
- **max-content width**: 折り返しなしの理想幅（現在の intrinsic_width 相当）
- `compute_table_column_widths` で min/max を使い分ける

### カラム幅決定ロジック
1. 各カラムの min-content width を最低保証
2. available width が全カラムの min-content 合計以上なら、余剰を max-content の比率で分配
3. available width が不足なら min-content で固定

### min-content width の計算
- 画像: width 属性 or 自然幅
- テキスト: 最長単語の幅（スペースで分割した最大幅）
- ネストテーブル: 再帰的に min-content を計算

## 受け入れ条件

- 実サイトのテーブルで写真カラム(col0)が350px、テキストカラム(col2)が残り幅を使う
- テキストが適切に折り返される
