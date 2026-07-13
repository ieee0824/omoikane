---
number: 061
slug: css-grid-layout
status: open
---

# CSS Grid レイアウトの実装（親 issue）

## 概要

CSS Grid レイアウト（`display: grid` / `inline-grid`）を実装する。現状レイアウトエンジンは
flex / inline / table のみで grid モジュールが無く、grid コンテナが block にフォールバックして
子が縦積み full-width になる。多くのモダンサイトが崩れる主因。

## 背景（実サイトログ由来: kasaneteto.jp）

kasaneteto.jp の app.css は grid を多用: `display: grid` 131箇所、`grid-template-columns` 37、
`grid-column` 109、`grid-row` 94、`place-content` 55、`display: flex` 59。grid 未対応のため
ナビ・ヒーローの重ね/分割レイアウトが全崩壊する。

## 段階分割（子 issue）

大きいため段階実装する。各段は単体で意味のある前進とし、順に PR 化する。

- [x] 061-1 明示的グリッド定義とトラックサイジング（PR #129）
      `display: grid`、`grid-template-columns/rows`（px / %  / fr / auto / repeat()）、
      基本の行優先フロー配置、`gap` / `row-gap` / `column-gap`
- [ ] 061-2 明示的アイテム配置とスパン
      `grid-column` / `grid-row`（line ベース `a / b`、`span n`）、
      明示配置と自動配置の混在、暗黙トラック生成（`grid-auto-rows/columns` の最小対応）
- [ ] 061-3 アラインメント
      `justify-items` / `align-items` / `justify-content` / `align-content` /
      `place-items` / `place-content` / `justify-self` / `align-self`
- [ ] 061-4（必要なら）minmax() / auto-fill / auto-fit / 名前付きライン等の拡張

## 方針

- 既存 `src/layout/flex.rs` と並ぶ `src/layout/grid.rs` を新設し、`src/layout/mod.rs` の
  ディスパッチに `Display::Grid` を追加
- CSS パーサに grid 系プロパティを追加（`is_supported_property` 登録、値のパース）
- レイアウトの座標系・BoxDimensions・positioned 子の扱いは既存 flex 実装のパターンに合わせる
- レンダリング（paint）は既存のボックス描画をそのまま使えるようにトラック→ボックス確定まで
  layout 側で完結させる

## 受け入れ条件（親）

- 子 issue 061-1〜061-3 がすべて実装・マージされる
- kasaneteto.jp のナビ/ヒーローが縦積み崩壊せず、意図に近い分割・重ねレイアウトで描画される
- 既存テスト・Acid3 スコアの維持

## 関連

- 060 CJK フォントフォールバック（同サイトの可読性）
- 016 レイアウトエンジン群 / 044 実サイト品質
