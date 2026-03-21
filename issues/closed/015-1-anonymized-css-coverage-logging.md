---
number: 015-1
slug: anonymized-css-coverage-logging
parent: 015-anonymized-real-world-rendering-gap
status: closed
---

# 未対応CSSプロパティ観測ログ

## 概要

匿名化された実ページ群で「どの未対応CSSが描画差分の主因か」を特定するため、
CSS解釈時に未対応プロパティ/値を収集する診断ログを追加する。

## スコープ

- 未対応プロパティ名の集計（出現回数）
- 未対応値パターンの集計（代表値のみ）
- ログ出力のON/OFF切り替え（通常実行では無効）
- テストでログ経路が壊れていないことを確認

## 受け入れ条件

- 匿名化ページを1回レンダリングして、優先実装候補トップNを出力できる
- サイト名/URLを含む情報がログに残らない

## 実施結果

- 未対応CSSログのSQLite集計基盤を活用し、`OMOIKANE_UNSUPPORTED_CSS_TOP_N` で上位N件を出力する経路を追加
- `OMOIKANE_LOG_UNSUPPORTED_CSS_TOP_N=1` でも既定上位件数（20件）を出力できるよう対応
- 未対応値ログに URL 匿名化を追加（`http(s)://`, `ws(s)://`, `ftp://`, `data:` を `[redacted-url]` に置換）
- top-N 集計の順序検証テストと匿名化テストを追加

## 検証

- `cargo test css::style::` 通過
