---
number: 015-2
slug: anonymized-priority-css-implementation
parent: 015-anonymized-real-world-rendering-gap
status: closed
---

# 主要CSS不足の優先実装

## 概要

015-1 の観測ログ結果を元に、描画差分への寄与が大きいCSS機能を
優先度順に実装する。

## スコープ

- 優先度上位の未対応CSSを2-3件実装
- 既存レイアウト/ペイントの回帰テスト追加
- 匿名化再現ページでの目視差分改善を確認

## 受け入れ条件

- 実装対象を issue/PR に明記し、before/after の差分を比較できる
- 既存テスト + 追加テストが通る
- 匿名化ページで主要ブロックの崩れが減少する

## 実装対象

- `transform`（`translate`, `translateX`, `translateY`, `translate3d`, `matrix` の平行移動成分）
- `overflow-x` / `overflow-y`（`hidden` を clipping 判定へ反映）

## 実施結果

- レイアウト処理に transform 平行移動オフセット適用を追加
- `overflow` 判定を `overflow` / `overflow-x` / `overflow-y` の3系統で評価するよう変更
- 未対応CSS判定のサポート対象へ `transform`, `overflow-x`, `overflow-y` を追加
- 回帰テスト追加:
  - transform が座標へ反映されること
  - overflow-x hidden で `Overflow::Hidden` になること

## 検証

- `cargo test layout::tests::` 通過
- `cargo test css::style::` 通過
