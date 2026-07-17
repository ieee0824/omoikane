---
number: 072
slug: png-checksum-performance
status: closed
---

# PNGチェックサム計算の高速化

## 概要

Issue 071のbaselineで全体の約52%を占めたPNG encodeを、出力互換性を保ったまま高速化する。

## 原因

無圧縮zlibストリームのCRC32を各バイト・各ビット単位で計算し、Adler-32でも各バイトごとに
剰余演算していた。1280x720 RGBAの約3.7MBに対して、このチェックサム計算がencode時間の大半を占めていた。

## 対応

- CRC32を`crc32fast`のテーブル/SIMD対応実装へ置換
- Adler-32をzlibの`NMAX`（5,552 bytes）単位で剰余する実装へ変更
- CRC32とAdler-32の標準既知ベクトルをテスト
- 固定fixtureのPNGサイズ・SHA-1が変更されないことを確認

## 結果（2026-07-16）

Linux aarch64、rustc 1.97.0、release build、1280x720、warm-up 3回、計測20回。

| 指標（cold median） | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| PNG encode | 41.312 ms | 3.312 ms | 92.0% |
| end-to-end | 78.828 ms | 40.797 ms | 48.2% |

- PNGサイズ: 3,687,468 bytes（変更なし）
- SHA-1: `6b8da0bccbf0b796b2a836fefff0bc13d5c858b3`（変更なし）

## 受け入れ条件

- チェックサムが標準既知ベクトルと一致する
- 固定fixtureのPNG出力が変更されない
- release benchmarkでPNG encodeとend-to-endが改善する
- `cargo test` と `cargo clippy --lib -- -D warnings` が通る

## 関連issue

- 071 レンダリング性能ベンチマーク基盤
