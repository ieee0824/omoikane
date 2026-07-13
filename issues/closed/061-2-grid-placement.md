---
number: 061-2
slug: grid-placement
parent: 061
status: closed
---

# CSS Grid: 明示的アイテム配置とスパン

## 目的

061-1 の自動配置に加え、`grid-column` / `grid-row` による明示的なセル配置とスパンを
実装する。061 の第2段。kasaneteto.jp は grid-column 109 / grid-row 94 箇所と明示配置を多用。

## スコープ

- `grid-column` / `grid-row`(および longhand `grid-column-start/end` / `grid-row-start/end`)
  - line ベース: `grid-column: 2 / 4`(2番目のラインから4番目のラインまで＝2トラック span）
  - span: `grid-column: span 2` / `grid-row: 1 / span 3`
  - 単一値: `grid-column: 2`(開始のみ、span 1)
  - 負のライン番号(末尾から)にも最小対応(`grid-column: 1 / -1` = 全幅)
- 明示配置アイテムを先にセルへ確定し、残りを 061-1 の自動配置アルゴリズムで
  空きセルに row-major 充填(占有セルはスキップ)
- スパンによりトラック範囲をまたぐアイテムは、またいだトラック幅/高 + 内側の gap の
  合計を専有領域とする
- 明示配置で行/列が不足する場合は暗黙トラックを生成
- `grid-auto-flow` は row(既定)のみ対応で可(column/dense は非スコープ、後続)

## 非スコープ

- justify/align/place（061-3）、minmax()/auto-fill/fit/名前付きライン（061-4）
- grid-auto-flow: column / dense

## 受け入れ条件

- `grid-column: 1 / 3`(2トラック span)や `span 2` が正しいセル範囲・幅になる単体テスト
  （子 rect を具体値でアサート）
- 明示配置と自動配置の混在(一部の子だけ grid-column 指定)で、自動配置が占有セルを
  避けて充填されるテスト
- `grid-column: 1 / -1`(全幅)のテスト
- 061-1 の既存テスト・Acid3 97/100 に回帰なし

## 関連

- 061 CSS Grid（親）/ 061-1 トラックサイジング（前段・実装済み）
