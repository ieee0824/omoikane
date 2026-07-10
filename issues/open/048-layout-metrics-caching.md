---
number: 048
slug: layout-metrics-caching
parent:
status: open
---

# レイアウトメトリクス・スタイル解決のキャッシュ（perf）

## 概要

044-2/016-8 で実装したレイアウトメトリクスと getComputedStyle の再計算コストを、
DOM 世代ベースのキャッシュで削減する。

## 背景（PR #105 Copilot レビュー + 批判的レビューより）

- `offsetWidth` / `clientWidth` / `scrollWidth` 等の各 getter が毎回 `__layoutMetrics()` ネイティブ呼び出し +
  `JSON.parse` を行うため、複数メトリクスの連続取得だけで同じ reflow 結果を何度も往復する
- `ensure_style_resolver` は dirty 時に全 `<style>` を毎回フルで再パースし、`ensure_layout` は全文書を再レイアウトする。
  部分無効化がないため、「ループ内で DOM を変更しつつ getComputedStyle を読む」パターン
  （Acid3 selectorTest が該当）で O(n²) 気味に退化する

## 対応内容

- DOM 変更でインクリメントする世代カウンタを導入し、要素単位のメトリクス/計算済みスタイルを
  同一世代の間キャッシュする（JS 側キャッシュ + 世代の native 露出、または native 側キャッシュ）
- `<style>` 再パースの差分化（追記された stylesheet のみパースして resolver に追加する等）は
  効果測定の上で判断する

## 優先度

低〜中 — 正しさには影響しない。Acid3 規模では現状でも許容範囲。selectorTest 解放（016-15）後に
実測して悪化が見えたら着手する。

## 受け入れ条件

- 同一 DOM 状態での連続メトリクス取得が native 呼び出し1回に集約される
- DOM 変更後は正しく無効化される（既存の forced reflow テストが通り続ける）
