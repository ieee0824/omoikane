---
number: 034-3
slug: not-specificity-fix
parent: 034-css-spec-compliance-fixes
status: open
---

# :not() specificity の修正

## 概要

CSS Selectors Level 4 §17.2 に準拠し、`:not()` の specificity 計算を修正する。

## 問題

現在の実装では `:not()` 内のセレクタの specificity を合計して親に加算している。

```rust
SimpleSelector::Not(inner) => {
    let max_inner = inner.iter().fold(Specificity::zero(), |mut acc, s| {
        add_simple_specificity(&mut acc, s);
        acc
    });
    // これが合計値を加算 → 仕様では最も高いもの1つのみ
}
```

## 仕様

- `:not()` 自体は specificity 0
- 引数が単一 compound selector の場合: そのセレクタの specificity を加算
- 引数が selector list の場合（CSS Selectors 4）: 最も高い specificity のもの1つを加算

## 現状の動作

- `:not(.foo)` → (0,1,0) — 正しい
- `:not(.foo#bar)` → (1,1,0) — 正しい（compound の合計）
- 将来 `:not(.foo, #bar)` を実装した場合 → max((0,1,0), (1,0,0)) = (1,0,0) にすべき

## 修正方針

現在は compound selector のみ対応なので合計で正しい。ただしコメントを「最大値」から「compound の合計」に修正し、将来の selector list 対応時の TODO を残す。

## 修正箇所

- `src/css/matcher.rs` の `add_simple_specificity` 内のコメント修正
