---
number: 012-3
slug: overflow-hidden-clipping
parent: 012-acid2-official-conformance
status: open
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
