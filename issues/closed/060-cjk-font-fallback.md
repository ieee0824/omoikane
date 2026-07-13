---
number: 060
slug: cjk-font-fallback
status: closed
---

# CJK グリフのフォントフォールバック修正（豆腐解消）

## 概要

ページ指定フォント（例: `"Zen Kaku Gothic New", sans-serif` や webfont の `Jost`）が
CJK グリフを持たない場合に、日本語が豆腐（□）で描画される。primary フォントが
`.notdef` を描いてしまい Noto Sans CJK JP へフォールバックしないのが原因。

## 背景（実サイトログ由来: kasaneteto.jp）

- コンテナには Noto Sans CJK JP がインストール済み（issue 052）で、デフォルトフォント
  スタック `load_default_text_fonts()`（src/font/mod.rs）にも Noto CJK が含まれる
- しかし `rasterize_with_fallback`（src/paint/text.rs:404-）は index 0（primary＝
  webfont や sans-serif 解決フォント）について `.notdef` を「visible last resort」として
  そのまま描く分岐があり（text.rs:412-414 の `if index != 0 && ... !has_glyph` ガードを
  index 0 は素通り）、CJK 文字が primary の豆腐グリフで描かれフォールバックに到達しない
- webfont が primary の場合（text.rs:80- の `variant_fonts` = [web_font, ...fallback]）も
  同様に web_font（index 0）が CJK を持たないと豆腐になりうる

## スコープ

- CJK（および primary が glyph を持たない任意の文字）で、primary が当該 glyph を
  持たない場合は**必ずフォールバック**（Noto Sans CJK JP 等）を先に試し、どの
  フォールバックも持たない時のみ最終手段として primary の .notdef を描く
- `is_cjk_preferred_character` の範囲が kasaneteto の文字（ひらがな/カタカナ/漢字/
  全角記号）を確実にカバーするか確認・補完
- webfont primary 経路（variant_fonts）でも同じフォールバック順序を適用
- `var(--f-f-ja)` 等 CSS 変数が font-family で解決されているか確認（未解決なら別 issue に
  切り出し。本 issue はグリフフォールバックに集中）

## 受け入れ条件

- kasaneteto.jp のレンダリングで日本語ナビ項目（プロフィール等）が豆腐でなく可読な
  グリフで描画される
- ラテン文字は従来どおり primary（webfont/sans-serif）で描画され回帰しない
- フォントフォールバックの単体テスト（CJK 文字が primary 非対応時に CJK フォントの
  index を返す）を追加

## 関連

- 052 サンドボックスの日本語フォント追加（フォント導入済み）
- 044 Web API / 実サイト品質トラック
