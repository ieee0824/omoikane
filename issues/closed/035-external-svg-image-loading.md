---
number: 035
slug: external-svg-image-loading
status: open
---

# 外部SVGファイルの画像読み込み

## 概要

`<img src="*.svg">` で参照される外部 SVG ファイルをフェッチし、ラスタライズして表示する。

## 背景

- 028 でインライン SVG のレンダリングは対応済み
- しかし `<img src="header-logo.svg">` のような外部 SVG ファイル参照は未対応
- モダンサイトでロゴや装飾に外部 SVG が多用されている

## 修正方針

1. `element_inline_image` の img 処理で、src が `.svg` の場合は SVG としてフェッチ・パース
2. フェッチした SVG テキストを `TreeBuilder::parse` → `render_svg_to_image` でラスタライズ
3. 結果を Image として返す

## 受け入れ条件

- `<img src="logo.svg" width="100" height="40">` が描画される
- data URI SVG (`<img src="data:image/svg+xml,...">`) も対応
- 既存テスト全通過
