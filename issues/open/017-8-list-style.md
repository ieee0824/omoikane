---
number: 017-8
slug: list-style
parent: 017-css-feature-gap
status: open
---

# list-style-type / list-style-position 対応

## 概要

リスト要素のマーカー表示を実装する。

## 対象プロパティ

- `list-style-type` — disc, circle, square, decimal, none 等
- `list-style-position` — inside, outside
- `list-style` (shorthand)
- `display: list-item`

## 実装方針

- `display: list-item` でマーカーボックスを生成
- `list-style-type` に応じて `::marker` 疑似要素相当のコンテンツを生成
- outside の場合はマーカーを content box の左外側に配置

## 受け入れ条件

- ul/ol のデフォルトマーカーが表示される
- list-style-type: none でマーカーが消える
