---
number: 034-6
slug: border-spacing-two-value
parent: 034-css-spec-compliance-fixes
status: open
---

# border-spacing の2値対応

## 概要

`border-spacing: <horizontal> <vertical>` の2値指定が `compute_value` で先頭値のみに潰される。
継承リストに `border-spacing` を追加したことで、誤った値が子要素に継承される可能性がある。

## 修正方針

- `compute_value` で `border-spacing` の `Value::List` を `render_value` 等で保持
- または `border-spacing-x` / `border-spacing-y` に分解して個別に継承
