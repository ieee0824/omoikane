---
number: 017-16
slug: media-query-parse-cache
parent: 017-css-feature-gap
status: open
---

# @media クエリのパース結果キャッシュ

## 概要

media_query_matches() がノードごとにメディアクエリ文字列を再パースしており、
大きな DOM + 多数の @media ルールでパフォーマンスが低下する。

## 対応方針

- @media プリリュードをスタイルシート解析時に1回パースし、AST を AtRule に保持
- または StyleResolver にパース結果を prelude 文字列でメモ化

## 出典

- PR #36 Copilot レビュー指摘7
