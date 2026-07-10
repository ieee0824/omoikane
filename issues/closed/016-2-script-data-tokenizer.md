---
number: 016-2
slug: script-data-tokenizer
parent: 016
status: closed
---

# HTMLトークナイザに script-data / RAWTEXT / RCDATA 状態を実装

## 目的

HTML5 仕様のトークナイザ状態（script data / RAWTEXT / RCDATA）を実装し、
`<script>` / `<style>` / `<title>` 等の生テキスト系要素の内容をタグと誤認しないようにする。

## 背景（GAP_ANALYSIS.md README・セクション1）

- 既存トークナイザには Data 系状態しか無く、`<script>` 内の `<`（比較演算子や
  `'<div>'` 等の文字列リテラル）を HTML タグとして誤認していた。
- 結果、Acid3 本体JS(約173KB)が 3 断片に分割され、`var tests` と `function update` が
  別断片になって SyntaxError となり、スコア 0 の根本原因になっていた。
- 実サイト互換性にも直結する基盤機能。

## スコープ

- script data（+escaped / double-escaped states）
- RAWTEXT（`style` / `xmp` / `noembed` / `noframes`）
- RCDATA（`title` / `textarea`、文字参照は解決）
- 終了タグの正確な認識（タグ名完全一致 + 次が空白/`/`/`>`、大文字終端 `</SCRIPT>` も）
- tree_builder との連携（生テキスト系要素の内容を単一テキストノードとして構築）

## 受け入れ条件

- `<script>if (a < b) { x('</div>'); }</script>` が 1 個の script 要素・完全なテキストになる
- `</scriptx>` では終端せず、`</script foo>` / `</script  >` では終端する
- RCDATA で文字参照は解決、タグは解決しない（`<textarea><div>` の div が要素にならない）
- acid3.html をパースして script 要素数が 10 になる（従来は 14）
- 既存テストを壊さない

## 完了メモ

- 実装コミット: `d7fc211`（"016-2: トークナイザにscript-data/RAWTEXT/RCDATA状態を実装"）
- script data（+escaped / double-escaped）/ RAWTEXT / RCDATA 状態を実装し、
  tree_builder と連携。受け入れ条件を満たしたためクローズ。
