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

## 実装済み

- `list-style` shorthand → `list-style-type` / `list-style-position` / `list-style-image` 展開
- `is_supported_property` に追加済み
- `apply_inheritance` に `list-style-type`, `list-style-position` 追加済み
- UA defaults: `ul` → disc/outside, `ol` → decimal/outside, `li` → display:list-item
- `LayoutBox::marker: Option<ListMarker>` 追加
- `display: list-item` でマーカーボックス生成（disc/circle/square/decimal/lower-roman/upper-roman/lower-alpha/upper-alpha）
- 描画: `paint_list_marker` (フォントあり/なし両対応)
- `outside` マーカー: content box の左外側に配置（1em オフセット）

## 既知の制限・残課題

### list-style-position: inside の不完全な実装 (P1)
- 現状: `inside` マーカーは content box の原点に配置されるが、後続のインラインコンテンツとのスペースを確保していない
- 理想: マーカーがインラインラインボックスに参加し、テキストを右にシフトする
- 影響: inside ポジションのリスト項目でマーカーが最初のテキストに重なる可能性
- 修正には layout エンジンへの変更が必要（別 issue で対応予定）
