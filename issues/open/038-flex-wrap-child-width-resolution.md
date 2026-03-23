---
number: 038
slug: flex-wrap-child-width-resolution
status: open
---

# flex-wrap: wrap 時の子要素幅解決

## 概要

`display: flex; flex-wrap: wrap` のコンテナで、子要素に `width: 165px` や `width: calc(100% - 165px)` が指定されている場合に、子要素が正しく横並びにならない。

## 再現条件

```css
.history li { display: flex; flex-wrap: wrap; }
.history li h3 { width: 165px; }
.history li dl { width: calc(100% - 165px); display: flex; flex-wrap: wrap; }
.history li dt { width: 75px; }
.history li dd { width: calc(100% - 75px); }
```

期待: h3(165px) と dl(残り) が横並び、dl 内で dt(75px) と dd(残り) が横並び
実際: 全要素が縦積み

## 原因候補

1. flex item の `base_main_size` が子要素の explicit width を正しく反映していない
2. `calc(100% - 165px)` が flex container の main size で解決されていない
3. flex-wrap: wrap で1行に収まらない判定が誤っている

## 修正箇所

- `src/layout/flex.rs` の flex item サイズ計算
- `resolved_length` の CalcPxPercent 解決が flex main size を basis として使えているか
