---
number: 088
slug: optimize-development-dependencies
status: closed
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

## 結果

`[profile.dev.package."*"] opt-level = 2`を設定した。omoikane本体は従来どおり
`unoptimized + debuginfo`を維持し、Boaを含む依存crateのみを最適化する。

Firefox UA、1280x720、x.comの同条件計測結果:

| 指標 | 変更前 | 変更後 |
| --- | ---: | ---: |
| render全体 | 42167.7ms | 12669.6ms |
| JavaScript | 13203.9ms | 7265.3ms |
| timers | 26977.6ms | 3410.2ms |
| document scripts | 12822.9ms | 7161.1ms |
| `castle.umd` parse | 6951.3ms | 1126.2ms |
| `castle.umd`を処理するtask 4 | 20154.1ms | 2349.1ms |

render全体は約70%短縮、最大のボトルネックだったtask 4は約88%短縮した。
初回の依存crate再buildには3分33秒を要するが、以降の実サイト検証は高速になる。
生成画像`/tmp/test-dev-deps-opt.png`を目視し、Xログイン画面、ロゴ、QRコードの
表示が変更前と同等であることを確認した。
