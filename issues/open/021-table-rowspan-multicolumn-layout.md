---
number: 021
slug: table-rowspan-multicolumn-layout
parent:
status: open
---

# テーブル rowspan 複数列レイアウトの本格改善

## 概要

rowspan を含むテーブルで、複数列のセルが正しく横並びにならない問題を本格的に修正する。
実サイトのテーブルレイアウト（frameset + rowspan 複数列）で顕著に発生しており、
Firefox のレンダリングと大きな差分がある。

## 現状の問題

Firefox 参考: 写真(col0) | スペーサー(col1) | ★★★最新情報+ドラマ情報(col2) が横並び
当方: 写真の下にドラマ情報が縦積み、右カラムのコンテンツがほぼ見えない

### HTML 構造
```html
<table align="center">
  <tr>
    <td rowspan="2">
      <img width="350" height="414">
      <table width="256">プロフィール</table>
      所属情報テキスト...
    </td>
    <td>&nbsp;</td>
    <td>★★★ 最新情報 ★★★</td>
  </tr>
  <tr>
    <td></td>
    <td>ドラマ・映画情報...</td>
  </tr>
</table>
```

### 原因分析

1. **カラム幅分配**: col0 の intrinsic width (350px) が explicit として扱われるが、
   col2 の intrinsic width (1143px) が比例縮小で col0 を圧縮している
2. **行高計算**: rowspan=2 のセルの高さが row0 の全高になり、
   row0 の他セル（col1, col2）も同じ高さに引き伸ばされる
3. **セルの垂直配置**: rowspan セルの内容（写真 + ネストテーブル + テキスト）が
   1行分の高さに制限されている可能性

## 子 issue

### 021-1: カラム幅の改善
- テキストの intrinsic width を「折り返し可能な最小幅」と「折り返し不可の最大幅」に分離
- 画像含有セルの幅は min-content width（折り返し不可）で固定
- テキストセルは available width に応じて折り返し
- CSS 2.1 §17.5.2 の自動テーブルレイアウトアルゴリズムに近づける

### 021-2: rowspan の行高分配
- rowspan=N のセルの高さを N 行に分配
- 各行の高さはセル内容の intrinsic height で決まる
- rowspan セルの高さが sum(行高) より大きい場合は追加分を分配

### 021-3: セル内容の配置
- rowspan セルの内容が全スパン行にわたって正しく配置される
- 他セルの垂直配置（vertical-align）が正しく機能する

## 検証対象

- Firefox 参考スクリーンショットとの比較
- rowspan + 複数列のテーブルを使った実サイト
- Acid2 テストの回帰なし

## 受け入れ条件

- rowspan テーブルで写真の右横にテキスト情報が横並びで表示される
- Firefox 参考と目視で同等のレイアウト
- 既存テスト全通過
