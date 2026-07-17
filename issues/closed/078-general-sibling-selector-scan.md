---
number: 078
slug: general-sibling-selector-scan
status: closed
---

# general sibling selectorの二重走査除去

## 概要

`A ~ B`の照合で、previous siblingを1つ進めるたびに親childrenから現在位置を再検索する`O(S²)`処理を
兄弟配列1回の逆走査`O(S)`へ変更する。

## 受け入れ条件

- 多数の兄弟を持つ専用benchmarkで改善を記録する
- adjacent/general sibling selectorの照合結果を維持する
- `cargo test`と`cargo clippy --lib -- -D warnings`が通る

## 関連issue

- 076 CSS rule候補indexによるstyle解決高速化

## 結果（2026-07-16）

Linux aarch64、rustc 1.97.0、release build。2,000兄弟、20回計測。

| style resolve median | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| `.anchor ~ .target` | 9.132 ms | 0.049 ms | 99.5% |

親childrenの取得と現在位置検索を各1回にし、先行兄弟を同じslice上で逆走査する。
