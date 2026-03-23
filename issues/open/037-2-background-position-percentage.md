---
number: 037-2
slug: background-position-percentage
parent: 037-rendering-precision-improvements
status: open
---

# background-position パーセンテージ対応

## 問題

`background-position: 50% 50%` が (0, 0) にフォールバックする。
`length_property()` が `ComputedValue::Percentage` を処理しない。

## 修正方針

`background_position()` で Percentage を検出し、要素の padding box サイズと画像サイズから位置を計算:
- `position_x = (box_width - image_width) * percentage / 100`
- `position_y = (box_height - image_height) * percentage / 100`
