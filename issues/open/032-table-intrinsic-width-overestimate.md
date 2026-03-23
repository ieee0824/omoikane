---
number: 032
slug: table-intrinsic-width-overestimate
status: open
---

# テーブル intrinsic width の過大評価

## 概要

explicit width のないテーブルの shrink-to-fit 幅が、実コンテンツ幅よりもはるかに大きくなる。auto 幅のカラムが containing block の残り幅をほぼ使い切ってしまい、テーブルが中央揃え（align=center）で狭くまとまるべきところが全幅に広がる。

## 再現条件

```html
<table align="center">
  <tr>
    <td rowspan="2"><img width="350" height="414">（固定幅コンテンツ）</td>
    <td>&nbsp;</td>
    <td>テキストコンテンツ</td>
  </tr>
  <tr>
    <td></td>
    <td>テキストコンテンツ</td>
  </tr>
</table>
```

- 期待: テーブル幅 ≈ 800px（コンテンツに合わせて shrink-to-fit し中央揃え）
- 実際: テーブル幅 ≈ 1143px（containing block 幅に近い）

## 原因分析

- `compute_table_column_widths` で auto カラムの幅計算時、remaining = available_width - fixed_total
- auto カラムの intrinsic hint が小さい場合、余った幅が等分で追加される
- 結果として auto カラムが不必要に広がる

## 修正方針

- auto カラムには intrinsic hint を超える幅を配分しない（shrink-to-fit の原則）
- テーブル全体の幅 = sum(column_widths) + spacing（containing block 幅ではなく）
- auto margin のセンタリングは shrink-to-fit 後の幅に基づいて再計算
