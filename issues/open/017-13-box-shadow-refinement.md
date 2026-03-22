---
number: 017-13
slug: box-shadow-refinement
parent: 017-css-feature-gap
status: open
---

# box-shadow / opacity の描画品質改善

## 概要

PR #33 の Copilot レビューで指摘された box-shadow / opacity の描画品質に関する4件の改善。

## 対象

### 1. box_blur_alpha のカーネルサイズ計算修正
- 端の処理と 2r+1 カーネルの不一致を修正
- 端では実効 kernel_size を動的に変えるか、clamp して常に 2r+1 を使う
- 出典: PR #33 Copilot 指摘1

### 2. opacity のオフスクリーンバッファを要素サイズに限定
- 現状はキャンバス全体サイズを確保しており O(N * W * H) になりやすい
- 要素の border_box + 影/フィルタ余白に合わせた最小バッファに切り出す
- 出典: PR #33 Copilot 指摘4

### 3. overflow: hidden での box-shadow クリップ
- 現状は inherited_clip のみ渡しており、要素自身の overflow clip が適用されない
- clip 計算後に box-shadow を描画するか、先に overflow clip を計算して渡す
- 出典: PR #33 Copilot 指摘5

### 4. blur なしの角丸 box-shadow
- blur=0 のパスで上下左右の矩形バンド塗りつぶしのみで角丸が反映されない
- rounded rect → annulus 描画に揃える
- 出典: PR #33 Copilot 指摘6
