---
number: 033
slug: modern-site-rendering-gap
status: open
---

# モダンサイトのレンダリング精度向上

## 概要

モダンな CSS を使用するサイトで、テキストコンテンツが描画されない、またはレイアウトが大きく崩れる問題の調査と修正。

## 症状

- ナビゲーションテキストは表示される
- 本文テキスト（h2, h3, p, dl/dt/dd 等）が全く描画されない
- 背景色は正しく表示される（#F0F0F0 のグレー）
- 2000px の tall viewport でもコンテンツが見えない

## 原因候補

1. **`font-weight: 500` の未対応** — `*` セレクタで全要素に font-weight: 500 が設定されるが、未対応なら無視されている可能性
2. **CSS @import のフォント読み込み** — Google Fonts の @import が解決されていない可能性
3. **`overflow-x: hidden`** — body と .wrap に overflow-x: hidden が設定
4. **section/dl/dt/dd 等のレイアウト** — 特に dl (definition list) のレイアウトが未実装の可能性
5. **`text-align: center`** — .page-ttl 等で使用

## 調査手順

1. ミニマルなHTMLで section > h2 + dl/dt/dd のレイアウトを確認
2. font-weight: 500 が computed style に反映されるか確認
3. @import で Google Fonts CSS がフェッチされるか確認
