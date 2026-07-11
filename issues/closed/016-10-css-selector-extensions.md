---
number: 016-10
slug: css-selector-extensions
parent: 016
status: closed
---

# querySelector の matcher.rs 接続とセレクタ拡充

## 目的

本格マッチャ `src/css/matcher.rs` を querySelector 系に接続し、対応セレクタを拡充する。

## 背景（GAP_ANALYSIS.md セクション3 領域H）

- 現状 querySelector 系は JS 層で単純セレクタ（`tag` / `.class` / `#id`）のみ対応
  （`matches_simple_selector`, `src/js/mod.rs`）。複合・属性・擬似クラスは非対応。
- `matcher.rs` は存在するが querySelector に接続されておらず、対応擬似クラスも限定的。
- bucket3（test 33〜44）のセレクタ判定に必要。既存 matcher で 33,35,36,41,42 は足りるが、
  34,37,38,39,40,43 のサブ条件に追加実装が要る。

## スコープ

- querySelector / querySelectorAll を matcher.rs に接続
- セレクタ拡充: of-type 系、`:only-child`、`:empty`、nth の an+b 式、`:nth-last-*`、
  `:lang`、UI 状態擬似クラス（`:enabled` / `:disabled` / `:checked`）

## 受け入れ条件

- 複合セレクタ・属性セレクタ・擬似クラスが querySelector で解決される
- 上記拡充セレクタの単体テストが通る
