---
number: 015-5
slug: ffi-boundary-refactor
parent: 015-anonymized-real-world-rendering-gap
status: closed
---

# FFI境界の責務整理（ffi肥大化の解消）

## 概要

現在 `src/ffi/mod.rs` にC ABIの橋渡しだけでなく、描画ロジックやHTML処理ロジックが集約されつつあり、
Rust APIとして再利用したい処理までFFI層に依存してしまう構造になっている。

本issueでは、FFIを「境界アダプタ」に限定し、コア機能は通常のRustモジュールに切り出して責務を分離する。

## 背景

- C向け公開APIとRust内部APIの責務が混在している
- FFI層の肥大化により、テスト/保守/拡張時の影響範囲が広い
- Rust側利用時にFFI経由を選びやすく、型安全・可読性の利点を活かしづらい

## 対応方針

1. `ffi/mod.rs` の責務を `extern "C"` の入出力変換とエラー橋渡しに限定する
2. 実処理をRust内部モジュール（例: screenshot/rendering/navigation など）へ移す
3. Rust利用コードは内部モジュールを直接呼び、FFI層を経由しない
4. FFIは薄いラッパーとして同等機能を維持する

## タスク

- `src/ffi/mod.rs` の関数を「境界処理」と「実処理」に棚卸し
- 実処理の切り出し先モジュールを設計し、段階的に移設
- 既存テストを移設後のモジュール責務に合わせて再配置
- FFI側は最小限の結線コードに簡素化

## 受け入れ条件

- `src/ffi/mod.rs` がC ABIアダプタ中心の構成になっている
- コア処理はRustモジュールとして直接テスト可能
- 既存のC API挙動（後方互換）が維持される
- `cargo test ffi::` および関連テストが通過する

## 備考

- 大規模な一括移動は避け、機能単位で段階的に進める
- PRでは「移動のみ」と「機能変更」を可能な限り分離する

## 実施結果

- `src/screenshot/mod.rs` を新設し、スクリーンショット実処理（frameset解決/描画ロジック）をFFI層から移設
- `src/ffi/mod.rs` は C ABI の入出力変換とエラー橋渡しを中心とした薄いラッパー構成へ簡素化
- コア処理テストを責務に合わせて再配置（`screenshot`/`html::encoding`）

## 検証

- `cargo test ffi::` 通過
- `cargo test screenshot::` 通過
- `cargo test html::encoding::` 通過
