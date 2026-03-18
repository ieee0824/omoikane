---
id: 002-2
title: ツリー構築
phase: 1
status: closed
parent: 002
---

# ツリー構築

HTML5仕様 (§13.2.6) に基づくツリー構築アルゴリズムの実装。

## タスク
- [x] 挿入モード（insertion mode）のステートマシン
- [x] "initial", "before html", "before head", "in head" モード
- [x] "in body" モード（主要な要素の処理）
- [x] "in table", "in row", "in cell" モード
- [x] "after body", "after after body" モード
- [x] 暗黙的タグ生成（html, head, body の自動挿入）
- [x] Foster parenting（テーブル内の不正なコンテンツ処理）
- [x] アクティブフォーマッティング要素リスト
- [x] テンプレート要素の処理
