---
number: 016-5
slug: data-uri-scripts
parent: 016
status: open
---

# data: URI スクリプト対応

## 目的

`<script src="data:...">` をサポートし、`data:` スキームのスクリプトソースを取得できるようにする。

## 背景（GAP_ANALYSIS.md test 97、セクション3 領域N）

- `fetch_script_source`（`src/js/mod.rs`）が `data:` スキーム非対応のため、
  Acid3 の d1〜d5（`<script src="data:text/javascript,...">`）が全て fetch 失敗する。
- 実測でも `[0]`〜`[4]` の "failed to fetch script: data:..." が残っている。
- Acid3 の d1〜d5 が実テストベクタであり、test 97 に直結する。

## スコープ

- `data:` スキームのパース（`fetch_script_source`）
  - percent-decoding（`d1`, `d5`）
  - base64（`d2`, `d3`）
  - base64 + 空白/改行混入の許容（`d4`）
- 併せて画像等の data: 取得も視野に入れる（本issueの主眼はスクリプト）

## 受け入れ条件

- d1〜d5 の 5 パターンが正しくデコード・実行される
- Acid3 test 97 が data: 系で fail しなくなる
