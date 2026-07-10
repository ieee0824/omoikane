---
number: 016-9
slug: iframe-content-document
parent: 016
status: open
---

# iframe / contentDocument サブブラウジングコンテキスト

## 目的

iframe 要素にサブブラウジングコンテキスト（独立 document）を持たせ、
`contentDocument` でアクセスできるようにする。

## 背景（GAP_ANALYSIS.md セクション1 P7、セクション3 領域E）

- 現状 iframe 要素はパースされるが、サブドキュメント / contentDocument の概念が無い。
- `getTestDocument()` = `document.getElementById("selectors").contentDocument` であり、
  test 1,2,3,6,9,11-13,14,15,33-44,46,48,65,69-71,74,80 等 約 35 テストの門。
- 実測でも test 14/15 が "no <iframe> support"、test 71/72 が "missing document for test" で失敗。

## スコープ

- iframe の独立 document 生成（空 HTML / src ロード）
- `contentDocument` / `contentWindow` アクセサ
- MIME 判定（image/png や text/plain を HTML としてパースしない: test 14/15）
- onload イベント（016-4 と連携）

## 受け入れ条件

- `iframe.contentDocument` が独立した document を返す
- 空 iframe に対する DOM 操作が親と分離して動く
- getTestDocument 依存テストの前提が満たされる
