---
number: 079
slug: structural-pseudo-sibling-scan
status: closed
---

# structural pseudo-classの兄弟走査改善

## 概要

`:nth-child`や`:nth-of-type`の照合ごとに兄弟Vecを再構築・再走査する処理を改善する。

## 受け入れ条件

- 多数の兄弟へstructural pseudo-classを適用するbenchmarkを追加する
- child/of-typeの位置と総数を正しく維持する
- 中間Vecの生成を除去し、必要に応じて親単位の位置cacheを導入する
- `cargo test`と`cargo clippy --lib -- -D warnings`が通る

## 関連issue

- 078 general sibling selectorの二重走査除去

## 結果（2026-07-17）

Linux aarch64、rustc 1.97.0、release build。2,000兄弟、10回計測。

| style resolve median | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| `span:nth-child(odd)` | 19.922 ms | 16.467 ms | 17.3% |

filtered sibling Vecの生成とposition再検索を、位置と総数を同時に求める単一走査へ変更した。
親単位のstructural position cacheは別issueで扱う。
