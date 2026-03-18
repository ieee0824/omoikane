---
number: 009
slug: real-world-http-header-compat
status: closed
---

# 実サイト向け HTTP ヘッダ互換性改善

ある実サイトのページ URL を `Page.navigate` で開こうとすると、現状の Omoikane は `invalid HTTP header` で失敗する。

再現確認に使った URL:

- `https://www.instagram.com/p/DWA5EZmk_g7/?utm_source=ig_web_copy_link&igsh=NTc4MTIwNjQ2YQ==`

現状では `curl` では HTML と `og:image` を取得できる一方で、Omoikane の `CdpSession::dispatch("Page.navigate", ...)` は失敗する。
この差分を埋めて、少なくとも実サイトに対して HTML と OGP メタデータを取得できる状態にする。

## ゴール

- 対象 URL への `Page.navigate` が成功する
- `DOM.getOuterHTML` で HTML を取得できる
- `og:image` を含む OGP メタタグを DOM から読める
- 失敗原因となっているレスポンスヘッダ形式を特定し、HTTP パーサーまたは HTTP/2 デコード側を修正する
- 回帰テストを追加する

## タスク

- [x] 実サイトで失敗するレスポンスの再現ケースを固定する
- [x] `invalid HTTP header` の原因となっているヘッダ行またはデコード処理を特定する
- [x] HTTP/1.1 / HTTP/2 のどちらで壊れているかを切り分ける
- [x] 必要に応じて `src/http/response.rs` または `src/http/http2.rs` を修正する
- [x] 実サイト互換性の回帰テストを追加する
- [x] `cargo test` と `cargo build` の成功を確認する

## 結果

- 実サイトでの失敗は HTTP/2 経路で発生しており、`invalid HTTP header` は HTTP/2 応答処理中に起きていた
- 互換性改善として、`h2` で `InvalidHeader` が発生した場合に `HTTP/1.1` へフォールバックするようにした
- `meta[property="og:image"]` のような属性セレクタを DOM で扱えるようにした
- `DOM.getAttributes` を追加して、HTML シリアライズに依存せず属性値を取得できるようにした
- 対象 URL に対して `Page.navigate`、`DOM.querySelector`、`DOM.getAttributes` を使って `og:image` を取得できることを確認した

## 相談

### 2026-03-18 Codex

`curl -I -L` では対象 URL のレスポンスヘッダ取得は成功しており、`og:image` も HTML 上に存在することを確認済みです。
一方で Omoikane 側は `Page.navigate` の内部で `invalid HTTP header` になっています。

まずは:

1. HTTP/2 レスポンスのヘッダ復元処理
2. `HttpResponse::parse` のヘッダ行パース
3. 改行や継続行ではなく、値に `:` や `,` を多く含む長いヘッダへの耐性

この順で切り分けるのがよさそうです。

### 2026-03-18 Codex

切り分けの結果、今回の再現条件では HTTP/1.1 パーサではなく HTTP/2 側の応答処理が失敗点でした。
根治的な HTTP/2 デコーダ拡張は今後の余地として残るものの、今回の issue では:

1. `h2` 失敗時の `HTTP/1.1` フォールバック
2. DOM 属性セレクタ対応
3. `DOM.getAttributes` 追加

まで入れることで、実サイトから OGP メタデータを取得できる状態にしました。
