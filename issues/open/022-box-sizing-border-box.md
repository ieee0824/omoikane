---
number: 022
slug: box-sizing-border-box
parent:
status: open
---

# box-sizing: border-box 対応

## 概要

`box-sizing: border-box` を実装する。モダン Web サイトのほぼ全要素が
`* { box-sizing: border-box }` を前提としており、未対応だとレイアウト全体が崩壊する。

## 背景

実サイトの未対応 CSS ログで `box-sizing` が **939 回** で最頻出。
`border-box` が適用されないと、padding/border を含む要素の width/height が
意図より大きくなり、カラム落ち・はみ出し・レイアウト崩壊が発生する。

## CSS 仕様

- `box-sizing: content-box` (デフォルト): width/height は content 領域のみ
- `box-sizing: border-box`: width/height は padding + border を含む
  - `content_width = specified_width - padding_left - padding_right - border_left - border_right`
  - `content_height = specified_height - padding_top - padding_bottom - border_top - border_bottom`

## 対応内容

### src/css/style.rs
- `is_supported_property` に `box-sizing` を追加

### src/layout/mod.rs
- `compute_width` で `box-sizing: border-box` の場合、specified width から
  padding + border を差し引いて content width を算出
- height 計算でも同様に対応
- min-width / max-width / min-height / max-height も border-box を考慮

### src/css/shorthand.rs
- `box-sizing` は shorthand ではないのでそのまま

## 受け入れ条件

- `box-sizing: border-box` の要素で width が padding + border を含むサイズとして扱われる
- `* { box-sizing: border-box }` を使った実サイトでレイアウトが改善される
- 既存テスト全通過（content-box の動作に回帰なし）
