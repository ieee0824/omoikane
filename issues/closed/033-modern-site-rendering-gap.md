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

## 根本原因（特定済み）

**`.fade { opacity: 0 }` + JavaScript 依存のアニメーション**

- サイトは `.fade` クラスで全コンテンツを `opacity: 0` に設定
- JavaScript の IntersectionObserver 等でスクロール時に `.fade.on` を追加
- `.fade.on { animation: fadein 1.2s ... forwards }` で表示される
- 当方は JS エンジンが未統合のため `.on` クラスが追加されず、全コンテンツが透明のまま

## 対応方針

### 短期（ワークアラウンド）
- `--force-opacity` オプションで `opacity: 0` を無視してレンダリング
- または `noscript` 時の代替スタイルを適用

### 中長期
- CSS animation/transition の基本対応（initial state → final state の即時適用）
- animation の `forwards` fill-mode で最終状態を表示
- JavaScript エンジン統合（フェーズ4）

## 調査ログ

1. ミニマルHTMLでは正常にテキスト描画される ✅
2. 外部CSS (stylehis.css) + ミニマルHTMLでもテキスト描画される ✅
3. フルHTMLで描画されないのは `.fade { opacity: 0 }` が原因 ✅
4. dl/dt/dd, section, h2 等のレイアウトは正常 ✅
5. box-sizing: border-box は対応済み ✅
