---
number: 073
slug: opaque-alpha-blend-fast-path
status: closed
---

# 不透明alpha blendのfast path

## 概要

paintの全描画要素が通るpixel blendで、完全不透明色に不要なalpha合成と除算を行わないようにする。

## 対応

- source alphaが255の場合はRGBAを直接コピーして終了する
- 半透明・透明色の既存経路は変更しない
- 不透明色がdestinationを完全に置換する回帰テストを追加する
- 固定fixtureのPNGサイズとSHA-1が変わらないことを確認する

## 結果（2026-07-16）

Linux aarch64、rustc 1.97.0、release build、1280x720、warm-up 3回、計測20回。

| 指標（cold median） | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| paint | 35.352 ms | 31.044 ms | 12.2% |
| end-to-end | 40.797 ms | 36.435 ms | 10.7% |

- PNGサイズ: 3,687,468 bytes（変更なし）
- SHA-1: `6b8da0bccbf0b796b2a836fefff0bc13d5c858b3`（変更なし）

## 関連issue

- 071 レンダリング性能ベンチマーク基盤
- 072 PNGチェックサム計算の高速化
