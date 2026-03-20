---
number: 012-3
slug: overflow-hidden-clipping
parent: 012-acid2-official-conformance
status: closed
---

# `overflow: hidden` によるクリッピング

## 概要

`overflow: hidden` が指定された要素の content + padding 領域外の描画をクリップする。

## 仕様参照

- CSS 2.1 §11.1.1 Overflow: the 'overflow' property

## スコープ

- `overflow: hidden` の computed value を正しく取得する
- paint 時に overflow: hidden を持つボックスの padding edge で clipping rect を設定する
- 子孫要素の描画がクリッピング領域外にはみ出す場合に切り取る
- ネストした overflow: hidden の clipping rect の交差

## スコープ外

- `overflow: scroll` / `overflow: auto` によるスクロールバー生成
- `overflow-x` / `overflow-y` の個別制御

## 完了メモ

- paint 側で overflow hidden の clip 適用は実装済み
- `paint::tests::clips_children_when_overflow_is_hidden` で基本ケースを固定済み
- Acid2 fixture / reference ともに viewport 側で `overflow: hidden` を使うため、Acid2 公式比較の前提機能として利用中
