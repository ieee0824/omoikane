---
number: 019-2
slug: font-weight-style-matching
parent: 019-web-font-face
status: open
---

# font-weight / font-style バリアント選択

## 概要

@font-face の font-weight / font-style 記述子に基づいて、適切なフォントバリアントを選択する。
現状は同じ font-family に対して最初にロードされたフォントファイルのみが使用される。

## 対応内容

- @font-face の font-weight 記述子（100〜900、normal、bold）を解析
- @font-face の font-style 記述子（normal、italic、oblique）を解析
- CSS の font-weight / font-style 指定に最も近いバリアントを選択
- 同じ font-family で複数の @font-face ルールがある場合に適切に分岐

## 受け入れ条件

- `font-weight: bold` 指定時に bold バリアントが選択される
- `font-style: italic` 指定時に italic バリアントが選択される
- バリアントがない場合は最も近いものにフォールバック
