---
number: 047
slug: inline-style-cascade
parent:
status: open
---

# インライン style 属性のカスケード・レイアウト適用

## 概要

`style="..."` 属性をカスケード（specificity 最高位の宣言）としてスタイル計算・レイアウト・描画に適用する。

## 背景（PR #105 レビューでの発見）

- エンジンのカスケード/レイアウトは現状 `<style>`/`<link>` ルールと presentational HTML 属性のみを適用し、
  **インライン style 属性を一切見ていない**
- 一方 016-8 の getComputedStyle は JS 側 `__parseInlineStyle` でインライン値をカスケード結果に上書きマージするため、
  「getComputedStyle だけがインラインを反映する」観測可能な不整合が生じている
  - 例: `<div style="width:100px">` で `getComputedStyle(el).width === "100px"` だが `el.offsetWidth` はインライン無視
- さらに `__parseInlineStyle` は `;` の素朴 split のため `url(data:...;base64,...)` を含む値を誤パースする
- インライン値は生文字列（`"blue"` / `"1em"`）のまま返り、px/rgb への computed value 解決もされない

## 対応内容

- style 属性のパースを Rust 側カスケードに統合（author style より高い specificity、!important 対応）
- レイアウト・描画がインラインスタイルを反映する
- getComputedStyle は JS 側マージを廃止し、カスケード結果（解決済み computed value）を一元的に返す
- `;` split の誤パース解消（カスケード統合により JS 側パーサ自体を削除できる想定）

## 優先度

中〜高 — 実サイトでのインライン style 使用率は極めて高く、レンダリング品質に直結する。

## 受け入れ条件

- インライン style がレイアウト結果（offsetWidth 等）と getComputedStyle の両方に一貫して反映される
- `url(data:...;base64,...)` を含む style 属性が正しくパースされるテストを追加
