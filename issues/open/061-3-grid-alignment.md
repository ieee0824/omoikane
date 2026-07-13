---
number: 061-3
slug: grid-alignment
parent: 061
status: open
---

# CSS Grid: アラインメント

## 目的

グリッドのアラインメント系プロパティを実装する。061 の第3段。kasaneteto.jp は
place-content 55 箇所を使用しており、セル内・トラック整列で描画がさらに改善する。

## スコープ

- セル内のアイテム整列（各アイテムのセル矩形内での寄せ）:
  - `justify-items`（インライン軸=水平）/ `align-items`（ブロック軸=垂直）
  - `justify-self` / `align-self`（アイテム個別、コンテナ既定を上書き）
  - 値: `start` / `end` / `center` / `stretch`（既定 stretch）
- グリッド全体のトラック整列（トラック合計がコンテナより小さい時の余白配分）:
  - `justify-content` / `align-content`
  - 値: `start` / `end` / `center` / `space-between` / `space-around` / `space-evenly` / `stretch`
- shorthand: `place-items`（align-items justify-items）/ `place-self` / `place-content`
  （align-content justify-content）。1値指定は両軸に適用

## 実装方針

- 061-1/061-2 で確定したトラック・セル矩形に対し、
  - content 整列: トラック開始オフセットとトラック間 gap を余白配分で調整
  - items/self 整列: 各アイテムをセル矩形内で寄せ（stretch 以外はアイテムの
    intrinsic サイズを使い、start/end/center でオフセット）
- stretch（既定）は現状のセル充填挙動を維持

## 受け入れ条件

- `justify-items: center` / `align-items: end` でアイテムがセル内で正しく寄る単体テスト
  （子 rect を具体値でアサート）
- `justify-content: space-between` 等でトラックの余白配分が正しいテスト
- `place-content: center` / `place-items: center` の shorthand 展開テスト
- `justify-self` / `align-self` がコンテナ既定を上書きするテスト
- 既存テスト・Acid3 97/100 に回帰なし

## 関連

- 061 CSS Grid（親）/ 061-1・061-2（前段、実装済み）
