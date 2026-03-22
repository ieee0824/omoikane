---
number: 017
slug: css-feature-gap
parent:
status: open
---

# CSS 未実装機能の段階的補完

## 概要

実サイトレンダリングと Acid3 対応に向けて、未実装の CSS 機能を優先度順に実装する。
現状の対応プロパティ数は約60で、モダンサイトの基本描画に必要な機能の多くが不足している。

## 子issue

### 高優先度（実サイト頻出）
- [017-1](../closed/017-1-css-units-rem-viewport.md): rem / vw / vh 等の単位対応 ✅ 完了
- [017-2](../closed/017-2-css-color-functions.md): rgba() / hsl() / hsla() / 8桁hex 色関数 ✅ 完了
- [017-3](017-3-border-radius.md): border-radius（角丸）🔄 PR#32
- [017-4](017-4-box-shadow-opacity.md): box-shadow / opacity
- [017-5](../closed/017-5-text-decoration-transform.md): text-decoration / text-transform / letter-spacing ✅ 完了
- [017-6](../closed/017-6-shorthand-expansion.md): margin / padding 等の shorthand 完全展開 ✅ 完了
- [017-7](../closed/017-7-advanced-selectors.md): :not() / :is() / 属性セレクタ ^= $= *= ✅ 完了

### 中優先度
- [017-8](017-8-list-style.md): list-style-type / list-style-position
- [017-9](017-9-gradient-background-size.md): linear-gradient() / background-size
- [017-10](017-10-media-query-evaluation.md): @media 条件評価

### レビュー派生
- [017-11](017-11-text-decoration-test-isolation.md): text-decoration underline テストの分離検証
- [017-12](017-12-per-fragment-inline-styling.md): ネスト inline 要素の per-fragment スタイリング

## 進め方

- 各子issueは独立して着手可能
- 実サイトの unsupported CSS ログ（SQLite）の出現頻度を参考に優先順位を調整
- 各issueごとにテストを先行して書き、仕様準拠を確認する
