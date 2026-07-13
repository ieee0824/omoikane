---
number: 062
slug: clip-path-inset
status: open
---

# clip-path: inset() の描画クリップ対応

## 概要

`clip-path` が未対応のため、サイトが「クリップで非表示」にしている要素がそのまま
全面描画される。kasaneteto.jp では以下により**ページ全域を覆う赤ブロック**が発生し、
grid（061）修正後も見た目が崩壊したままになっている（レンダリング実測で特定済み）。

- `.l-header__content`: `position: fixed; inset: 0; background-color: var(--c-red);
  clip-path: inset(0 0 100% 0)` — 全クリップ（＝非表示）のはずのメニューオーバーレイが全面赤で描画
- `.p-home-about__bg--red`: `padding: 200vh 0 ...; background-color: var(--c-red);
  clip-path: inset(100% 0 0 0)` — スクロール演出用の隠し赤ブロックが 200vh ぶん描画

## スコープ

1. `clip-path` / `-webkit-clip-path` を is_supported_property に登録
2. `inset(top right bottom left [round ...])` をパース（px / % / calc。round は当面無視可）
3. paint 時に要素の描画（背景・ボーダー・内容・子孫）をクリップ矩形で切り抜く
   - 最低限、**クリップ結果が空（100% inset 等）の要素を描画しない**だけでも
     kasaneteto.jp の赤ブロックは解消する。段階実装可
4. inset() 以外の形状（circle/ellipse/polygon/url）は当面パースのみ受理して
   クリップ無し（現状挙動）でフォールバック。ログは抑制

## 受け入れ条件

- `clip-path: inset(0 0 100% 0)` の要素が描画されない回帰テスト
- 部分 inset のクリップ矩形テスト（具体値アサート）
- kasaneteto.jp の全面赤ブロックが解消する
- 既存テスト・Acid3 スコア（97/100）の維持

## 関連

- 061 CSS Grid（同サイト。grid 側は 061-4/061-5 で解消済み）
- 063 CSS マスキング（mask / -webkit-mask。ヒーローの赤矩形はこちら）
