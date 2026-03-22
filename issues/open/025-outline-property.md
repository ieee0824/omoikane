---
number: 025
slug: outline-property
parent:
status: open
---

# outline プロパティ対応

## 概要

`outline` shorthand と longhand を実装する。出現数 939 で頻出だが、
多くのサイトが `outline: none` でリセットしており、描画への影響は限定的。

## 対象プロパティ

- `outline: none` — 最頻出、アウトラインを消す
- `outline-style: none | solid | dashed | dotted`
- `outline-width`
- `outline-color`
- `outline-offset`

## 実装方針

- 最小実装: `outline: none` を認識して is_supported_property に追加
- 描画は後回し（ほとんどのサイトが none で使用）

## 受け入れ条件

- `outline: none` が認識されて未対応CSSログに出なくなる
- 既存テスト全通過
