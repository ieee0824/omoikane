---
number: 008
slug: go-binding
status: open
---

# Goバインディング対応

`github.com/ieee0824/omoikane` として Go から直接利用できる形を整備する。

想定する利用形態は、既存の C FFI / `cdylib` を土台にした `cgo` ラッパーを同一リポジトリ内で提供し、
Go 側から `NewBrowser`, `Navigate`, `Evaluate`, `Content`, `Close` などの高水準 API を呼び出せるようにすること。

## ゴール

- `go get github.com/ieee0824/omoikane/...` で取り込める Go パッケージ構成を用意する
- Rust 側の `cdylib` と生成ヘッダを Go から参照できるようにする
- Go からページ遷移・JavaScript評価・HTML取得の基本操作ができることを確認する
- 配布方法とビルド手順を README で案内する

## 子issue

- [ ] [008-1 Go cgo ラッパー実装](008-1-go-cgo-wrapper.md) — Go API, cgo 設定, サンプル・テスト整備
