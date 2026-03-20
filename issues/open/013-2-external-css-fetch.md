---
number: 013-2
slug: external-css-fetch
parent: 013-real-world-rendering
status: open
---

# 外部CSSのHTTPフェッチと適用

## 概要

`<link rel="stylesheet" href="https://...">` で参照される外部CSSファイルを
HTTP経由で取得し、カスケードに組み込む。現在は `data:` URI のみ対応済み。

## スコープ

- `<link rel="stylesheet" href="...">` の href が http/https の場合に HTTPフェッチ
- フェッチした CSS テキストを author stylesheet としてカスケードに追加
- `<link>` の `media` 属性は初期段階では無視（全メディア適用）
- 相対URLの解決（base URL の管理）
- `@import` ルールによる追加 CSS の読み込み（優先度低）

## 前提

- HTTP クライアント（`crate::http::Client`）は既に実装済み
- `extract_author_stylesheets` 内で `<link>` の `data:` URI は既に対応済み
- forgiving CSS parse も実装済み

## 実装方針

- `extract_author_stylesheets` に HTTP フェッチのパスを追加
- base URL はドキュメントの URL から決定
- フェッチ失敗時はスキップ（エラーで全体を止めない）
