---
number: 016-7
slug: document-write
parent: 016
status: open
---

# document.write 実装

## 目的

`document.write` を実装する。Acid3 の body 内スクリプトが
`document.write('<map>…<iframe id="selectors">…')` で selectors iframe 等を生成する。

## 背景（GAP_ANALYSIS.md セクション1 P8、セクション3 領域E）

- body 末尾で `document.write(...)` により map / area / iframe(#selectors) / form / table を生成する。
- これらは `getTestDocument()` 依存の約 35 テストの前提の一つ。
- 現状 `document.write` は未実装。

## スコープ

- `document.write` / （必要に応じて `document.open` / `document.close`）
- 書き込まれた HTML 断片のトークナイズ・ツリー挿入（同期）
- 生成される map / area / iframe / form / table 等の要素化

## 受け入れ条件

- `document.write('<iframe id="selectors">…')` で selectors iframe 要素が生成される
- 生成要素が `getElementById` 等で参照できる
- 016-9（iframe/contentDocument）と併せて getTestDocument が機能する前提を満たす
