---
number: 088
slug: optimize-development-dependencies
status: open
priority: high
---

# 開発buildの依存crateを最適化する

## 概要

x.comの残り時間はBoaによる巨大JavaScript moduleのparse・評価が中心である。現在の`cargo run --example screenshot`は依存crateも最適化なしでbuildするため、実サイト検証時のCPUコストが過大になっている。

## 対応

- 開発profileで依存crateのみを最適化し、workspace本体のデバッグ性は維持する
- x.comの`javascript`、`timers`、render全体を変更前後で比較する
- unit/integration testの所要時間と互換性を確認する

## 完了条件

- x.comの表示結果を維持する
- debug情報を維持したまま実サイトレンダリング時間が再現可能に短縮する
- 全体の`cargo test`と`cargo build`が通る
