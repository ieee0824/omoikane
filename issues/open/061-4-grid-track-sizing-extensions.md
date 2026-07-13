---
number: 061-4
slug: grid-track-sizing-extensions
status: open
parent: 061
---

# Grid トラックサイジング拡張（calc / 単位 / minmax / auto-fill / auto-fit）

## 概要

grid-template-columns/rows のトラックリストで px / % / fr / auto 以外の値が
一切パースできず、**1 つでも不明なトラックがあると track_list 全体が None になり
`1fr` 単一カラムへ崩壊**する。実サイトで頻出の calc() / vw 単位 / minmax() /
auto-fill / auto-fit / 複数トラック repeat を解決する。

## 背景（実測: kasaneteto.jp レンダリングログ）

- トラックサイズに `calc(120 * var(--vw-scale-pc))` 形式が多用（var 置換は済むが calc が残る）
- `1fr 21.09375vw 9.322917vw` のような viewport 単位トラック
- `minmax(0, calc(920 * var(--vw-scale-pc)))`
- `compute_value`（src/css/style.rs）は grid-template 系を**単位変換前に**
  `Keyword(render_value(value))` で early-return するため、vw/em/rem すら px にならない
- calc 評価器 `evaluate_calc` / `CalcPxPercent`（src/css/style.rs）は加減乗除・
  単位変換込みで実装済み。grid 側から使われていないだけ

## スコープ

1. **compute_value の grid-template 系処理を単位解決付きに変更**
   - Value ツリーを走査し Length → px 変換、calc() → `evaluate_calc` で評価
     （px 単独 / px+% 混在は `calc(Apx + B%)` の canonical 形式で保持）
   - fr / % / auto / minmax() / repeat() / 名前付きライン `[...]` は構造を保った
     canonical 文字列にレンダリング（例: `300px minmax(0px, 1fr) repeat(auto-fill, 120px)`）
2. **grid.rs の track_list / parse_track 拡張**
   - `minmax(min, max)`: 新 Track 表現。max が fr なら fr として伸長しつつ min を下限に、
     固定値同士なら clamp
   - `repeat(N, <track...>)`: 複数トラックの繰り返し（現状は単一トラックのみ）
   - `repeat(auto-fill, ...)` / `repeat(auto-fit, ...)`: 利用可能幅とトラック最小サイズから
     繰り返し数を算出。auto-fit はアイテムのない繰り返しトラックを 0 幅に collapse
   - `min-content` / `max-content`: 当面 auto と同等（intrinsic width ベース）で受理
   - 名前付きライン `[name]`: スキップして残りのトラックを正しくパース
     （ライン名の解決自体は 061-5 以降。無視して壊れないことが目的）
   - `calc(Apx + B%)` canonical 形式のパース（% は基準長で解決）
   - **パース不能トラックが混ざっても全体を捨てず当該トラックを auto 扱い**にし、
     単一カラム崩壊を防ぐ
3. **resolve_tracks の minmax / auto-fill 対応**

## 受け入れ条件

- vw / calc / minmax / auto-fill / auto-fit / 複数トラック repeat を含む
  grid-template-columns/rows の回帰テストが通る（期待幅を具体値でアサート）
- kasaneteto.jp のトラック起因の単一カラム崩壊が解消する
- 既存テスト・Acid3 スコア（97/100）の維持

## 関連

- 親: 061 CSS Grid レイアウト
- 後続: 061-5 名前付きエリア（grid-template ショートハンドのトラック部が本 issue に依存）
