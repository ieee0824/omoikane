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

- [x] rustls設定をtransport modeごとに共有し、依存module間でTLS session cacheを再利用する
- [x] host・TLS mode・public IP制約単位でHTTP接続をpoolし、HTTP/1.1 keep-aliveとHTTP/2 sessionを再利用する
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

## 途中経過

rustlsの`ClientConfig`をリクエストごとに生成していたため、同一process内でもTLS session cacheが共有されていなかった。HTTP/2・HTTP/1.1と証明書検証有無ごとに設定を共有した。

x.com実測では、表示結果を維持したまま以下となった。

- 変更前: `render=71.5s`、`document-scripts=40.3s`
- 変更後1回目: `render=68.1s`、`document-scripts=37.6s`
- 変更後2回目: `render=67.1s`、`document-scripts=36.4s`
- 2回目の依存module内訳: 319 module、3,806,817 bytes、fetch合計27.7s、parse合計12.2s

TLS session再利用だけでは各moduleのTCP接続と逐次待機が残ったため、接続poolを追加した。さらに短縮する場合はmodule fetchの並列化が必要。

接続pool追加後、x.com/CDNはHTTP/2のheader decodeからHTTP/1.1へfallbackする経路だったため、HTTP/1.1 keep-aliveを含めて再利用した。実測では8接続を開き、322 requestで再利用、再利用失敗は0件だった。

- 接続pool前: `render=66〜71s`、`document-scripts=36〜40s`
- 接続pool後: `render=41.7s`、`document-scripts=12.4s`

表示結果はログインフォーム、Xロゴ、QRコードを含めて維持された。残る最大フェーズは約26.9秒のtimer実行。
