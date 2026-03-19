---
number: 012-6
slug: eyes-inline-fallback-and-fixed-background
parent: 012-acid2-official-conformance
status: closed
---

# `#eyes-a object` の inline fallback / fixed background 調整

## 概要

Acid2 の `#eyes-a` は nested `object` fallback の最内周で PNG を inline image として描きつつ、
`vertical-align: bottom` / `text-align: right` / `background: ... fixed 1px 0` を同時に満たす必要がある。
現在は個別の回帰テストは通るが、公式比較ではまだ目が横長の赤帯として崩れている。

## スコープ

- fallback 後の replaced content が inline のまま line box に参加すること
- `#eyes-a object[type] { width/height }` が Acid2 コメントどおり最終 inline fallback に効かないこと
- `vertical-align: bottom` と `text-align: right` が目画像の最終位置に反映されること
- inline image fragment の padding / border / background を含む外形が正しく描かれること
- `background-attachment: fixed` と `background-position: 1px 0` が inline fragment 上でも viewport 基準で効くこと

## 検証観点

- `paint::tests::acid2_eyes_inline_layer_stays_at_same_origin_as_float_and_block_layers`
- `paint::tests::fixed_background_image_uses_viewport_origin`
- `paint::tests::inline_replaced_element_with_padding_border_and_background_paints_in_order`
- ignored の公式 Acid2 比較で目の横長赤帯が縮小すること

## メモ

- 既に object fallback chain 自体は解決できている
- 問題は「inline fallback の最終使用幅」と「fixed background の最終 paint origin」が混ざる地点に残っている可能性が高い
- `layout::tests::object_type_width_and_height_do_not_change_nested_inline_fallback_image_size` を追加し、`object[type]` の `width/height` が nested inline fallback の最終 image fragment 幅に効かないことを固定した
- `paint::tests::nested_object_fallback_preserves_fixed_background_on_inline_image_fragment` を追加し、nested object fallback 上でも innermost decoration style の fixed background が実際に paint されることを確認した
- `cargo test --lib` は通過、ignored の公式 Acid2 比較差分はこの時点では `40,427` のまま
