---
number: 061-1
slug: grid-track-sizing
parent: 061
status: closed
---

# CSS Grid: 明示的グリッド定義とトラックサイジング

## 目的

`display: grid` / `inline-grid` の基本を実装する。明示的なトラック定義
（`grid-template-columns` / `grid-template-rows`）に沿ってセルを確定し、
子を行優先（row-major）で自動配置し、`gap` を反映する。061 の第1段。

## スコープ

- `src/layout/grid.rs` を新設し、`layout_grid_container(...)`（flex と同じ引数シグネチャ）を実装
- `src/layout/mod.rs` の layout_element で `is_flex_container` 分岐の後に
  `is_grid_container(&style)` 分岐を追加
- CSS: `is_supported_property` に grid 系を登録し、値をパース
  - `grid-template-columns` / `grid-template-rows`: トラックリスト。対応値は
    `<length>`(px) / `%` / `fr` / `auto` / `repeat(N, <track>)`（整数 N のみ）
  - `gap` / `row-gap` / `column-gap`（`grid-gap` エイリアス含む）
- トラックサイジング（本段の範囲）:
  - 固定（px, %）と auto をまず確定
  - 余り幅を `fr` トラックに比率配分
  - auto トラックは中身の最大コンテンツ幅で近似（既存の shrink-to-fit 幅算出を活用可）
- 自動配置: 子要素を宣言順に、列数（columns トラック数）に従って行優先でセルへ割当。
  行数が足りなければ暗黙行を生成（高さは行内の最大コンテンツ高）
- 各セル内で子を通常のブロック/インラインとしてレイアウトし、セル矩形を
  containing block として渡す
- positioned 子（absolute/fixed）は既存 flex と同様に out-of-flow として処理

## 非スコープ（後続段）

- `grid-column` / `grid-row` による明示配置・スパン → 061-2
- `justify-*` / `align-*` / `place-*` → 061-3
- `minmax()` / `auto-fill` / `auto-fit` / 名前付きライン → 061-4

## 受け入れ条件

- `display:grid; grid-template-columns: 1fr 1fr; gap: 10px` で2列グリッドになり、
  子が行優先で配置され列幅・gap が正しい単体テスト（期待値明示）
- `repeat(3, 1fr)` / `auto` / px / % 混在のトラックサイジングのテスト
- grid 未使用ページ（既存テスト・Acid3 97/100）に回帰がない

## 関連

- 061 CSS Grid（親）/ 060 CJK フォント（同じ kasaneteto 対応）
