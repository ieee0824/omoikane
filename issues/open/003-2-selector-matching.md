---
id: 003-2
title: セレクタマッチング
phase: 2
status: open
parent: 003
---

# セレクタマッチング

DOMノードに対するCSSセレクタのマッチング処理。

## タスク
- [ ] 単純セレクタのマッチング（型、クラス、ID、ユニバーサル）
- [ ] 属性セレクタのマッチング（[attr], [attr=val], [attr~=val]等）
- [ ] 擬似クラス（:first-child, :last-child, :nth-child等）
- [ ] 結合子の処理（右から左へのマッチング）
- [ ] 詳細度（specificity）の計算
- [ ] パフォーマンス最適化（Bloom filter等）
