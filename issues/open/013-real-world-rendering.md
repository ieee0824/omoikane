---
number: 013
slug: real-world-rendering
parent:
status: open
---

# 実ページレンダリング対応

## 概要

https://kasaneteto.jp/history/ をレンダリングした結果、HTMLパース→レイアウト→描画のパイプラインは動作するが、テキストが黒い四角として表示され実用的な描画にはなっていない。
実ページを視認可能なレベルで描画するために必要な機能を段階的に実装する。

## 現状の問題（kasaneteto.jp/history/ の描画結果から）

1. **フォントグリフレンダリング未実装** — テキストが固定幅の黒四角として描画される
2. **外部CSSの読み込み未実装** — `<link rel="stylesheet" href="...">` のHTTP経由CSS取得ができない
3. **外部画像の読み込み未実装** — `<img src="https://...">` の画像フェッチ・描画ができない
4. **CJKテキストの行折り返し** — 日本語テキスト固有の禁則処理がない
5. **viewport meta** — `<meta name="viewport">` の解釈がない

## 子issue

- [013-1 フォントグリフレンダリング](../closed/013-1-font-glyph-rendering.md) ✅ 完了
- [013-2 外部CSSのHTTPフェッチと適用](../closed/013-2-external-css-fetch.md) ✅ 完了
- [013-3 外部画像のHTTPフェッチと描画](013-3-external-image-fetch.md)
- [013-4 CJKテキストの行折り返し・禁則処理](013-4-cjk-line-breaking.md)
- [013-5 paint_text()へのグリフ描画統合](013-5-paint-text-glyph-integration.md) — 013-1 の残作業

## 優先度

1. **013-2（外部CSS）** — スタイルがないとレイアウトが全く正しくならない
2. **013-1（フォント）** — テキストが読めないと何も判断できない
3. **013-3（外部画像）** — ページの視覚的な完成度
4. **013-4（CJK）** — 日本語ページで必須
