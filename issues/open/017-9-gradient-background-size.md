---
number: 017-9
slug: gradient-background-size
parent: 017-css-feature-gap
status: open
---

# linear-gradient() / background-size 対応

## 概要

グラデーション背景と背景サイズ制御を実装する。

## 対象

- `linear-gradient(direction, color-stop, ...)` — 線形グラデーション
- `radial-gradient()` — 放射グラデーション（中優先）
- `background-size` — contain, cover, length/percentage

## 実装方針

### linear-gradient
- パース: 方向（角度 or to top/right 等）+ カラーストップ列
- 描画: ピクセル毎にグラデーション軸上の位置を算出し色を補間

### background-size
- `cover`: 画像がボックス全体を覆う最小拡大
- `contain`: 画像がボックスに収まる最大拡大
- `length`/`%`: 明示的サイズ

## 受け入れ条件

- `linear-gradient(to right, red, blue)` が正しく描画される
- `background-size: cover` で画像がボックスを覆う
