---
number: 017-7
slug: advanced-selectors
parent: 017-css-feature-gap
status: open
---

# :not() / :is() / 属性セレクタ ^= $= *= 対応

## 概要

モダン CSS で頻出するセレクタを実装する。

## 対象

### Pseudo-class 関数
- `:not(selector)` — 否定セレクタ（CSS3、最頻出）
- `:is(selector-list)` — マッチングセレクタ（CSS4）
- `:where(selector-list)` — :is() と同様だが specificity 0

### 属性セレクタ演算子
- `[attr^=value]` — 前方一致
- `[attr$=value]` — 後方一致
- `[attr*=value]` — 部分一致
- `[attr|=value]` — ハイフン区切り前方一致（lang 属性用）

### Pseudo-class 追加
- `:empty` — 子要素なし
- `:only-child`
- `:nth-of-type()` / `:nth-last-child()` / `:nth-last-of-type()`

## 実装方針

- `:not()` は引数をセレクタとしてパースし、matches の否定
- `:is()` は引数のセレクタリストのいずれかにマッチ
- 属性セレクタ演算子は `matches_attribute_selector` に分岐追加

## 受け入れ条件

- `:not(.class)` が正しくマッチ/非マッチ
- `[attr^=prefix]` 等の属性セレクタが動作
- 各セレクタのユニットテストが通過
