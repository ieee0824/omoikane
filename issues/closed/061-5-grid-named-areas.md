---
number: 061-5
slug: grid-named-areas
status: closed
parent: 061
---

# Grid 名前付きエリア（grid-template-areas / grid-area / grid-template）

## 概要

名前付きエリアによる配置を実装する。kasaneteto.jp のレンダリング実測で
`grid-area`（名前参照）22 箇所・`grid-template` ショートハンド 8 箇所が未対応で、
ヒーロー/フッター等のエリアレイアウトが全崩壊している（現状の最重要ギャップ）。

## 背景（実測: kasaneteto.jp レンダリングログ）

- `grid-template: "kasane teto teto" auto "official official official" auto ... / 1fr 21.09375vw 9.322917vw`
  形式（エリア行とトラックサイズの交互 + `/` 後に列トラック）が 8 箇所
- `grid-area: title` などエリア名参照が 22 箇所
- いずれも is_supported_property 未登録でカスケードから落ちる

## スコープ

1. **CSS 側**
   - `grid-template-areas` / `grid-area` / `grid-template` を is_supported_property に登録
   - shorthand 展開（src/css/shorthand.rs）:
     - `grid-area: <name>` → grid-row-start/column-start/row-end/column-end に名前を配布
       （1〜4 値の line 形式 `grid-area: 1 / 2 / 3 / 4` も既存 grid-row/column 展開に合わせて対応）
     - `grid-template: <areas+rows> / <columns>` → grid-template-areas + grid-template-rows +
       grid-template-columns に分解（エリア文字列とトラックサイズの交互列を分離）
   - grid-template-areas の値（文字列リスト）を computed style に保持
2. **layout/grid.rs 側**
   - エリア行列のパース: 行ごとの文字列 → セル名グリッド、名前ごとに矩形
     (row_start, row_span, col_start, col_span) を算出。非矩形は当該エリア無効
   - `.`（無名セル）対応
   - axis_request / place_items に名前解決を統合:
     grid-row/column-start/end の値がエリア名なら対応するエリア境界ラインに解決
     （`name` → start 側は `name-start`、end 側は `name-end` 相当の簡易規則）
   - エリア行数がトラックサイズより多い場合の暗黙行、逆の場合の余りトラックの扱い
3. **テスト**
   - エリア配置の基本（座標・サイズの具体値アサート）
   - `.` セル、非矩形エリアの無効化
   - grid-template ショートハンド（kasaneteto 実パターン: エリア行+auto 交互、`/` 後の列トラック、
     calc/vw トラック混在 — 061-4 の成果に依存）

## 受け入れ条件

- 上記テストが通る
- kasaneteto.jp のヒーロー/フッターのエリアレイアウトが名前どおりに配置される
- 既存テスト・Acid3 スコア（97/100）の維持

## 関連

- 親: 061 CSS Grid レイアウト
- 依存: 061-4 トラックサイジング拡張（grid-template のトラック部パース）
