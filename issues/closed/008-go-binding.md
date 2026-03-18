---
number: 008
slug: go-binding
status: closed
---

# Goバインディング対応

`github.com/ieee0824/omoikane` として Go から直接利用できる形を整備する。

当初は既存の C FFI / `cdylib` を土台にした `cgo` ラッパーを同一リポジトリ内で提供する想定だったが、
配置と配布の都合から、Go 向けラッパーの同梱は取りやめた。

## ゴール

- Go 連携の検討経緯を記録する
- Go ラッパーは必要なら別リポジトリまたは外部パッケージとして提供する方針を明確化する

## 子issue

- [x] [008-1 Go cgo ラッパー実装](008-1-go-cgo-wrapper.md) — 試作後に同梱を撤回
