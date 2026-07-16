---
number: 074
slug: box-shadow-blur-buffer-reuse
status: closed
---

# box-shadow blurの作業バッファ再利用

## 概要

box-shadowで3回適用するbox blurのalpha抽出、一時バッファ確保、RGBAへの書き戻しをまとめる。

## 対応

- alpha配列と水平blur用配列を3 passで再利用する
- alpha抽出とRGBAへの書き戻しをshadowごとに各1回へ削減する
- 1 pass用テストと既存box-shadowテストでblur結果を維持する
- 固定fixtureのPNGサイズ・SHA-1が変わらないことを確認する

## 結果（2026-07-16）

Linux aarch64、rustc 1.97.0、release build、1280x720、warm-up 3回、計測20回。

| 指標（cold median） | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| paint | 31.044 ms | 29.456 ms | 5.1% |
| end-to-end | 36.435 ms | 35.066 ms | 3.8% |

- PNGサイズ: 3,687,468 bytes（変更なし）
- SHA-1: `6b8da0bccbf0b796b2a836fefff0bc13d5c858b3`（変更なし）

## 関連issue

- 071 レンダリング性能ベンチマーク基盤
- 073 不透明alpha blendのfast path
