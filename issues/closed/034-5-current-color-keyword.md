---
number: 034-5
slug: current-color-keyword
parent: 034-css-spec-compliance-fixes
status: open
---

# currentColor キーワード対応

## 概要

CSS Color Module Level 4 の `currentColor` キーワードを実装する。

## 仕様

- `currentColor` は要素の computed `color` プロパティの値を参照する
- `border-color` の初期値は `currentColor`
- `text-decoration-color` の初期値は `currentColor`
- `outline-color` の初期値は `currentColor`

## 修正方針

1. `compute_value` で `currentColor` キーワードを `ComputedValue::Color("currentColor")` として保持
2. paint 時に `currentColor` を解決（要素の computed color を参照）
3. `border-color` 未指定時のデフォルトを `currentColor` に設定

## 修正箇所

- `src/css/style.rs` — currentColor のキーワード認識
- `src/paint/color.rs` — currentColor の解決
- `src/paint/border.rs` — border-color のデフォルト値
