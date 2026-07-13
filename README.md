# Omoikane

Rustでできたヘッドレスブラウザライブラリです。

## 概要

Omoikane は、HTTP クライアント、HTML/CSS パーサー、DOM、レイアウト、JavaScript 実行、CDP 互換 API、C FFI を Rust で構築するプロジェクトです。

現時点では「最小構成で一通り動く」段階まで進んでおり、Rust ライブラリとしての利用に加えて、C FFI 経由で他言語から呼び出せます。

## 現在できること

### レンダリングエンジン
- CSS パースとスタイル計算（カスケード、継承、em/px/mm/rem/vw/vh/vmin/vmax 等の単位変換）
- `@import` ルールによる外部 CSS の再帰的読み込み
- `@media` 条件評価（max-width / min-width / orientation / prefers-color-scheme / and / not）
- UA stylesheet デフォルト（h1〜h6 フォントサイズ・太字・margin、p、b/strong、i/em、hr）
- shorthand 完全展開（margin / padding / border-width / border-color / border-style / overflow / flex / border-radius / list-style / text-decoration）
- 高度セレクタ（`:not()` / `[attr^=]` / `[attr$=]` / `[attr*=]` / `[attr|=]`）
- ブロック、インライン、Flexbox、Grid、テーブルを含むレイアウトエンジン
- **CSS Grid レイアウト**（`display: grid` / `inline-grid`）
  - `grid-template-columns` / `grid-template-rows`（px / % / vw 等の単位、`fr`、`auto`、`min-content` / `max-content`、`calc()`、`minmax()`、`repeat(N | auto-fill | auto-fit, ...)`、名前付きラインへの耐性）
  - 明示配置とスパン（`grid-column` / `grid-row`）、自動配置、暗黙トラック生成
  - `gap` / `row-gap` / `column-gap`
  - アラインメント（`justify-content` / `align-content` / `justify-items` / `align-items` / `justify-self` / `align-self` / `place-content` / `place-items` / `place-self`）
  - 名前付きエリア（`grid-template-areas` / `grid-area` / `grid-template` ショートハンド）
- テーブルの `colspan` / `rowspan` 対応、カラム幅の intrinsic hint + 均等余剰分配
- margin collapsing（empty element、parent-child、負margin）
- float / clear / positioned element のレイアウトとペイント
- CSS `transform`（`translate` / `translateX` / `translateY` / `translate3d` / `matrix` の平行移動成分）
- CSS 2.1 Appendix E 準拠のスタッキング順序
- `:before` / `:after` 擬似要素（border triangle 含む）
- `overflow: hidden` / `overflow-x` / `overflow-y` クリッピング
- **Acid2 テスト完全通過（差分 0px）**

### CSS 色・値・視覚効果
- `rgba()` / `hsl()` / `hsla()` 色関数（アルファチャンネル対応）
- `rgb()` 現代構文（`rgb(r g b / a)` スラッシュ形式）
- 8桁 / 4桁 hex カラー（`#RRGGBBAA` / `#RGBA`）
- CSS Level 4 の 140+ named color
- `border-radius`（角丸描画、背景・ボーダー対応）
- `box-shadow`（offset / blur / spread / color / inset、複数影対応）
- `opacity`（オフスクリーンバッファ + alpha 乗算）
- `linear-gradient()`（方向指定 + 複数カラーストップ）
- `background-size`（cover / contain / length / percentage）
- `clip-path: inset()` による描画クリッピング（`-webkit-clip-path` 正規化含む。inset 以外の形状（circle/ellipse/polygon/url）はパースのみでクリップ無しフォールバック）
- CSS マスキング（`mask` / `-webkit-mask` / `mask-image` / `mask-position` / `mask-size` / `mask-repeat`）。`url()` マスク（SVG 含む）を alpha 乗算、第1レイヤのみ対応

### フォント・テキスト
- ab_glyph によるフォントファイル読み込みとグリフラスタライズ
- **`@font-face` Web フォント対応**（TTF / OTF / WOFF の HTTP フェッチ・デコード・登録）
- `font-family` 優先順位に従うフォント選択（Web フォント → システムフォント → フォールバック）
- macOS / Linux のシステムフォント自動検索
- フォントキャッシュ・グリフキャッシュ
- グリフベースのテキスト幅計測とカーニング
- CJK テキストの行折り返し・禁則処理・フォールバック
- `text-decoration`（underline / overline / line-through、per-fragment 対応）
- `text-transform`（uppercase / lowercase / capitalize）
- `letter-spacing` / `word-spacing`
- `list-style-type`（disc / circle / square / decimal / roman / alpha）
- `list-style-position`（outside / inside）

