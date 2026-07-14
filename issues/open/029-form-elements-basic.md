---
number: 029
slug: form-elements-basic
parent:
status: open
---

# フォーム要素の基本レイアウト

## 概要

`<form>`, `<input>`, `<button>`, `<textarea>`, `<select>` の基本レイアウトを実装する。

## 背景

実サイト ページの未対応HTMLタグログ:
- `button` (10回), `form` (1回), `input` (1回)

フォーム要素はほぼ全てのWebサイトに存在し、レイアウトに影響する。

## 対応内容

### Phase 1: ブロック/インラインコンテナ
- `<form>` — ブロックコンテナ（`<div>` と同等）
- `<button>` — inline-block（テキスト内容を描画）
- `<input>` — replaced inline 要素（type に応じたデフォルトサイズ）
- `<textarea>` — replaced block 要素（cols/rows からサイズ決定）
- `<select>` — inline-block

### Phase 2: デフォルトスタイル
- ブラウザデフォルトのフォームスタイル（border, padding, background）
- `<button>` のデフォルト appearance

## 実装状況（2026-07-14）

- [x] `<form>` をブロックコンテナとしてレイアウト
- [x] `<input>` の text/submit/button/reset/hidden と `size`/`value` に基づく基本寸法
- [x] input の UA 既定背景・padding・border と値テキスト描画
- [x] block-in-inline ラッパー内の input を欠落させない収集
- [x] 大きな置換要素の baseline を行内へ収める補正
- [x] `https://www.google.co.jp/` でロゴ・検索欄・検索ボタン・フッターを実表示確認
- [ ] `<button>`、`<textarea>`、`<select>`（Issue は継続）

## 受け入れ条件

- `<form>` がブロックコンテナとしてレイアウトされる
- `<button>` が inline-block として表示される
- `<input>` が適切なデフォルトサイズで配置される
- 既存テスト全通過
