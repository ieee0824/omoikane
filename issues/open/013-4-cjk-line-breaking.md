---
number: 013-4
slug: cjk-line-breaking
parent: 013-real-world-rendering
status: open
---

# CJKテキストの行折り返し・禁則処理

## 概要

日本語テキストの行折り返しを正しく処理する。現在は ASCII スペースでのみ
折り返しが発生し、CJK 文字列は1つの不可分な単語として扱われる。

## スコープ

- CJK 文字間での折り返し許可（Unicode の Line Break Algorithm 簡易版）
- 禁則処理: 行頭禁則（句読点、閉じ括弧等が行頭に来ない）
- 禁則処理: 行末禁則（開き括弧等が行末に来ない）
- `word-break: break-all` の対応
- `overflow-wrap: break-word` の対応

## スコープ外（初期）

- Unicode Bidirectional Algorithm（RTL テキスト）
- `text-align: justify` の均等割り付け
- ルビ（`<ruby>`）
- 縦書き（`writing-mode: vertical-rl`）

## 参考

- UAX #14: Unicode Line Breaking Algorithm
- CSS Text Module Level 3 §5 Line Breaking
