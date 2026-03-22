---
number: 019-1
slug: woff2-decode
parent: 019-web-font-face
status: open
---

# WOFF2 フォントデコード対応

## 概要

WOFF2 形式のフォントファイル（brotli 圧縮）をデコードして利用可能にする。
Google Fonts 等の多くのサービスが WOFF2 を優先的に配信するため、実サイト対応に必須。

## 現状

- TTF / OTF: 対応済み（ab_glyph に直接渡す）
- WOFF1: 対応済み（zlib テーブル展開 → sfnt 再構築）
- WOFF2: 未対応（brotli 展開 + WOFF2 固有のテーブル再構築が必要）

## 対応方針

- brotli クレートを依存に追加
- WOFF2 ヘッダをパースし、brotli で展開
- WOFF2 のテーブル再構築（transform 適用の逆変換）
- または `woff2-decoder` クレートの利用を検討

## 受け入れ条件

- Google Fonts の WOFF2 ファイルがデコードできる
- `Font::load_from_bytes` で WOFF2 を自動検出してデコード
