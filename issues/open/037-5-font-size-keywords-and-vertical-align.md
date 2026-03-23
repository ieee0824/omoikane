---
number: 037-5
slug: font-size-keywords-and-vertical-align
parent: 037-rendering-precision-improvements
status: open
---

# font-size キーワードと vertical-align sub/super の解決

## 概要

UA defaults で `font-size: smaller` / `vertical-align: sub/super` を設定しているが、
layout 側でこれらの値が解決されていない。

## 問題

- `font_size()` は `ComputedValue::Px` のみ処理。`Keyword("smaller")` は未解決で親のサイズにフォールバック
- `vertical_align()` は `sub` / `super` を `Baseline` にフォールバック
- `<sub>`, `<sup>`, `<small>` が通常テキストと同サイズ・同位置で描画される

## 修正方針

- `font-size: smaller` → 親 font-size × 0.833（HTML仕様）
- `font-size: larger` → 親 font-size × 1.2
- `vertical-align: sub` → baseline から下にオフセット（font-size × 0.4）
- `vertical-align: super` → baseline から上にオフセット（font-size × 0.6）