### 画像・メディア
- PNG / JPEG 画像デコード（data URI / ファイル / HTTP フェッチ）
- `<img>` の `width` / `height` 属性によるサイズ指定
- 画像の相対 URL 解決（`<base>` 要素対応）
- 画像読み込み失敗時の `alt` 属性テキスト表示
- `<frameset>` / `<frame>` の合成レンダリング（rows/cols 属性によるグリッド分割）
- ページの PNG スクリーンショット出力

### ネットワーク
- HTTP/1.1 と最小の HTTP/2 クライアント
- `gzip` 圧縮レスポンスの自動展開
- `User-Agent` の既定設定と上書き
- 外部 CSS / 画像 / Web フォントの HTTP フェッチ
- TLS 証明書検証スキップオプション（`--insecure` / `-k`）

### HTML
- HTML パースと DOM 構築
- `<meta charset>` / `Content-Type` ヘッダによる文字エンコーディング自動検出（encoding_rs）
- `<base>` 要素による URL 解決
- `<link rel="stylesheet">` による外部 CSS 読み込み
- `media` 属性による screen/print 判定
- HTML presentational hints（`bgcolor` / `text` / `align` / `width` / `height` 属性）

### JavaScript・API
- Boa ベースの JavaScript 実行
- `document` / `window` / `console` / `fetch` などの最小 Web API バインディング
- WebSocket + JSON-RPC ベースの最小 CDP サーバー
- `Page` / `DOM` / `Runtime` / `Network` / `Target` / `Input` の最小 CDP ドメイン
- C FFI（スクリーンショット API 含む）

### 開発者向け
- 未対応 CSS プロパティの観測ログ（stderr / SQLite 永続化）

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

- [`src/http`](/src/http) HTTP/1.1, HTTP/2
- [`src/html`](/src/html) HTML パーサー・文字エンコーディング検出
- [`src/dom`](/src/dom) DOM
- [`src/css`](/src/css) CSS パーサー、スタイル計算、カスケード
- [`src/layout`](/src/layout) レイアウトエンジン（block / inline / flex / table / float / positioned）
- [`src/font`](/src/font) フォント読み込み・グリフラスタライズ・キャッシュ
- [`src/paint`](/src/paint) ペイント・レンダリング（Canvas / PNG 出力）
- [`src/screenshot`](/src/screenshot) スクリーンショット合成（frameset 対応含む）
- [`src/js`](/src/js) JavaScript ランタイム
- [`src/cdp`](/src/cdp) CDP / WebSocket / JSON-RPC
- [`src/ffi`](/src/ffi) C FFI

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

スクリーンショット取得の例:

```bash
cargo run --example screenshot -- "https://example.com/" tests/output/example.png 1366 900

# 証明書エラーを無視して取得
cargo run --example screenshot -- --insecure "https://expired.example.com/" out.png
```

`Client::new()` の既定 `User-Agent` は `Omoikane/{version} {OS}` 形式です。
必要に応じて [`src/http/client.rs`](/src/http/client.rs) の `set_user_agent` で上書きできます。

HTTP クライアントの現状仕様:

- 既定で `Accept-Encoding: gzip` を送信します
- `Content-Encoding: gzip` のレスポンスは自動で展開されます
- `Transfer-Encoding: chunked` と `gzip` の組み合わせも扱えます
- `h2` で応答ヘッダ解釈に失敗した場合は `HTTP/1.1` へフォールバックします

### C FFI

共有ライブラリをビルドすると、macOS では `target/debug/libomoikane.dylib`、Linux では `target/debug/libomoikane.so` が生成されます。

生成ヘッダは [`include/omoikane.h`](/include/omoikane.h) です。

サンプルは [`examples/ffi`](/examples/ffi) にあります。

### Docker サンドボックス

