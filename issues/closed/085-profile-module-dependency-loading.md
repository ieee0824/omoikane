---
number: 085
slug: profile-module-dependency-loading
status: closed
---

# ES module依存読込時間を分割計測する

## 概要

x.com entry moduleの`load/link/evaluate`が約39秒を占めるが、その中に依存moduleのHTTP取得とparseが含まれている。

## 対応

- module cache hit、HTTP取得、source byte数、parse時間をURL別にデバッグログへ出す
- x.comで再計測し、ネットワーク・parse・実行の優先度を確定する

## 結果

- x.comでは数百件の依存module読込とcache hitが発生した
- 多くの依存moduleはHTTP取得だけで約90〜120msを要した
- entry module自体のparseは約35msだが、依存moduleを含む`load/link/evaluate`は約39〜40秒だった
- `castle.umd-C4qsRRNW.js`は493,832 bytes、fetch 177.9ms、parse 7,050.0msだった
- `sentry-filter-CHh7F15L.js`は396,824 bytes、fetch 129.6ms、parse 775.3msだった

これにより、HTML/DOMの前処理ではなく、逐次的なmodule取得と一部の巨大module parseを優先して改善すべきと判断した。改善作業はIssue 086で追跡する。
