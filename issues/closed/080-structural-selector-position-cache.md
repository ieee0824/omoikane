---
number: 080
slug: structural-selector-position-cache
status: closed
---

# structural selectorの親単位位置cache

## 概要

同じ親の`:nth-child` / `:nth-of-type`照合で兄弟全体を繰り返し走査せず、親ごとの位置表を再利用する。

## 受け入れ条件

- 2,000兄弟benchmarkでIssue 079後のbaselineから改善する
- DOM/style cache無効化時に位置cacheも破棄する
- child/of-type系pseudo-classの結果を維持する
- `cargo test`と`cargo clippy --lib -- -D warnings`が通る

## 関連issue

- 079 structural pseudo-classの兄弟走査改善

## 結果（2026-07-17）

Linux aarch64、rustc 1.97.0、release build。2,000兄弟、10回計測。

| style resolve median | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| `span:nth-child(odd)` | 16.467 ms | 4.028 ms | 75.5% |

Issue 079前の19.922msからは79.8%短縮。StyleResolverが親node IDごとにchild/of-typeの
位置と総数を保持し、style cache無効化時に同時破棄する。
