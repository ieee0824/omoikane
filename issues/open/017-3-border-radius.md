---
number: 017-3
slug: border-radius
parent: 017-css-feature-gap
status: open
---

# border-radius（角丸）対応

## 概要

ボタン、カード、画像等で頻出する角丸ボーダーを実装する。

## 対象プロパティ

- `border-radius` (shorthand)
- `border-top-left-radius`
- `border-top-right-radius`
- `border-bottom-right-radius`
- `border-bottom-left-radius`

## 実装方針

- shorthand を 4 つの longhand に展開（1値/2値/3値/4値）
- paint 時に各コーナーを楕円弧で描画
- 背景色・背景画像・ボーダー描画で角丸をクリッピング
- overflow: hidden との組み合わせ考慮

## 受け入れ条件

- 4 コーナーの角丸が正しく描画される
- 背景色とボーダーが角丸に沿う
- shorthand の展開テストが通過
