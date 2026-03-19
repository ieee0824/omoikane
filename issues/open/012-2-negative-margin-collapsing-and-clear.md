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
