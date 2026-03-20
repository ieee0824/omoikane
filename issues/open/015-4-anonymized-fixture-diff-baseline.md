---
number: 015-4
slug: anonymized-fixture-diff-baseline
parent: 015-anonymized-real-world-rendering-gap
status: open
---

# 匿名化fixtureと差分比較基盤

## 概要

サイト固有情報を含まない形で継続的に描画品質を追跡するため、
匿名化fixtureと画像差分比較の運用を整備する。

## スコープ

- 匿名化HTML/CSS/画像fixtureの配置ルール定義
- baseline画像の生成と更新手順を文書化
- 差分画像の出力先/命名規則を統一
- CIで実行可能な比較テスト追加（必要なら `ignored` で段階導入）

## 受け入れ条件

- ローカルで baseline / actual / diff を再現できる
- リポジトリ内にサイト名/URL等の固有情報を含めない
- 差分改善が時系列で追える状態になる
