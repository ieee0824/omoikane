---
number: 013-3
slug: external-image-fetch
parent: 013-real-world-rendering
status: closed
---

# 外部画像のHTTPフェッチと描画

## 概要

`<img src="https://...">` の画像を HTTP 経由で取得し、レイアウト・描画する。
現在は `data:` URI の PNG 画像のみ対応済み。

## スコープ

- `<img src="...">` の src が http/https の場合に HTTPフェッチ
- PNG 画像のデコードと描画（既存の decode_png を利用）
- JPEG 画像のデコード対応
- 画像の intrinsic size をレイアウトに反映
- `width` / `height` HTML属性による画像サイズ指定
- 相対URLの解決
- 画像フェッチ失敗時の alt テキスト表示（フォント実装後）

## スコープ外（初期）

- SVG 画像
- WebP / AVIF
- `<picture>` / `srcset` によるレスポンシブ画像
- 遅延読み込み（loading="lazy"）
- CSS `background-image: url(https://...)` の外部画像

## 技術方針

- JPEG デコードは pure Rust クレートの利用を検討
- 画像キャッシュ（同一 URL の重複フェッチ防止）
