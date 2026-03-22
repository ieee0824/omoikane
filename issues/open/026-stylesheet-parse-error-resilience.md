---
number: 026
slug: stylesheet-parse-error-resilience
parent:
status: open
---

# スタイルシートパースエラーの耐性改善

## 概要

外部 CSS のパースが1つでも失敗すると `InvalidStylesheet` エラーで
レンダリング全体が中断する問題を修正する。

## 背景

```
$ cargo run --example screenshot -- "https://qiita.com/..." tests/output/qiita.png
Error: "failed to render screenshot: InvalidStylesheet"
```

Qiita 等のモダンサイトは複数の外部 CSS を読み込むが、
`parse_stylesheet_forgiving` が1つの CSS で全ルールをパースできない場合に
`InvalidStylesheet` エラーを返し、`render_document_with_url` の `?` で即中断する。

## 対応内容

- `render_document_with_url` で `parse_stylesheet_forgiving` のエラーを無視して続行
- パースに失敗した CSS はスキップし、成功したものだけ適用
- `parse_stylesheet_forgiving` がルール0件の場合も空の Stylesheet を返す（エラーにしない）

## 受け入れ条件

- CSS パースが失敗してもレンダリングが中断しない
- パース可能な CSS は正しく適用される
- 既存テスト全通過
