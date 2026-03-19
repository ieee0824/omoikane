---
number: 012-8
slug: smile-and-chin-alignment
parent: 012-acid2-official-conformance
status: open
---

# `smile` / `chin` 下半分の位置合わせ

## 概要

Acid2 下半分は `clear: both`、negative margin、`relative + absolute + nested float`、
`line-height`、fixed background が重なっており、現在も公式比較で大きめの残差分が残っている。
`.smile` の nested float 幅崩れは止められているため、次は縦位置と最終 paint を詰める段階。

## スコープ

- `.smile` の `clear: both` と周辺 margin の相互作用を正しく扱う
- `relative + absolute + nested float` 構成で口の最終位置を official reference に寄せる
- `.chin` の `line-height: 1em` と inline child の見え方を確認する
- `.chin` の `background: ... no-repeat fixed` が viewport 原点依存で不要に見えないこと
- 下半分を対象にした小さな回帰テストを追加する

## 検証観点

- `paint::tests::acid2_smile_layout_contains_positioned_and_floated_descendants`
- `paint::tests::acid2_smile_nested_float_keeps_block_width_source_descendant`
- `paint::tests::fixed_background_image_uses_viewport_origin`
- ignored の公式 Acid2 比較で下半分の差分が縮小すること

## メモ

- 以前に empty margin collapse / negative clearance を試して差分が悪化した履歴があるため、再挑戦時は局所テストを先に置く
- `chin` は fixed background の最終可視性確認も必要なので、layout だけでなく paint まで含めて見る
