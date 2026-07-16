---
number: 076
slug: css-rule-candidate-index
status: open
---

# CSS rule候補indexによるstyle解決高速化

## 概要

大規模DOM/CSSで各要素が全style ruleを走査する`O(N × R)`経路を、selector右端のID・class・tagを
使った候補indexで削減する。

## 対応内容

- 外部通信なしで多数の要素・ruleを生成するstyle専用benchmarkを追加する
- ID・class・tag・universal/複雑規則にruleを分類する
- computed styleでは対象要素に関係する候補ruleだけをselector照合する
- `@media`など条件付き規則は互換性を保つ安全なfallback経路を用意する
- 出力・cascade・source orderを既存テストで維持する

## 受け入れ条件

- 大規模DOM/CSS benchmarkで変更前baselineと変更後結果を記録する
- 既存CSS、layout、paintの出力が変わらない
- `cargo test`と`cargo clippy --lib -- -D warnings`が通る

## baseline（2026-07-16）

Linux aarch64、rustc 1.97.0、release build。1,000要素×1,000 class rule、10回計測。

- 属性Map clone除去前: median 100.640ms
- Issue 077適用後（rule index導入前）: median 31.588ms
- 右端selector候補フィルタ適用後: median 18.974ms（Issue 077適用後から39.9%短縮）

現在は各ruleの軽量な候補判定が残っている。次段階でID・class・tagのHashMap indexから
候補ruleを直接列挙し、`O(N × R)`のrule走査自体を削減する。

## 関連issue

- 071 レンダリング性能ベンチマーク基盤
- 048 layout metrics・style解決キャッシュ
