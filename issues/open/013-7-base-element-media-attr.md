---
number: 013-7
slug: base-element-media-attr
parent: 013-real-world-rendering
status: open
---

# base要素とmedia属性対応

## 概要

`<base>` 要素によるbase URL上書きと、`<link>` の `media` 属性によるメディアクエリ判定を実装する。

## スコープ

### base要素
- `<base href="...">` でドキュメントのbase URLを上書き
- 相対URL解決時にbase URLを使用
- 複数の `<base>` がある場合は最初のものを採用

### media属性
- `<link rel="stylesheet" media="screen">` のメディアクエリ判定
- `media="all"` / `media=""` / media属性なし → 常に適用
- `media="print"` → スクリーンレンダリングでは適用しない
- 基本的なメディアタイプ（screen, print, all）のみ初期対応

## スコープ外（初期）

- メディアクエリの条件式（`@media (max-width: 800px)`）
- `<style>` の media 属性
