---
number: 044-2
slug: layout-metrics-bindings
status: open
parent: 044
---

# レイアウトメトリクス（Rust連携）

## 概要

JS からレイアウト結果を取得する API を Rust ネイティブバインディングとして実装する。

## 追加 API

| API | 用途 |
|-----|------|
| `getBoundingClientRect()` | 要素の位置・サイズ取得 |
| `offsetWidth` / `offsetHeight` | レイアウト幅・高さ |
| `offsetTop` / `offsetLeft` | 親からのオフセット |
| `clientWidth` / `clientHeight` | padding 含むサイズ |
| `scrollWidth` / `scrollHeight` / `scrollTop` / `scrollLeft` | スクロール情報 |
| `getComputedStyle()` 実値返却 | 計算済みスタイル取得 |

## 技術的課題

- JS 実行時点ではレイアウトが未完了の可能性がある
- 同期的にレイアウト結果を返す必要がある（forced reflow）
- HostState にレイアウトツリーへの参照を持たせる設計が必要
