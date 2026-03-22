---
number: 017-15
slug: gradient-tile-performance
parent: 017-css-feature-gap
status: open
---

# グラデーションタイル繰り返しのパフォーマンス改善

## 概要

background-size + background-repeat でグラデーションをタイル描画する際、
ピクセル毎にグラデーション計算を繰り返しているため、小タイル×大面積で非常に重い。

## 対応方針

- 1タイル分をオフスクリーンバッファにレンダリング
- 繰り返しはバッファのblit/スケールで描画（画像タイルと同じ方式）

## 出典

- PR #35 Copilot レビュー指摘4
