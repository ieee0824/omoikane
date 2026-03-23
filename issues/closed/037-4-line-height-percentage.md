---
number: 037-4
slug: line-height-percentage
parent: 037-rendering-precision-improvements
status: open
---

# line-height パーセンテージ/unitless 対応

## 問題

`line-height: 150%` や `line-height: 1.5` が font_size * 1.2 にフォールバック。

## 修正

`line_height()` で:
- `ComputedValue::Percentage(p)` → `font_size * p / 100.0`
- `ComputedValue::Number(n)` → `font_size * n`（既に対応済みの可能性、要確認）
