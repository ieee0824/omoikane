---
number: 015-2
slug: anonymized-priority-css-implementation
parent: 015-anonymized-real-world-rendering-gap
status: open
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
