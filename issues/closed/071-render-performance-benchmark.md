---
number: 071
slug: render-performance-benchmark
status: closed
---

# レンダリング性能ベンチマーク基盤

## 概要

レンダリング速度改善に着手する前に、ネットワークの揺れを除外した再現可能なベンチマークを整備し、
HTML/CSS解析、style解決、layout、paint、PNG encodeの所要時間をbaselineとして記録する。

## 背景

x.comのレンダリング精度改善により、Webフォントを使う文字幅計測、inline SVG rasterize、
角丸border描画などの処理が増えた。一方、現在はスクリーンショット全体の経過時間しか把握できず、
どの工程や重複計算を優先して整理すべきか判断できない。

既存issue 048ではlayout metricsとcomputed styleの世代キャッシュを扱っているが、実測結果がない状態で
キャッシュを追加すると複雑性だけが増える可能性があるため、本issueのbaselineを先行させる。

## 対応内容

- 外部通信を行わない、モダンな実サイト相当の固定HTML/CSS fixtureを追加する
- 1280x720を標準viewportとするレンダリングベンチマークコマンドを追加する
- warm-up後に複数回計測し、min / median / mean / p95を表示する
- 少なくとも以下を個別計測する
  - HTML parse
  - stylesheet収集・CSS parse / style resolver構築
  - layout
  - paint
  - PNG encode
  - end-to-end
- cold相当（毎回documentとresolverを再構築）とwarm相当（再利用可能な入力を保持）を区別する
- optimizerによる処理消去を避け、各反復のPNGサイズまたはdigestを検証する
- 実行環境、反復回数、測定結果をissueへ記録する

## 対象外

- 本issue内でのキャッシュ追加やアルゴリズム変更
- ネットワーク取得時間の測定
- ベンチマーク結果を固定時間のテスト閾値にしてCIを不安定化させること

## baseline（2026-07-16）

- 実行環境: Linux aarch64 / rustc 1.97.0
- build: `--release`
- viewport: 1280x720
- warm-up: 3回、計測: 20回
- 出力: 3,687,468 bytes、SHA-1 `6b8da0bccbf0b796b2a836fefff0bc13d5c858b3`

| mode / stage | median (ms) | p95 (ms) |
| --- | ---: | ---: |
| cold / HTML parse | 0.114 | 0.162 |
| cold / style | 0.093 | 0.110 |
| cold / layout | 1.810 | 2.059 |
| cold / paint | 35.266 | 37.639 |
| cold / PNG encode | 41.312 | 42.458 |
| cold / total | 78.828 | 83.567 |
| warm DOM / style | 0.111 | 0.135 |
| warm DOM / layout | 1.843 | 1.950 |
| warm DOM / paint | 35.295 | 36.863 |
| warm DOM / PNG encode | 41.477 | 42.742 |
| warm DOM / total | 78.594 | 81.702 |

coldのmedianではPNG encodeが全体の約52%、paintが約45%を占め、両工程の合計が約97%となった。
DOM parse再利用による差は約0.23ms（0.3%）に留まるため、次の改善候補はpaintとPNG encodeを優先する。

## 受け入れ条件

- 単一コマンドで固定fixtureのベンチマークを実行できる
- 工程別およびend-to-endの統計が機械可読形式でも取得できる
- 同一入力の全反復で出力が一致することを検証する
- release buildでbaselineを取得し、本issueに測定条件と結果を追記する
- `cargo test` と `cargo clippy --lib -- -D warnings` が通る

## 関連issue

- 048 layout metrics・style解決キャッシュ
- 070 flex column grow後の再layout
