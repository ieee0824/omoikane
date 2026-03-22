---
number: 023
slug: word-break-overflow-wrap
parent:
status: open
---

# word-break / overflow-wrap 対応

## 概要

`word-break` と `overflow-wrap` (旧 `word-wrap`) を実装する。
日本語サイトで頻出（出現数 231）で、テキストの折り返し制御に必須。

## 対象プロパティ

- `word-break: normal | break-all | keep-all | break-word`
  - `break-all`: 任意の文字間で折り返し（CJK以外でも）
  - `keep-all`: CJK テキストでも単語単位で折り返し（禁則処理優先）
- `overflow-wrap: normal | break-word | anywhere`
  - `break-word`: 行に収まらない長い単語を途中で折り返し
- `word-wrap`: `overflow-wrap` のレガシーエイリアス

## 実装場所

- `src/css/style.rs`: `is_supported_property` に追加、継承プロパティに追加
- `src/layout/inline.rs`: テキスト分割ロジックに `word-break` / `overflow-wrap` を反映

## 受け入れ条件

- `word-break: break-all` で任意の文字間で折り返し可能
- `overflow-wrap: break-word` で長い単語が折り返される
- 既存テスト全通過
