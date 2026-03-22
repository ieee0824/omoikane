---
number: 017-10
slug: media-query-evaluation
parent: 017-css-feature-gap
status: open
---

# @media 条件評価

## 概要

@media ルールの条件を評価し、viewport サイズに応じたスタイル適用を実現する。
現状は @media ブロック内のルールを無条件に適用している。

## 対象

- `@media screen { ... }` — メディアタイプ
- `@media (max-width: 768px) { ... }` — viewport 幅条件
- `@media (min-width: 1024px) { ... }` — viewport 幅条件
- `@media (orientation: portrait) { ... }` — 向き
- `@media (prefers-color-scheme: dark) { ... }` — カラースキーム
- `and` / `or` / `not` 条件結合

## 実装方針

- CSSパーサーで @media の条件式を AST として保持
- StyleResolver にviewport 情報を渡し、条件評価
- レスポンシブデザインの基本的なブレークポイントをサポート

## 受け入れ条件

- `@media (max-width: 768px)` が viewport 幅に応じて適用/非適用
- `@media screen` がスクリーンメディアとしてマッチ
- 条件不一致のルールが適用されない
