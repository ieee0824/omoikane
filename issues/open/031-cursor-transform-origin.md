---
number: 031
slug: cursor-transform-origin
parent:
status: open
---

# cursor / transform-origin の基本対応

## 概要

実サイトで頻出する `cursor` と `transform-origin` プロパティを
is_supported_property に追加し、未対応ログのノイズを削減する。

## 背景

実サイト ページの未対応CSSログ:
- `transform-origin` (36回)
- `cursor` (18回)

ヘッドレスブラウザでは `cursor` は視覚的影響なし。
`transform-origin` は `transform` 未実装のため当面影響なし。

## 対応内容

- `cursor` を is_supported_property に追加（レンダリング影響なし、ログ抑制のみ）
- `transform-origin` を is_supported_property に追加（transform 実装時に再検討）
- `animation-*` 系を is_supported_property に追加（ヘッドレスでは静的スナップショットのみ）

## 受け入れ条件

- 上記プロパティが未対応ログに出なくなる
- 既存テスト全通過

## 関連 issue

- [051 CSS プロパティ値検証と computed style serialization](051-css-property-value-validation.md)
  - 本 issue は supported property 登録まで、051 は `cursor` keyword の妥当性検証・無効宣言破棄・初期値 `auto` の serialization を担当する
