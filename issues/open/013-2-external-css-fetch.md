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

## 実装済み

### URL解決ユーティリティ (`src/http/url.rs`)
- `resolve_url(base: &Url, reference: &str) -> Result<Url, UrlParseError>` を追加
- 絶対URL、プロトコル相対(`//`)、絶対パス(`/path`)、相対パス(`../path`)に対応
- RFC 3986 §5.2.4 準拠の `.` / `..` 正規化
- `http::url` モジュールを `pub` に変更して外部公開

### 外部CSSフェッチ (`src/paint/mod.rs`)
- `extract_author_stylesheets` に `base_url: Option<&Url>` 引数を追加
- `collect_author_stylesheets` が HTTP/HTTPS の `<link>` href を検出した場合、`resolve_url` でURL解決後 `http::Client` でGETフェッチ
- ステータス200 + UTF-8のレスポンスのみ採用、それ以外はスキップ
- `base_url=None` の場合はHTTP hrefをスキップ（data: URIのみ処理）

### 新規公開API
- `render_document_with_url(document, viewport, base_url)` — base URL指定でレンダリング
- `render_document_png_with_url(document, viewport, base_url)` — 同上のPNG出力版
- 既存の `render_document` / `render_document_png` は後方互換維持

### テスト
- `resolve_url` のユニットテスト6件（絶対URL、プロトコル相対、絶対パス、相対パス、親相対、クエリ付き）
- `extract_author_stylesheets` のテスト3件（HTTP linkスキップ、data URIとの混在、空href）

## 未対応（スコープ外）

- `@import` ルールによる追加CSSの読み込み
- `<link>` の `media` 属性によるメディアクエリ判定
- `<base>` 要素によるbase URL上書き
