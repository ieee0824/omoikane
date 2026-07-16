---
number: 075
slug: opaque-rectangle-fill-fast-path
status: closed
---

# 不透明矩形fillの行単位fast path

## 概要

背景などの完全不透明な矩形を、汎用alpha blend経由ではなく行内のRGBA直接書き込みで描画する。

## 対応

- clip後の矩形が完全不透明なら、行sliceをRGBA pixel単位で直接更新する
- 半透明色は既存のalpha blend経路を維持する
- 既存Canvas矩形テストと固定fixtureのPNG完全一致を確認する

## 結果（2026-07-16）

Linux aarch64、rustc 1.97.0、release build、1280x720、warm-up 3回、計測20回。

| 指標（cold median） | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| paint | 29.456 ms | 27.985 ms | 5.0% |
| end-to-end | 35.066 ms | 33.345 ms | 4.9% |

- PNGサイズ: 3,687,468 bytes（変更なし）
- SHA-1: `6b8da0bccbf0b796b2a836fefff0bc13d5c858b3`（変更なし）

## 関連issue

- 071 レンダリング性能ベンチマーク基盤
- 073 不透明alpha blendのfast path
- 074 box-shadow blurの作業バッファ再利用
