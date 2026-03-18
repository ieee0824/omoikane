---
number: 008-1
slug: go-cgo-wrapper
status: closed
parent: 008
---

# Go cgo ラッパー実装

既存の C FFI を利用して、Go から `github.com/ieee0824/omoikane` 経由でブラウザ機能を呼び出せるようにする。

## タスク

- [x] `bindings/go/` もしくは `go/` 配下に Go パッケージを新設
- [x] `go.mod` を追加し、モジュールパスを `github.com/ieee0824/omoikane/...` に合わせる
- [x] `cgo` で `include/omoikane.h` と `cdylib` を参照するビルド設定を追加
- [x] `NewBrowser`, `Navigate`, `Evaluate`, `Content`, `Close` などの Go API を設計
- [x] Rust 側のエラー文字列を Go の `error` として扱えるようにする
- [x] 最小の Go サンプルを追加
- [x] 可能なら `go test` による smoke test を追加
- [x] README に Go からの利用手順を追記

## 相談

### 2026-03-18 Codex

Go パッケージの配置は `bindings/go/` に切り出す案と、リポジトリ直下に `go/` を置く案があります。
モジュールパスを `github.com/ieee0824/omoikane` に自然に揃えるなら、どこを import ルートにするかを実装前に決めたいです。
