---
number: 012-1
slug: link-stylesheet-loading
parent: 012-acid2-official-conformance
status: closed
---

# `<link rel="stylesheet">` による外部スタイルシート適用

## 概要

`<link rel="stylesheet" href="...">` で参照される外部CSSを読み込み、カスケードに組み込む。
現状は `<style>` 要素内のインラインCSSのみ対応しており、`<link>` 経由のスタイルシートは無視されている。

## 仕様参照

- CSS 2.1 §6.4 The cascade
- HTML 4.01 §14.3.2 Specifying external style sheets

## スコープ

- `<link rel="stylesheet">` の `href` 属性から CSS テキストを取得する
- `data:` URI スキームへの対応（`data:text/css,...` 形式）
- percent-encoding のデコード
- 取得した CSS をカスケードに author stylesheet として追加する
- `rel="appendix stylesheet"` 等の複合 rel 値でも stylesheet として認識する

## スコープ外

- HTTP/HTTPS による外部リソース取得（将来の HTTP クライアント統合時に対応）
- `@import` ルール
- `media` 属性によるメディアクエリフィルタリング

## 結果

- 調査の結果、`collect_author_stylesheets` 内で `<link rel="stylesheet">` の `data:text/css` URI は既に対応済みだった
- `rel.contains("stylesheet")` で複合 rel 値にも対応、`parse_data_uri` で percent-encoding デコードも動作
- `paint::tests::acid2_link_stylesheet_overrides_picture_background_to_none` テストで `.picture { background: none; }` が正しくカスケードに乗ることを確認し close
