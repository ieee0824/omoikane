---
number: 015
slug: anonymized-real-world-rendering-gap
parent: 013-real-world-rendering
status: open
---

# 実サイトAでの描画差分解消（サイト名非公開）

## 概要

匿名化した実在サイトAをレンダリングしたところ、HTML/CSSの基本処理は動作しているものの、
実ブラウザと比較して視覚差分が大きく、スクリーンショット用途としては不十分な状態。

本issueでは、サイト固有情報を含めずに再現できる「レンダリング品質ギャップ」を整理し、
差分縮小のための優先実装項目を定義する。

## 現状の症状（サイト名・URLは記載しない）

1. 動的DOM前提のレイアウトが崩れる（初期HTMLのみ描画される）
2. 一部CSSが未対応で、位置・重なり順・視覚効果が実ブラウザと異なる
3. フォントメトリクス差により、改行位置と行高がずれる
4. 固定viewport依存で、実ページ想定の見え方とずれるケースがある

## 対応方針

1. 未対応CSSプロパティの観測ログを追加し、実サイトAでの使用頻度を可視化
2. 影響の大きいプロパティから順に実装（例: `position` / `transform` / `filter` 周辺）
3. スクリーンショットAPIのviewport指定を外部から制御可能にする
4. 実サイトAを匿名化fixtureとして継続比較できる仕組みを用意する

## 子issue

- [015-1 未対応CSSプロパティ観測ログ](../closed/015-1-anonymized-css-coverage-logging.md)
- [015-2 主要CSS不足の優先実装](../closed/015-2-anonymized-priority-css-implementation.md)
- [015-3 screenshot API の viewport 指定](../closed/015-3-ffi-screenshot-viewport-control.md)
- [015-4 匿名化fixtureと差分比較基盤](../closed/015-4-anonymized-fixture-diff-baseline.md)
- [015-5 FFI境界の責務整理](../closed/015-5-ffi-boundary-refactor.md)
- [015-6 CJK文字の描画品質ギャップ](./015-6-anonymized-cjk-font-rendering-gap.md)

## 受け入れ条件

- 匿名化された再現ページで、主要セクションのレイアウト崩れが解消される
- 現状baseline比で明確な差分削減が確認できる（目視 + ピクセル差分）
- issue本文・コミット・PR説明にサイト固有情報（URL/名称）が含まれない

## 備考

- 実サイト名は社内メモ/口頭共有のみとし、リポジトリには記録しない
- 必要に応じて子issueへ分割して進める
