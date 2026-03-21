---
number: 017-5
slug: text-decoration-transform
parent: 017-css-feature-gap
status: open
---

# text-decoration / text-transform / letter-spacing 対応

## 概要

テキスト装飾と変換に関するプロパティを実装する。

## 対象プロパティ

- `text-decoration` / `text-decoration-line` — underline, overline, line-through, none
- `text-decoration-color`
- `text-decoration-style` — solid, dashed, dotted, double, wavy
- `text-transform` — uppercase, lowercase, capitalize, none
- `letter-spacing` — 文字間隔
- `word-spacing` — 単語間隔
- `text-indent` — 段落先頭インデント
- `text-overflow` — ellipsis, clip

## 実装方針

- text-decoration: paint 時にテキスト baseline 付近に線を描画
- text-transform: テキスト描画前に文字列を変換
- letter-spacing: グリフ間の advance に加算
- text-overflow: overflow:hidden + 幅超過時に省略記号

## 受け入れ条件

- underline/line-through が描画される
- text-transform: uppercase/lowercase が動作する
- letter-spacing が反映される
