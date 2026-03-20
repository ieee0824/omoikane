---
number: 012-5
slug: min-max-height-override
parent: 012-acid2-official-conformance
status: closed
---

# `min-height` が `max-height` を override する処理

## 概要

`min-height` の値が `max-height` より大きい場合、`min-height` が優先される仕様を正しく実装する。
`min-width` / `max-width` についても同様。

## 仕様参照

- CSS 2.1 §10.7: If the computed `min-height` is greater than the computed `max-height`, `max-height` is set to the value of `min-height`.
- CSS 2.1 §10.4: 同様に `min-width` > `max-width` の場合

## スコープ

- layout 時の height 計算で `min-height` > `max-height` なら `min-height` を採用する
- layout 時の width 計算で `min-width` > `max-width` なら `min-width` を採用する
- 各種単位（px, em, mm 等）の比較を正しく行う

## 検証観点

- `height: 8px; min-height: 1em; max-height: 2mm;` のようなケースで `min-height: 1em` (= 12px) が勝つこと
- `min-height` が percentage で containing block 高さが未確定の場合は auto 扱い（既存処理との整合性）

## 完了メモ

- layout の width / height clamp に `min > max` 正規化を共通化して導入した
- `layout::tests::min_height_overrides_smaller_max_height` を追加し、height 側の override を固定した
- `layout::tests::min_width_overrides_smaller_max_width` を追加し、width 側の override を固定した
