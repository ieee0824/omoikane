---
number: 067
slug: element-style-cssom-parser-robustness
parent:
status: open
---

# el.style（CSSOM）パーサの `;` 分割誤パース修正

## 概要

`el.style`（CSSStyleDeclaration Proxy）を支える `parseDecls`（`src/js/dom_bootstrap.js` 722-741 付近）は
style 属性値を素朴に `split(';')` で分割するため、`url(data:...;base64,...)` を含む値を誤パースする。
引用符・括弧深度を考慮した分割に置き換える。

## 背景（047 のスコープ判断）

- 047 のカスケード統合で getComputedStyle 側の `__parseInlineStyle` は削除されるが、
  el.style の `parseDecls` は別パーサとして残り、同じ `;` 分割バグを持つ
- data-URI を含む style 属性への `el.style.*` の読み書き（read-modify-write）で値が破壊される
- 047 の PR スコープを絞るため別 issue に切り出した（2026-07-13）

## 対応内容

- `parseDecls` の分割を引用符（`'` / `"`、エスケープ含む）・括弧深度を考慮した実装に置き換える
- 宣言値セマンティクス（生の宣言値を返す）は維持する — 既存 CSSOM テスト12件
  （`src/js/mod.rs` 3958〜4570 付近の `style_*` 系）の期待値を変更しない
- `el.style.backgroundImage` が引用符付き / 引用符なし双方の data-URI で正しく読めるテストを追加

## 優先度

中 — 実サイトで data-URI 入りインライン style は頻出。047 完了後に着手可能。
