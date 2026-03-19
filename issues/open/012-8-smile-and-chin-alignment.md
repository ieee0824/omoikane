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

## 修正方法

- `smile/chin` だけを直接いじる前に、`nose -> empty -> smile -> chin` の主要 box の `y` 位置と使用高さを layout test で固定し、どの区間が実際に伸びているかを先に確定する
- `clear: both`、negative margin、relative offset は一度に触らず、1 つずつ局所テストで差分を見ながら進める
- `.smile div` の relative offset とその内側 absolute box の paint phase が、祖先 stacking の変更で副作用を受けていないかを毎回確認する
- `.chin` は layout だけでなく paint まで見て、`line-height: 1em` と `fixed background` が最終可視結果を押し広げていないかを切り分ける
- `empty margin collapse` や `negative clearance` は過去に悪化実績があるため、再挑戦する場合も必ず先に狭い regression を追加する

## 検証方法

- `cargo test acid2_smile_layout_contains_positioned_and_floated_descendants --lib`
- `cargo test acid2_smile_nested_float_keeps_block_width_source_descendant --lib`
- `cargo test acid2_lower_face_boxes_keep_expected_vertical_order --lib`
- `cargo test acid2_empty_block_starts_shortly_after_nose --lib`
- `cargo test acid2_empty_block_creates_large_gap_before_smile --lib`
- `cargo test fixed_background_image_uses_viewport_origin --lib`
- `cargo test paint::tests::acid2_fixture_matches_official_reference_rendering --lib -- --ignored`
- diff 画像 `tests/output/acid2/acid2.official-reference.{actual,diff}.png` を見て、口元から顎にかけての縦方向の magenta が減っていることを確認する

## 検証観点

- `paint::tests::acid2_smile_layout_contains_positioned_and_floated_descendants`
- `paint::tests::acid2_smile_nested_float_keeps_block_width_source_descendant`
- `paint::tests::acid2_lower_face_boxes_keep_expected_vertical_order`
- `paint::tests::acid2_empty_block_starts_shortly_after_nose`
- `paint::tests::acid2_empty_block_creates_large_gap_before_smile`
- `paint::tests::fixed_background_image_uses_viewport_origin`
- ignored の公式 Acid2 比較で下半分の差分が縮小すること

## メモ

- 以前に empty margin collapse / negative clearance を試して差分が悪化した履歴があるため、再挑戦時は局所テストを先に置く
- `chin` は fixed background の最終可視性確認も必要なので、layout だけでなく paint まで含めて見る
- `nose -> empty` と `empty -> smile` の gap は現状の局所テストでは大きく崩れていないため、下半分の大きな見た目差分は単純な block 間隔よりも paint phase / stacking / さらに上流の縦配置にある可能性が高い
