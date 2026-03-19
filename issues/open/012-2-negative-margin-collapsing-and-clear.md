---
number: 012-2
slug: negative-margin-collapsing-and-clear
parent: 012-acid2-official-conformance
status: open
---

# 負margin collapsingと clear の負clearance

## 概要

隣接ブロックの負margin同士の collapsing、empty element の自己 margin collapsing、
および `clear` プロパティの clearance が負になるケースを正しく処理する。

## 仕様参照

- CSS 2.1 §8.3.1 Collapsing margins
- CSS 2.1 §9.5.1 Positioning the float (clearance)
- CSS 2.1 §9.5.2 Controlling flow next to floats

## スコープ

### 負margin collapsing
- 隣接する正margin + 負margin → 正の最大値 + 負の最小値（絶対値最大）を合算
- 隣接する負margin同士 → 絶対値が大きい方を採用
- 現在の正margin同士の collapse（max）に加え、上記ルールを実装

### Empty element の margin collapsing
- `height: auto` かつ中身なし（子・テキスト・line box なし）の要素は、自身の上下 margin が collapse する
- 結果として要素が高さ 0 になり、前後の margin collapsing chain に参加する

### Clear の負 clearance
- `clear: both/left/right` で float をクリアする際、clearance が負になるケースがある
- clearance = float bottom - (element の border edge top) で、float bottom が element より上にある場合に負
- 負 clearance は要素を上に引き上げるのではなく、margin collapsing の分離点として機能する

## 進捗メモ

- `clear` の押し下げ量を margin edge 基準ではなく border edge 基準へ寄せた
- `layout::tests::clear_both_positions_border_edge_below_float_not_margin_edge` を追加し、`margin-top` を持つ cleared block が不必要に余分な分だけ下がらないことを固定した
- 既存の `layout::tests::clear_both_moves_block_below_floats` も維持できている
- ただし ignored の公式 Acid2 比較差分はこの変更単独では `40,427` のままで、`smile`/`chin` の残差分解消にはまだ追加要因がある
- block layout の float/clear 処理を単一の `left/right offset` 追跡から、active float region を都度問い合わせる内部モデルへ切り替えた
- float 配置は「現在 y で使える幅が足りなければ次の float boundary まで降りる」形に寄せ、`clear` も active float region の bottom を見て計算するようにした
- `layout::tests::float_left_and_right_reduce_available_block_width` や Acid2 関連の局所回帰は維持できているが、ignored の公式比較差分は依然 `40,427`
- 鼻まわりの切り分けとして、float 配置時に負 `margin-top` を打ち消していたバグを別筋で修正し、`layout::tests::float_preserves_negative_top_margin_offset` を追加した。これは `.nose { margin: -2em ... }` の前提には効くが、ignored の公式比較差分 `33,957` 自体は動かなかったため、012-2 の本丸は引き続き `clear` の負 clearance と empty/self margin collapse にある
