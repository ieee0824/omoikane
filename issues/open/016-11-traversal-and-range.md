---
number: 016-11
slug: traversal-and-range
parent: 016
status: open
---

# NodeIterator / TreeWalker / Range 実装

## 目的

DOM Traversal（NodeIterator / TreeWalker）と DOM Range を実装する。bucket1、最大 12 点。

## 背景（GAP_ANALYSIS.md セクション3 領域F/G）

- NodeIterator / TreeWalker（領域F）: test 1〜6（6 点）
- Range（領域G）: test 7,8,9,11,12,13（6 点、10 は素通し）
- 純粋な DOM 木操作だが、多くが iframe/contentDocument（016-9）前提。

## スコープ

- `createNodeIterator` / `createTreeWalker` + NodeFilter + whatToShow ビットマスク +
  例外転送 + 反復中のノード削除への追従（live）
- `createRange` + 境界点 / collapse / cloneContents / extractContents / insertNode /
  surroundContents / toString + 削除時の境界点補正

## 受け入れ条件

- NodeIterator/TreeWalker が whatToShow とフィルタに従って走査する
- Range の各操作が仕様どおり動く
- Acid3 test 1〜13（10 除く）の前提を満たす
