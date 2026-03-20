---
number: 013-6
slug: css-import-rule
parent: 013-real-world-rendering
status: closed
---

# @import CSSルール対応

## 概要

CSS内の `@import` ルールで参照される追加CSSファイルを読み込み、カスケードに組み込む。

## スコープ

- `@import url("style.css")` / `@import "style.css"` の解析
- 相対URLの解決（親CSSのURLを基準）
- フェッチしたCSSをカスケードに追加（@import の出現順序を維持）
- 再帰的な @import の処理（深さ制限付き）
- SSRF対策（同一オリジンのみ）

## スコープ外（初期）

- `@import` のメディアクエリ条件（`@import url("print.css") print;`）
- `@import` の supports 条件

## 技術方針

- CSSパーサーで @import ルールを抽出
- 再帰深さは5程度に制限（無限ループ防止）
- フェッチ失敗時はスキップ
