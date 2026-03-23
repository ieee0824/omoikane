---
number: 039
slug: html5-media-elements
status: open
---

# HTML5 メディア要素対応

## 概要

`<video>`, `<audio>`, `<canvas>`, `<source>`, `<picture>` 等のメディア要素の基本レンダリングを実装する。

## 対象要素

| 要素 | 最小実装 |
|------|---------|
| `<video>` | poster 属性の画像表示、width/height でサイズ確保 |
| `<audio>` | display: none（非表示）、controls 属性時はプレースホルダー |
| `<canvas>` | width/height でサイズ確保（空矩形） |
| `<source>` | 非レンダリング（親要素の属性として処理） |
| `<picture>` | 内部の `<img>` にフォールバック |

## 優先度

中 — モダンサイトでの出現頻度は中程度
