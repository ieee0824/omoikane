---
number: 016-8
slug: computed-style-exposure
parent: 016
status: open
---

# getComputedStyle の実値化（カスケード結果のJS露出）

## 目的

CSS カスケードの計算結果を JS の `getComputedStyle` に接続し、実値を返せるようにする。

## 背景（GAP_ANALYSIS.md セクション1 P5/P6、セクション3 領域D）

- 現状 `getComputedStyle` は常に `""` を返すスタブで、カスケード結果が JS に露出していない。
- `src/css/matcher.rs` / カスケード計算は存在するが JS 側に繋がっていない。
- Acid3 の `selectorTest` は `getComputedStyle(node,'').zIndex` で全セレクタ判定を行い、
  test0 は `whiteSpace === 'pre-wrap'` を確認するため、bucket3 全体と test 0/47 の必須前提。

## 相互参照

- **044-2（`issues/open/044-2-layout-metrics-bindings.md`）と同根**。
  044-2 は `getComputedStyle()` 実値返却 + レイアウトメトリクス API を扱う。
  本issueと 044-2 は同じ「カスケード/レイアウト結果の JS 露出」基盤であり、
  実装は一体で進めること（重複実装を避ける）。

## スコープ

- カスケード計算済みプロパティを `getComputedStyle` の戻り値オブジェクトに露出
- `document.defaultView.getComputedStyle` と global 両対応

## 受け入れ条件

- `getComputedStyle(el,'').whiteSpace` 等がカスケード結果の実値を返す
- Acid3 test 0 の `pre-wrap` 判定と selectorTest の zIndex 判定が機能する
