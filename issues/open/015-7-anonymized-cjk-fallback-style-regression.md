---
number: 015-7
slug: anonymized-cjk-fallback-style-regression
parent: 015-anonymized-real-world-rendering-gap
status: open
---

# CJKフォントフォールバック導入後のスタイル崩れ

## 概要

CJK文字の表示改善により日本語の可読性は向上した一方で、
実サイトA（匿名化対象）では一部レイアウト・見た目の崩れが増加している。

本issueでは「文字化け様表示の改善」と「スタイル整合性」を分離し、
フォント選択ロジック起因の回帰を解消する。

## 現状

- 日本語表示は改善している
- ただし、行高・改行位置・要素の詰まり方に差分が残る
- CJK優先フォールバックの影響で、非CJKテキストや混在行にも副作用が出る可能性がある
- PR #20 で指摘された「layout時の文字計測（単一フォント）と paint時のフォールバック描画」の不一致が残っている

## スコープ

- `font-family`（CSS指定）の優先順位を描画時に反映
- 文字種（CJK/Latin混在）ごとのフォールバック適用条件を見直し
- 行高/折り返しの回帰を検出できる比較fixtureを追加
- 既存比較（Acid2含む）を維持しつつ、匿名化実サイトAで差分縮小を確認
- layout の文字幅計測を paint 側フォールバック戦略と整合させる

## 受け入れ条件

- 日本語可読性を維持したまま、スタイル崩れが現状より縮小する
- `font-family` 指定のある要素で期待フォント順序が尊重される
- 混在テキスト行での不自然な行高/改行増加が抑制される
- 既存レンダリング回帰テスト（Acid2系）が通過する
- 同一テキストに対して layout/paint で実効フォント選択・文字幅計測の乖離が許容範囲内になる

## 進捗メモ（2026-03-21）

- layout 側の `measure_text_width` を単一 `sans-serif` 依存から複数候補フォント前提へ変更
- 文字単位で CJK 優先選択を行う計測ロジックを追加し、paint 側方針と整合するよう調整
- 実サイトAの確認用スクリーンショットを `tests/output/https-abehiroshi-la-coocan-jp.layout-paint-aligned.1366x900.actual.png` として出力
- `::-webkit-scrollbar` のような未対応擬似要素セレクタが通常要素に誤マッチしていた問題を修正
- 上記修正により `blog.ast.moe` で発生していた `body/main` 幅 `19px` への崩壊は解消（`@media ... ::-webkit-scrollbar { width: 19px }` の誤適用を防止）
- `:root` 疑似クラス対応と CSS custom property（`--*`）の継承/`var()` 解決を導入し、黒背景のみ表示される状態を解消
- `calc(var(--main-width) + var(--gap) * 2)` のような式を最小評価できるようにし、`max-width` などの長さ計算を一部復元
- `blog.ast.moe` は主要コンテンツが描画される段階まで改善したが、Firefox比較ではヘッダー周辺（ロゴ縦積み・メニュー配置）にFlexレイアウト差分が残る
- 次ステップ: Flex item の縮小/自動幅解決（`display:flex` + `margin:auto` + `justify-content: space-between` 組み合わせ）を実装差分の主対象として追う
