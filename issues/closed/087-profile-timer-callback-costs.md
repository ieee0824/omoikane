---
number: 087
slug: profile-timer-callback-costs
status: closed
priority: high
---

# timer callbackの実行コストを分割計測する

## 概要

HTTP接続再利用後のx.comでは、render約41.7秒のうちtimer処理が約26.9秒を占める。過去の計測では実行macrotaskは約251件であり、仮想時間を進めるloop自体よりcallbackまたはcallback後のPromise jobが重い可能性がある。

## 対応

- timer payload種別（callback、source、resource load）ごとの件数と時間を計測する
- callback本体とcallback後のjob queue処理を分けて計測する
- x.comでslow taskを特定する
- DOM結果を維持できる範囲で不要なtimer反復または高コスト処理を削減する

## 完了条件

- timer約26.9秒の主要因を実測値で説明できる
- 改善後もx.comのログインフォームとQRコードが表示される
- 関連テストと全体の`cargo test`、`cargo build`が通る

## 結果

x.comで251 macrotaskを実行し、内訳は以下だった。

- callback本体合計: 約6.1秒
- callback後のPromise/job queue合計: 約21.3秒
- task 4のjob queueだけで約20.2〜20.6秒

task 4は`castle.umd-C4qsRRNW.js`をdynamic importしていた。このmoduleは493,832 bytesで、接続pool追加後のfetchは約41msまで短縮された一方、parseに約6.95秒、残りのmodule評価などに約13.2秒を要した。

したがってtimer管理loopやDOM前処理ではなく、最適化なしの開発buildにおけるBoaの巨大module parse・評価が残る主要因と判断した。
