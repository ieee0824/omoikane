# Omoikane

Rustでできたヘッドレスブラウザライブラリです。

## 概要

Omoikane は、HTTP クライアント、HTML/CSS パーサー、DOM、レイアウト、JavaScript 実行、CDP 互換 API、C FFI を Rust で構築するプロジェクトです。

現時点では「最小構成で一通り動く」段階まで進んでおり、Rust ライブラリとしての利用に加えて、C FFI 経由で他言語から呼び出せます。

## 現在できること

- HTTP/1.1 と最小の HTTP/2 クライアント
- `gzip` 圧縮レスポンスの自動展開
- `User-Agent` の既定設定と上書き
- HTML パースと DOM 構築
- CSS パースと基本的なスタイル計算
- ブロック、インライン、Flexbox を含む最小レイアウト
- Boa ベースの JavaScript 実行
- `document` / `window` / `console` / `fetch` などの最小 Web API バインディング
- WebSocket + JSON-RPC ベースの最小 CDP サーバー
- `Page` / `DOM` / `Runtime` / `Network` / `Target` / `Input` の最小 CDP ドメイン
- C FFI

## アーキテクチャ

```text
┌─────────────────────────────────────────────────────┐
│ External Clients                                    │
│ Rust / C FFI / Go / CDP Clients                     │
└───────────────────┬─────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        │ CDP (WebSocket) / FFI │
        └───────────┬───────────┘
                    │
┌───────────────────▼─────────────────────────────────┐
│ Omoikane Engine                                     │
│                                                     │
│ HTTP -> HTML -> DOM -> CSS -> Layout -> JS -> CDP  │
└─────────────────────────────────────────────────────┘
```

主要モジュール:

- [`src/http`](/Users/ast/Documents/product/omoikane/src/http) HTTP/1.1, HTTP/2
- [`src/html`](/Users/ast/Documents/product/omoikane/src/html) HTML パーサー
- [`src/dom`](/Users/ast/Documents/product/omoikane/src/dom) DOM
- [`src/css`](/Users/ast/Documents/product/omoikane/src/css) CSS パーサーとスタイル計算
- [`src/layout`](/Users/ast/Documents/product/omoikane/src/layout) レイアウトエンジン
- [`src/js`](/Users/ast/Documents/product/omoikane/src/js) JavaScript ランタイム
- [`src/cdp`](/Users/ast/Documents/product/omoikane/src/cdp) CDP / WebSocket / JSON-RPC
- [`src/ffi`](/Users/ast/Documents/product/omoikane/src/ffi) C FFI

## クイックスタート

### Rust

```bash
cargo build
cargo test
```

最小の HTTP クライアント例:

```rust
use omoikane::http::Client;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::new();
    client.set_user_agent("MyCrawler/1.0");

    let response = client.get("https://example.com/")?;
    println!("status={}", response.status_code());
    println!("body-bytes={}", response.body().len());

    Ok(())
}
```

`Client::new()` の既定 `User-Agent` は `Omoikane/{version} {OS}` 形式です。
必要に応じて [`src/http/client.rs`](/Users/ast/Documents/product/omoikane/src/http/client.rs) の `set_user_agent` で上書きできます。

HTTP クライアントの現状仕様:

- 既定で `Accept-Encoding: gzip` を送信します
- `Content-Encoding: gzip` のレスポンスは自動で展開されます
- `Transfer-Encoding: chunked` と `gzip` の組み合わせも扱えます
- `h2` で応答ヘッダ解釈に失敗した場合は `HTTP/1.1` へフォールバックします

### C FFI

共有ライブラリをビルドすると、macOS では `target/debug/libomoikane.dylib`、Linux では `target/debug/libomoikane.so` が生成されます。

生成ヘッダは [`include/omoikane.h`](/Users/ast/Documents/product/omoikane/include/omoikane.h) です。

サンプルは [`examples/ffi`](/Users/ast/Documents/product/omoikane/examples/ffi) にあります。

## 進捗

issue ベースの開発状況では、以下の大きな実装フェーズは完了済みです。

- HTTP クライアント
- HTML パーサー
- CSS パーサーとスタイル計算
- レイアウトエンジン
- JavaScript エンジン統合
- CDP 互換 API
- C FFI

現在の open issue は [`issues/open`](/Users/ast/Documents/product/omoikane/issues/open) を参照してください。

## 制約

- 描画パイプラインとスクリーンショット出力はまだ最小段階です
- Web 標準の完全互換は目標であり、現状は必要最小限の実装です
- Puppeteer / Playwright 互換は段階的に拡張中です
- Go 向けラッパーは同梱せず、必要に応じて別リポジトリや外部パッケージとして提供する方針です

## 開発ルール

開発ルールと進め方は [`CLAUDE.md`](/Users/ast/Documents/product/omoikane/CLAUDE.md) にあります。作業前に必ず参照してください。

## ライセンス

TBD