ホスト環境を汚さずに omoikane のビルド・テスト・Claude Code / Codex CLI 実行ができる開発用コンテナを用意しています。CI と同一のフォント構成に加えて日本語フォント（Noto CJK / IPA ゴシック・明朝）を含み、ビルドに必要な `cmake` などに加え、開発用に `vim`・`ripgrep`（`rg`）・GitHub CLI（`gh`）・[Codex CLI](https://learn.chatgpt.com/docs/codex/cli)（`codex`）もインストール済みです。

```bash
# イメージをビルド
docker compose build

# コンテナを起動してシェルに入る
docker compose up -d
docker compose exec dev bash

# コンテナ内でビルド・テスト
cargo build
CI=1 cargo test -- --include-ignored
```

- ビルド成果物は `CARGO_TARGET_DIR=/target` に出力され、ホストの `target/` とは分離されます。
- crates.io インデックスと crate ソースは named volume（`cargo-registry` / `cargo-git`）に永続化されるため、コンテナを作り直しても（`down` / `up`）クレートを再ダウンロードしません。
- Claude Code / Codex CLI のログイン情報は named volume に永続化されるため、コンテナを作り直してもログインし直す必要はありません（初回はそれぞれ `claude` / `codex login` でログイン）。
- Codex CLI は本体パッケージも `~/.codex`（named volume）に置かれるため、更新はコンテナ内で `codex update` を実行します（イメージ再ビルドでは既存 volume 内の本体は更新されません）。
- リポジトリはバインドマウントで共有されるため、ホスト側での編集がそのままコンテナに反映されます。
- `dev` ユーザーはパスワードなしで `sudo` を使えます（コンテナ内での ad-hoc なパッケージ追加など）。
- SSH 鍵は named volume（`ssh-config`）で `~/.ssh` に永続化されます。サンドボックスの隔離を保つためホストの `~/.ssh` はマウントしない方針です。コンテナ内で `ssh-keygen -t ed25519` で生成し、公開鍵を GitHub 等に登録してください。

## Acid2 / Acid3 テスト

[Acid2 テスト](https://www.webstandards.org/files/acid2/test.html)の公式リファレンスレンダリングとの比較で**差分 0px** を達成しています。

[Acid3 テスト](http://acid3.acidtests.org/)は `cargo run --example acid3` の実測で **100/100（満点、FAITHFUL / DIRECT 両ドライブモード）** です（詳細は [`tests/fixtures/acid3/README.md`](/tests/fixtures/acid3/README.md)）。

CSS パーサー、レイアウトエンジン、ペイントシステムの統合テストとして、1121 件のテストが常時通過しています（`cargo test --lib`: 1121 passed / 0 failed、doc テスト 10 件）。

## 進捗

issue ベースの開発状況では、以下の大きな実装フェーズは完了済みです（closed issue 169 件）。

- HTTP クライアント
- HTML パーサー・文字エンコーディング検出
- CSS パーサーとスタイル計算
- UA stylesheet デフォルト
- CSS 色関数（rgba / hsl / hsla / 8桁hex / Level 4 色名）
- CSS transform（平行移動成分）
- レイアウトエンジン（Acid2 完全通過）
- テーブル colspan / rowspan 対応
- ペイント・レンダリングパイプライン
- frameset / frame 合成レンダリング
- スクリーンショット出力モジュール（`src/screenshot`）
- フォントグリフレンダリング・システムフォント検索
- CJK テキスト行折り返し・禁則処理・フォールバック
- 外部 CSS / 画像の HTTP フェッチ
- 画像サイズ属性・alt フォールバック
- JavaScript エンジン統合
- CDP 互換 API
- C FFI（スクリーンショット API）
- 未対応 CSS 観測ログ基盤
- CSS 未実装機能の段階的補完（017 シリーズ: 色関数、セレクタ、shorthand、border-radius、box-shadow、opacity、text-decoration、list-style、gradient、media query）
- 大規模ファイル分割リファクタリング（paint/layout/css モジュール）
- per-fragment inline styling（ネスト inline 要素の個別スタイル適用）
- `@font-face` Web フォント対応（TTF / OTF / WOFF / WOFF2 フェッチ・デコード）
- font-weight / font-style バリアント選択
- TLS 証明書検証スキップオプション
- CSS Grid レイアウト（061-1〜061-5: トラックサイジング、明示配置・スパン、アラインメント、トラックサイジング拡張、名前付きエリア）
- `clip-path: inset()` の描画クリッピング（062）
- CSS マスキング `mask` / `-webkit-mask` / `mask-image`（063）

現在の open issue は [`issues/open`](/issues/open) を参照してください（open issue 13 件）。

## 制約

- CSS 3 の一部機能（アニメーション、`position: sticky`）は未実装
- `clip-path` は `inset()` のみクリップを適用（circle / ellipse / polygon / url 形状はパースのみで未クリップ）
- CSS マスキングは `mask-image` の第1レイヤーのみ適用（複数レイヤー合成・luminance モードは未対応）
- WOFF2 の glyf/loca transform 逆変換は未対応（transform なしの WOFF2 は対応済み）
- Web 標準の完全互換は目標であり、現状は CSS 2.1 の主要機能を実装済みです
- Puppeteer / Playwright 互換は段階的に拡張中です
- Go 向けラッパーは同梱せず、必要に応じて別リポジトリや外部パッケージとして提供する方針です

## 開発ルール

開発ルールと進め方は [`CLAUDE.md`](/CLAUDE.md) にあります。作業前に必ず参照してください。

## ライセンス

TBD
