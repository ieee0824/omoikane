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

1. ~~**カラム幅分配**: col0 の intrinsic width (350px) が explicit として扱われるが、
   col2 の intrinsic width (1143px) が比例縮小で col0 を圧縮している~~ → 021-1 で修正済み
2. **行高計算**: rowspan=2 のセルの高さが2行に分配されない（1パスレイアウトの限界）
3. **セルの垂直配置**: 行高変更後の vertical-align 調整が未実装

### 調査結果

- CSS 2.1 §17.5.3: rowspan セルの高さ分配は **実装依存**
- 仕様上 **2パスレイアウトが必須**（rowspan 制約はデータ依存）
- Chromium: 均等分配、Gecko: 最後の行に集約、WebKit: 比例分配
- **Chromium 方式（均等分配）** を採用

## 子 issue

### 021-1: カラム幅の改善 ✅ 完了 (PR #56)
- テーブル shrink-to-fit 後の auto margin 再計算
- img width 属性によるカラム幅設定
- align="center" のセンタリング対応

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
