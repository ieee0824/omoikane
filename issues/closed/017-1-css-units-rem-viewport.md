---
number: 017-1
slug: css-units-rem-viewport
parent: 017-css-feature-gap
status: open
---

# rem / vw / vh 等の CSS 単位対応

## 概要

モダンサイトで頻出する相対単位を実装する。

## 対象単位

- `rem` — ルート要素の font-size 基準（最優先、ほぼ全サイトで使用）
- `vw` — viewport 幅の 1%
- `vh` — viewport 高さの 1%
- `vmin` — vw/vh の小さい方
- `vmax` — vw/vh の大きい方
- `ch` — フォントの "0" 文字幅
- `ex` — フォントの x-height

## 実装方針

- `compute_value` で各単位を px に変換
- `rem` は root element の computed font-size を参照（StyleResolver に root font-size を保持）
- viewport 単位は layout_tree の viewport rect から取得（thread-local or 引数伝播）

## 受け入れ条件

- `rem`, `vw`, `vh` が px に正しく変換される
- 各単位のユニットテストが通過
