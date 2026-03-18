---
number: 010
slug: gzip-response-support
status: closed
---

# gzip 圧縮レスポンス対応

現状の HTTP クライアントは `Content-Encoding: gzip` なレスポンスを自動展開しない。
そのため、実サイトで配信される HTML や API レスポンスを正しく読めないケースがある。

## ゴール

- リクエストで `Accept-Encoding: gzip` を送れる
- `Content-Encoding: gzip` なレスポンスを自動展開できる
- 展開後の body を既存 API (`HttpResponse::body`) から透過的に読める
- `Content-Encoding` が無い通常レスポンスは従来どおり動く
- 回帰テストを追加する

## タスク

- [x] gzip 対応の実装方針を決める
- [x] 必要なら `Accept-Encoding: gzip` を既定で付与する
- [x] `Content-Encoding: gzip` のレスポンス body を展開する
- [x] `chunked` と gzip の組み合わせも考慮する
- [x] 通常レスポンスと gzip レスポンスの回帰テストを追加する
- [x] `cargo test` と `cargo build` の成功を確認する

## 結果

- `HttpRequest::new` で `Accept-Encoding: gzip` を既定送信するようにした
- `HttpResponse::parse` で body 読み込み後に `Content-Encoding: gzip` を見て自動展開するようにした
- `Transfer-Encoding: chunked` と `Content-Encoding: gzip` の組み合わせも通るようにした
- gzip 展開は `flate2` を使って pure Rust で実装した
- 通常レスポンス、gzip レスポンス、chunked + gzip レスポンスの回帰テストを追加した

## 相談

### 2026-03-18 Codex

実装位置としては:

1. `src/http/request.rs` で `Accept-Encoding` を送る
2. `src/http/response.rs` の body 読み出し後に `Content-Encoding` を見て展開する

この順が自然そうです。

圧縮はインフラ層なので、プロジェクト方針上も pure Rust クレート利用で問題ない想定です。
