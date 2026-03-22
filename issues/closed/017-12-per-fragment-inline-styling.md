---
number: 017-12
slug: per-fragment-inline-styling
parent: 017-css-feature-gap
status: open
---

# ネストした inline 要素の per-fragment スタイリング

## 概要

PR #30 の Copilot レビューで指摘された、ネストした inline 要素（`<span>` 内の
`text-transform` 等）のスタイルが fragment 単位で適用されない問題を修正する。

## 背景

現在の `paint_text` はボックスレベルの `style` から `text-transform` / `text-decoration` を
1回取得し、全 fragment に同じ値を適用している。しかし inline fragment は異なるノード由来の
異なるスタイルを持つ可能性がある（例: `<p>normal <span style="text-transform:uppercase">upper</span></p>`）。

## 対応方針

- `InlineFragment` に computed style の参照またはスタイル情報を保持
- `paint_text` で fragment ごとにスタイルを取得して適用
- `collect_inline_segments` でノードごとの text-transform / text-decoration を伝播

## 出典

- PR #30 Copilot レビュー指摘3
