---
id: 003-2
title: セレクタマッチング
phase: 2
status: closed
parent: 003
---

# セレクタマッチング

DOMノードに対するCSSセレクタのマッチング処理。

## タスク
- [x] 単純セレクタのマッチング（型、クラス、ID、ユニバーサル）
- [x] 属性セレクタのマッチング（[attr], [attr=val], [attr~=val]等）
- [x] 擬似クラス（:first-child, :last-child, :nth-child等）
- [x] 結合子の処理（右から左へのマッチング）
- [x] 詳細度（specificity）の計算
- [x] パフォーマンス最適化（Bloom filter等）
