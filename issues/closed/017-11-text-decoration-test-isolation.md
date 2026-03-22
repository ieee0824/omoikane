---
number: 017-11
slug: text-decoration-test-isolation
parent: 017-css-feature-gap
status: open
---

# text-decoration underline テストの分離検証

## 概要

PR #30 の Copilot レビューで指摘された、text-decoration: underline のテストが
テキストグリフのピクセルと下線のピクセルを区別できていない問題を修正する。

## 背景

現在のテスト `paints_underline_for_text_decoration` は「non-transparent ピクセルが存在するか」
のみを検証しているため、underline の描画が壊れていてもテキスト自体のピクセルで通過してしまう。

## 対応方針

- underline あり/なしの2つのレンダリングを比較
- テキストグリフの縦方向範囲を特定
- グリフ帯域の外側（下側）に underline 由来のピクセルがあることを検証

## 出典

- PR #30 Copilot レビュー指摘5
