---
number: 017-6
slug: shorthand-expansion
parent: 017-css-feature-gap
status: open
---

# margin / padding 等の shorthand 完全展開

## 概要

CSS shorthand プロパティの 1値/2値/3値/4値 展開を完全実装する。
現状は一部の shorthand（background, font, border, gap）のみ展開済み。

## 対象 shorthand

### 未展開（高優先）
- `margin` → margin-top/right/bottom/left
- `padding` → padding-top/right/bottom/left
- `border-width` → border-top-width/right-width/bottom-width/left-width
- `border-color` → border-top-color/right-color/bottom-color/left-color
- `border-style` → border-top-style/right-style/bottom-style/left-style
- `border-top`/`border-right`/`border-bottom`/`border-left` → width + style + color
- `overflow` → overflow-x/overflow-y (2値)
- `flex` → flex-grow/shrink/basis

### 既存の展開で不足があるもの
- `background` — background-size 未対応
- `font` — font-variant 未対応
- `border` — 部分的

## 展開ルール（CSS仕様）

```
1値: all sides
2値: top+bottom, left+right
3値: top, left+right, bottom
4値: top, right, bottom, left
```

## 受け入れ条件

- `margin: 10px 20px` が margin-top:10px, margin-right:20px, margin-bottom:10px, margin-left:20px に展開
- 1値/2値/3値/4値の全パターンがテスト通過
- 既存テストの回帰なし
