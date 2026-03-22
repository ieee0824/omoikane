---
number: 020
slug: tls-certificate-skip-verify
parent:
status: open
---

# TLS 証明書検証スキップオプション

## 概要

期限切れ・自己署名等の TLS 証明書エラーを無視してリクエストを続行できるオプションを追加する。

## 背景

```
Error: "I/O error: invalid peer certificate: certificate expired"
```

期限切れ証明書のサイトにアクセスできず、スクリーンショットが取得できない。
ヘッドレスブラウザとしてはデバッグ用途で証明書を無視する機能が必要。

## 対応内容

### HTTP クライアント (`src/http/client.rs` / `src/http/connection.rs`)
- `Client` に `set_insecure(true)` メソッドを追加
- insecure モード時は rustls の `ServerCertVerifier` をカスタム実装し、全証明書を受理
- `dangerous_tls_config()` で `DangerousClientConfig` を使用

### CDP セッション (`src/cdp/mod.rs`)
- `CdpSession` に insecure オプションを伝播

### FFI (`src/ffi/mod.rs`)
- `omoikane_set_insecure(browser, bool)` を追加

### screenshot サンプル (`examples/screenshot.rs`)
- `--insecure` / `-k` フラグを追加

### HTTP/2 (`src/http/http2.rs`)
- 同様に insecure モード対応

## 受け入れ条件

- `cargo run --example screenshot -- --insecure "https://expired.example.com/" out.png` で期限切れ証明書のサイトにアクセスできる
- デフォルトでは従来通り証明書検証を行う（セキュリティ維持）
