---
number: 017-4
slug: box-shadow-opacity
parent: 017-css-feature-gap
status: open
---

# box-shadow / opacity 対応

## 概要

カード、モーダル、オーバーレイ等で頻出する影と透明度を実装する。

## 対象プロパティ

- `box-shadow` — オフセット、ぼかし、広がり、色、inset
- `opacity` — 要素全体の不透明度 (0.0〜1.0)

## 実装方針

### box-shadow
- パース: `h-offset v-offset blur spread color` + optional `inset`
- 描画: ボックスの外側（or inset なら内側）にぼかし付き矩形を描画
- ぼかしは Gaussian blur か box blur の近似で実装

### opacity
- 要素の描画を一旦オフスクリーンバッファに描画
- バッファ全体に alpha を乗算してメインキャンバスに合成
- 子要素にも伝播

## 受け入れ条件

- box-shadow の基本形（offset + blur + color）が描画される
- opacity が要素全体に適用される
