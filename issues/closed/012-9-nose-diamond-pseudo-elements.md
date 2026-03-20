---
number: 012-9
slug: nose-diamond-pseudo-elements
parent: 012-acid2-official-conformance
status: closed
---

# 鼻のダイヤモンド形状（`:before` / `:after` の border triangle）

## 概要

Acid2 の `.nose div div` の中央に赤い四角（2em × 2em）があり、
`:before` と `:after` 擬似要素の border で上下三角形を描いてダイヤモンド形状を作る。
赤い四角がそのまま見える状態は解消済みで、現在はダイヤモンド形状まで到達している。
ただし公式比較では対角線境界に微小なピクセル差が残っている（全体差分 28px のうち 12px が主因）。

## 仕様参照

- CSS 2.1 §12.1 The :before and :after pseudo-elements
- border triangle テクニック: height:0 + border で三角形を描画

## CSS

```css
.nose div div { width: 2em; height: 2em; background: red; margin: auto; }
.nose div div:before { display: block; border-style: none solid solid; border-color: red yellow black yellow; border-width: 1em; content: ''; height: 0; }
.nose div    :after { display: block; border-style: solid solid none; border-color: black yellow red yellow; border-width: 1em; content: ''; height: 0; }
```

## スコープ

- `:before` の border triangle: 上半分が黄色/赤、下半分が黒/黄色で下向き三角
- `:after` の border triangle: 上半分が黒/黄色、下半分が赤/黄色で上向き三角
- 二つの三角形が組み合わさってダイヤモンド（ひし形）を描く
- `margin: auto` による中央配置
- border の各辺が異なる色を持つ場合の描画（斜め分割は不要、矩形でOK）
- 1px 境界の塗り分け（triangle の内外判定、floor/ceil 丸め）を詰めて公式比較差分を 0 にする
