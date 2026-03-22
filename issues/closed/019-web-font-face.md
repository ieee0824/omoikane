---
number: 019
slug: web-font-face
parent:
status: open
---

# @font-face Web フォント対応

## 概要

`@font-face` ルールで指定された Web フォントを HTTP フェッチし、テキスト描画に使用できるようにする。
現状はシステムフォントのみ対応しており、Web フォントを指定したサイトでフォールバックフォントになる。

## 背景

- モダン Web サイトの多くが Google Fonts 等の Web フォントを使用
- `@font-face { font-family: "MyFont"; src: url(...) format("woff2"); }` が未対応
- フォントが読み込めないとテキスト幅の計測がずれ、レイアウトも崩れる

## 想定スコープ

### Phase 1: @font-face パースと URL 抽出
- CSS パーサーで `@font-face` ルールの `font-family` と `src` を抽出
- `src: url(...)` から フォント URL を解決
- `format("woff2")` / `format("truetype")` 等のヒントをパース

### Phase 2: フォントファイルのフェッチとデコード
- HTTP でフォントファイルをダウンロード
- WOFF2 デコード（brotli 展開 → OpenType）
- WOFF デコード（zlib 展開 → OpenType）
- TTF/OTF はそのまま ab_glyph に渡す

### Phase 3: font-family マッチングとフォールバック
- `font-family: "MyFont", sans-serif` の優先順位に従いフォント選択
- Web フォントが利用可能ならそれを使い、なければシステムフォントにフォールバック
- `FontCache` に Web フォントを登録

### Phase 4: font-weight / font-style マッチング
- `@font-face` の `font-weight` / `font-style` 記述子に基づくバリアント選択
- `font-weight: bold` 指定時に bold バリアントを選択

## 技術方針

- WOFF2 デコードには `woff2-decoder` クレートまたは自前実装を検討
- フォントキャッシュは既存の `FontCache` を拡張
- フォントフェッチは既存の HTTP クライアントを使用
- CORS 制約は初期段階では無視（same-origin のみ対応でも可）

## 子 issue（必要に応じて分割）

- 019-1: @font-face パースと URL 抽出
- 019-2: WOFF2/WOFF デコード
- 019-3: font-family マッチングとフォールバック
- 019-4: font-weight / font-style バリアント選択

## 受け入れ条件

- Google Fonts 等の Web フォントを使ったページでフォントが正しく描画される
- `@font-face` で指定した Web フォントが `font-family` で参照できる
- フォールバックが正しく機能する（Web フォント取得失敗時にシステムフォントを使用）
