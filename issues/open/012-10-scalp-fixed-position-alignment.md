---
number: 012-10
slug: scalp-fixed-position-alignment
parent: 012-acid2-official-conformance
status: open
---

# scalp（position:fixed）の顔との位置合わせ

## 概要

`.picture p`（scalp）は `position: fixed; top: 9em; left: 11em` で viewport 基準配置。
比較テストでは通常フロー要素を translation するが、fixed 要素はスキップされるため
scalp が顔の上部と合わない。

## CSS

```css
.picture p { position: fixed; top: 9em; left: 11em; width: 140%; max-width: 4em;
  height: 8px; min-height: 1em; max-height: 2mm; background: black; border-bottom: 0.5em yellow solid; }
```

## スコープ

- `top: 9em` の em が html の font-size (12px) ではなく `.picture p` 自身の font-size で解決されるべき
  - ただし `.picture p` に explicit font-size がないため親から継承 → 12px → top = 108px
- `width: 140%; max-width: 4em` → min(140% of containing block, 48px)
- `min-height: 1em > max-height: 2mm` → min wins → 12px
- 比較テストでの fixed 要素と通常フローの位置合わせ戦略
