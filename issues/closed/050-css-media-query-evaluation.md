---
number: 050
slug: css-media-query-evaluation
parent: 016
status: closed
---

# CSS media query の構文解析・評価と computed style 反映

## 概要

`@media` の構文解析と評価を実装し、iframe ごとの viewport を使ったカスケードと
`text-transform` の computed style 露出を Acid3 test 46 相当まで整備する。

## 背景

issue 016-10 の selector 拡充調査で、Acid3 test 46 は selector matcher ではなく
media query、iframe viewport、computed style の複合ギャップであることを確認した。
既存の issue 030 はベンダープレフィックスの標準プロパティへのマッピング、
issue 047 は inline style のカスケード統合が対象であり、本 issue の media 条件評価とは重複しない。

## 対応内容

- `@media all` / `not` / `only` と comma-separated query の OR 評価
- 未知の media feature を false として扱う構文・評価
- `color` / `monochrome` feature
- `min/max-width`、`min/max-height` と `em` 単位
- main document と iframe contentDocument それぞれの viewport に基づく評価
- viewport 変更時の style resolver invalidation
- `text-transform` の初期値 `none` と computed style serialization

## 関連 issue

- 016 Acid3 対応（test 46）
- 030 ベンダープレフィックスの標準プロパティへのマッピング（非重複）
- 031 cursor / transform-origin の基本対応（非重複）
- 047 inline style カスケード（カスケード経路の一元化で関連）

## 受け入れ条件

- Acid3 test 46 の media query assertion が全て通る
- iframe の viewport サイズ変更後に media 条件と computed style が再評価される
- media query の正常系・否定・未知 feature・リスト OR の単体テストが通る
- 既存テストが全て通る
