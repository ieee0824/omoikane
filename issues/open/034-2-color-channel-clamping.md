---
number: 034-2
slug: color-channel-clamping
parent: 034-css-spec-compliance-fixes
status: open
---

# RGB/HSLチャネル値のクランプ修正

## 概要

CSS Color Level 4 に準拠し、範囲外のチャネル値をクランプする。

## 問題

### RGB
- `rgb(300, 0, 0)` → 現在: u8 キャストでラップ (300→44) → 期待: クランプ (300→255)
- `rgb(200%, 0%, 0%)` → 現在: 510→u8ラップ → 期待: 255にクランプ
- 負の値 `rgb(-50, 0, 0)` → 期待: 0にクランプ

### HSL
- `paint/color.rs` と `css/style.rs` に2つのHSLパーサーが存在
- 片方でのみ hue wrap が実装されている

## 修正方針

1. `extract_channel()` で 0.0〜255.0 にクランプしてから u8 キャスト
2. HSL パーサーを1箇所に統合（paint/color.rs に共通関数を配置）
3. 両方の呼び出し元から共通関数を使用

## 修正箇所

- `src/css/style.rs` — `compute_rgb_function`, `compute_hsl_function`
- `src/paint/color.rs` — `parse_rgb_args`, `parse_hsl_args`
