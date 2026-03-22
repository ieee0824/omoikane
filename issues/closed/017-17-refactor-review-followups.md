---
number: 017-17
slug: refactor-review-followups
parent: 017-css-feature-gap
status: open
---

# PR#37 リファクタリングレビュー派生の修正

## 対象

### 1. placeholder に letter-spacing 未反映 (P2)
- `paint_text_placeholder` が letter-spacing を考慮していない
- layout 幅とのずれが発生する
- 出典: PR #37 Copilot 指摘2

### 2. border-top shorthand が全辺に影響 (P2)
- `expand_border_side_shorthand` が `border-style`/`border-color` をグローバルに emit
- `border-top: 1px solid red` が他の辺にも影響する
- 出典: PR #37 Copilot 指摘4

### 3. Text ノードに text-transform 未適用 (P2)
- `collect_inline_segments` の `NodeType::Text` 分岐で `apply_text_transform_layout` が呼ばれない
- Element 経由のテキストのみ変換され、直接の Text ノードは変換されない
- 出典: PR #37 Copilot 指摘5
