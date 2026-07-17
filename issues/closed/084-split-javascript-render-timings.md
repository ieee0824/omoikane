---
number: 084
slug: split-javascript-render-timings
status: closed
---

# render前JavaScript処理の計測を分割する

## 概要

x.comでは`javascript`フェーズが約40〜44秒を占めるが、runtime初期化・document script・load eventが一括計測されており、改善対象を特定できない。

## 対応

- runtime初期化、document scripts、inline handler/load eventを個別計測する
- screenshot CLIにJavaScript内訳を出力する
- 各scriptの取得、parse、compile、execute、job処理時間をデバッグログへ出す
- x.comで再計測する

## 計測結果

- 全体: `render=70269.2ms`
- JavaScript: `39701.7ms`
- runtime初期化: `251.3ms`
- document scripts: `39298.1ms`
- load event: `151.7ms`
- x.com entry module: `parse=35.2ms`, `load/link/evaluate=38964.3ms`

構文parseやruntime初期化ではなく、entry moduleのload/link/evaluateが主要なボトルネックである。
