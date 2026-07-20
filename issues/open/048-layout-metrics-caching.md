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

- `offsetWidth` / `clientWidth` / `scrollWidth` 等の各 getter が毎回 `__omoikane_layout_metrics(nodeId)` の
  ネイティブ呼び出し + `JSON.parse` を行うため、複数メトリクスの連続取得だけで同じ reflow 結果を何度も往復する
- `ensure_style_resolver` は dirty 時に全 `<style>` を毎回フルで再パースし、`ensure_layout` は全文書を再レイアウトする。
  部分無効化がないため、「ループ内で DOM を変更しつつ getComputedStyle を読む」パターン
  （Acid3 selectorTest が該当）で O(n²) 気味に退化する

## 対応内容

- DOM 変更でインクリメントする世代カウンタを導入し、要素単位のメトリクス/計算済みスタイルを
  同一世代の間キャッシュする（JS 側キャッシュ + 世代の native 露出、または native 側キャッシュ）
- `<style>` 再パースの差分化（追記された stylesheet のみパースして resolver に追加する等）は
  効果測定の上で判断する

## 関連メモ（047 レビュー由来、2026-07-13）

- 047 でインライン style 属性がカスケードに統合された結果、`compute_style_with_pseudo` が
  要素ごとに style 属性文字列を毎回 `parse_style_attribute` で再パースする
  （resolver の per-node キャッシュが効く同一 resolver 内では1回だが、resolver 再構築のたびに再パース）。
  世代ベースキャッシュ導入時に、style 属性文字列（または属性世代）をキーとした
  解析済み宣言のキャッシュも合わせて検討する。

## 関連メモ（016-15 由来）

- 016-15 で style 無効化を文書単位に絞ったが、`document_root_for_node` で owner document を
  特定できない detached ノード（`createElement` 直後などまだツリーに接続されていないノード）への
  mutation は、安全側に倒して**全文書 dirty**（`mark_all_document_styles_dirty`）にフォールバックする。
  多数の iframe を持つページで detached ノードを頻繁に変更・接続すると、本来 1 文書で済む再構築が
  全文書に波及し、わずかに再構築コストが増えうる。正しさには影響しない（過剰無効化のみ）。
  世代ベースキャッシュ導入時に、接続先が確定した時点での限定無効化を検討する。

## 現状メモ（2026-07-20、実装照合）

- レイアウトメトリクス側は未着手。`src/js/dom_bootstrap.js` の各 getter
  （`offsetWidth`/`clientWidth`/`scrollWidth`/`getBoundingClientRect` 等）が個別に
  `__omoikane_layout_metrics(nodeId)` を呼び `JSON.parse` するため、複数メトリクスの
  連続取得で JS↔native 境界越え + parse が毎回発生する。世代カウンタも JS 側キャッシュもない。
  受け入れ条件1（native 呼び出し1回への集約）は未達。
- style 側の無効化は文書単位（`mark_document_style_dirty` / `mark_style_dirty_for_node`）で、
  detached ノードは全文書 dirty にフォールバックする。`ensure_style_resolver` は dirty 時に
  全 `<style>` をフルで再パースし、`compute_style_with_pseudo` は要素ごとに style 属性を
  毎回 `parse_style_attribute` する（いずれも 048 が想定する差分化・キャッシュは未実装）。
- ただし本 issue 執筆後に 076（rule index / selector match cache / media query cache、
  2026-07-16〜17）が別途入り、セレクタマッチング側の `O(n²)` 気味の退化は緩和されている。
  背景で述べた selectorTest の悪化懸念のうちセレクタ側は本 issue のスコープ外で改善済み。

## 優先度

低〜中 — 正しさには影響しない。Acid3 規模では現状でも許容範囲。selectorTest 解放（016-15）後に
実測して悪化が見えたら着手する。着手前に、076 のセレクタ perf 改善を踏まえて Acid3 selectorTest を
再計測し、残るボトルネックが本当にメトリクス往復側にあるかを確認してからスコープを見直す。

## 受け入れ条件

- 同一 DOM 状態での連続メトリクス取得が native 呼び出し1回に集約される
- DOM 変更後は正しく無効化される（既存の forced reflow テストが通り続ける）
