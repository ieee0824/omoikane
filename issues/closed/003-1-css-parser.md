---
id: 003-1
title: CSSパーサー
phase: 2
status: closed
parent: 003
---

# CSSパーサー

CSS Syntax Module (Level 3) に基づくパーサーの実装。

## タスク
- [x] CSSトークナイザ（ident, number, string, delim等）
- [x] スタイルシートのパース（ルールリスト）
- [x] セレクタのパース（型、クラス、ID、属性、擬似クラス、擬似要素）
- [x] 結合子のパース（子孫、子、隣接、一般兄弟）
- [x] プロパティ値のパース（キーワード、長さ、色、関数等）
- [x] @ルール（@media, @import, @font-face）
- [x] ショートハンドプロパティの展開（margin, padding, border等）
