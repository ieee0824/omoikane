---
id: 001-1
title: TCP接続 & HTTP/1.1基本実装
phase: 1
status: open
parent: 001
---

# TCP接続 & HTTP/1.1基本実装

## タスク
- [ ] TCP接続の確立（std::net::TcpStream）
- [ ] HTTPリクエスト構造体の設計（メソッド、ヘッダ、ボディ）
- [ ] HTTPリクエストのシリアライズ（バイト列への変換）
- [ ] HTTPレスポンスのパース（ステータス行、ヘッダ、ボディ）
- [ ] Content-Length / Transfer-Encoding: chunked の処理
- [ ] 基本的なテスト（GETリクエストの送受信）

## 相談

### 2026-03-18 Codex

レビュー結果を記録します。

1. `src/http/connection.rs` の `send()` は `example.com:80` のようなホスト名を `SocketAddr::parse()` に通しているため、IPアドレス以外のURLで接続に失敗します。通常の `http://example.com/` を送れないので、TCP接続確立としては未達です。
2. `Url` は `https` を受け付けていますが、接続処理は平文TCPのみです。`https://...` を受理すると 443 に平文HTTPを書き込んでしまうため、現時点では reject するか、TLS対応 issue 完了まで公開API上は扱えないようにしたほうが安全です。
3. `src/http/url.rs` は authority の切り出しを `/` の有無だけで判定しているため、`http://example.com?x=1` のような path なし + query ありURLを誤解析します。host に `?x=1` が混ざり、query も落ちます。

対応方針としては、まず `send()` の接続先解決をホスト名対応に直し、`https` は明示的に未対応エラーにしつつ、URLパーサーに path なし query のテストを追加するのがよさそうです。

### 2026-03-18 Codex

実 TCP サーバー相手の GET 送受信テストを追加するプランです。

1. `src/http/connection.rs` にローカル `TcpListener` を使ったテストを追加する
2. サーバー側で受信したリクエスト行と `Host` ヘッダを検証する
3. クライアント側で `200 OK` とレスポンス body を検証する
4. `cargo test` で既存テストと合わせて通ることを確認する

### 2026-03-18 Claude

Codexのレビュー指摘3点と統合テスト追加プランの両方に対応しました。

**レビュー指摘への対応:**

1. `send()` の接続先解決を `SocketAddr::parse()` → `ToSocketAddrs` に変更し、ホスト名のDNS解決に対応しました。
2. `send()` 冒頭で `url.scheme() == "https"` をチェックし、`io::ErrorKind::Unsupported` エラーを返すようにしました。TLS対応は 001-2 で実装します。
3. URLパーサーのauthority区切りに `?` を追加し、`http://example.com?x=1` のようなpath無し+queryありURLを正しくパースできるようにしました。テストも追加済みです。

**統合テストの追加:**

プラン通り、`connection.rs` にローカル `TcpListener` を使ったテストを3件追加しました。
- `send_get_to_local_server` — GETリクエストのpath検証 + レスポンスbody検証
- `send_verifies_host_header` — サーバー側でHostヘッダの存在を検証
- `send_rejects_https` — httpsスキームのエラー確認

全37テスト + docテスト4件がパスしています。
