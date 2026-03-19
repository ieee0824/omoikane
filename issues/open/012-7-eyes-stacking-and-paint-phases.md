---
number: 012-7
slug: eyes-stacking-and-paint-phases
parent: 012-acid2-official-conformance
status: open
---

# `.eyes` の stacking / paint phase 整合

## 概要

Acid2 の `.eyes` は `#eyes-c` の block、`#eyes-b` の float、`#eyes-a` の inline content が
同じ帯域で重なる Appendix E 寄りの paint order テストになっている。
現在は block / float / inline の大枠は入っているが、公式比較では layering がまだ崩れている。

## スコープ

- `#eyes-c` が通常 flow block として最下層に残ること
- `#eyes-b` が float としてその上に乗ること
- `#eyes-a` が inline content を含む `height: 0` box として最上層に乗ること
- descendant 順ではなく paint phase 順で `.eyes` 配下を描けること
- `positioned + float + inline` の混在ケースを小さな回帰テストで固定すること

## 検証観点

- `paint::tests::acid2_eyes_block_layer_stays_overlapping_float_layer`
- `paint::tests::absolute_inline_content_paints_above_float_siblings`
- `paint::tests::positioned_grandchild_paints_above_float_uncle`
- `.eyes` 専用の追加回帰テスト
- ignored の公式 Acid2 比較で目周りの差分が縮小すること

## メモ

- Appendix E 全体の完全実装を先に目指すより、`.eyes` に必要な phase ordering を先に狭く固定する
- `#eyes-a` 自体の inline/fixed background 問題は [012-6](012-6-eyes-inline-fallback-and-fixed-background.md) で扱う
- immediate child だけでなく、non-positioned subtree の中にある positioned descendant も祖先 stacking phase へ defer する必要がある
- 上の defer を paint に入れたことで、`positioned_grandchild_paints_above_float_uncle` を含む局所回帰は維持しつつ、ignored の公式比較差分は `40,427 -> 36,708` まで改善した
