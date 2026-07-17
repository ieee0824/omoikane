---
number: 086
slug: optimize-es-module-graph-loading
status: open
priority: high
---

# ES module graphの取得・parseを高速化する

## 概要

x.comのdocument scriptsに約40秒かかっている。計測の結果、entry module自体のparseは約35msだが、依存moduleの`load/link/evaluate`が約39〜40秒を占めることが判明した。

現在の`HttpModuleLoader`は、依存moduleを要求されるたびに同期HTTP取得してからparseしている。x.comでは数百moduleが読み込まれ、1件あたり概ね90〜120msの取得待ちが発生する。また、大きなbundleのparseも無視できない。

## 計測例

- `castle.umd-C4qsRRNW.js`: 493,832 bytes、fetch 177.9ms、parse 7,050.0ms
- `sentry-filter-CHh7F15L.js`: 396,824 bytes、fetch 129.6ms、parse 775.3ms
- `relay-runtime-rgn7krKZ.js`: 187,090 bytes、fetch 125.7ms、parse 281.8ms
- 全体: `render=71.5s`、`javascript=40.7s`、`timers=28.4s`

## 対応方針

- module graph取得が逐次化されている箇所とBoaのloader呼び出し順を確認する
- 独立した依存moduleの並列取得、またはリンク前の先読みを検討する
- URL単位のmodule source・parse結果のキャッシュ範囲を確認する
- 巨大moduleのparse時間を分離し、不要なdynamic importを早期に読み込んでいないか確認する
- x.comの同一条件で変更前後を複数回計測する

## 完了条件

- QRコードを含むx.comの表示結果を維持する
- module取得・parse・実行の内訳を変更前後で比較できる
- document scriptsまたはrender全体で再現可能な短縮を確認する
- 関連するunit/integration testと全体の`cargo test`、`cargo build`が通る
