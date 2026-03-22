---
number: 017-14
slug: list-marker-fontless-fallback
parent: 017-css-feature-gap
status: open
---

# リストマーカーのフォントなしフォールバック改善

## 概要

フォントが読み込めない環境で、全マーカー種別が四角形にフォールバックする問題を修正。

## 背景

PR #34 の Copilot レビューで指摘。`paint_list_marker_placeholder` が全マーカー種別で
塗りつぶし四角形を描画するため、decimal (1. 2. 3.) や roman (i. ii.) が四角形になる。

## 対応方針

- bullet マーカー (disc/circle/square): 現状の四角形フォールバックを維持
- テキストマーカー (decimal/roman/alpha): `paint_text_placeholder` で `marker.text` を描画

## 出典

- PR #34 Copilot レビュー指摘3
