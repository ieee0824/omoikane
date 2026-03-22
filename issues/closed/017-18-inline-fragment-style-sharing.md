---
number: 017-18
slug: inline-fragment-style-sharing
parent: 017-css-feature-gap
status: open
---

# InlineFragment の ComputedStyle 共有化

## 概要

InlineFragment に ComputedStyle (BTreeMap) を Clone して保持しているため、
テキストが多数の fragment に分割される場合に CPU/メモリコストが大きい。

## 対応方針

- `Arc<ComputedStyle>` で共有参照にする
- または必要なプロパティのみ（color, text-transform, text-decoration-*）を小さな構造体で保持

## 出典

- PR #42 Copilot レビュー指摘3
