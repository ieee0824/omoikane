---
number: 024
slug: flex-flow-shorthand
parent:
status: open
---

# flex-flow shorthand 対応

## 概要

`flex-flow` shorthand を `flex-direction` + `flex-wrap` に展開する。

## CSS 仕様

```css
flex-flow: <flex-direction> || <flex-wrap>
```

- `flex-flow: row wrap` → `flex-direction: row` + `flex-wrap: wrap`
- `flex-flow: column` → `flex-direction: column` + `flex-wrap: nowrap`
- `flex-flow: wrap` → `flex-direction: row` + `flex-wrap: wrap`

## 実装場所

- `src/css/shorthand.rs`: `expand_flex_flow_shorthand` を追加

## 受け入れ条件

- `flex-flow: column` が `flex-direction: column` に展開される
- 既存テスト全通過
