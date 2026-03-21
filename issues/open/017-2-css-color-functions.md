---
number: 017-2
slug: css-color-functions
parent: 017-css-feature-gap
status: open
---

# rgba() / hsl() / hsla() / 8桁hex 色関数対応

## 概要

透明度や色相指定を含む色関数を実装する。現状は rgb() と named color, 3/6桁hex のみ。

## 対象

- `rgba(r, g, b, a)` — アルファチャンネル付き RGB
- `hsl(h, s%, l%)` — 色相・彩度・明度
- `hsla(h, s%, l%, a)` — アルファ付き HSL
- `#RRGGBBAA` / `#RGBA` — 8桁/4桁 hex カラー
- `rgb()` の現代構文: `rgb(r g b / a)`
- Named color の拡充（CSS Level 4 の 140+ 色）

## 実装方針

- `parse_color` 関数を拡張
- HSL → RGB 変換アルゴリズムを実装
- alpha 値を Color 構造体で保持し、paint 時にブレンド

## 受け入れ条件

- rgba/hsl/hsla が正しくパース・描画される
- 8桁hex カラーが対応
- named color が CSS Level 4 準拠
