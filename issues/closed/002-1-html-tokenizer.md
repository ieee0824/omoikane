---
id: 002-1
title: HTMLトークナイザ
phase: 1
status: closed
parent: 002
---

# HTMLトークナイザ

HTML5仕様 (§13.2.5) に基づくトークナイザの実装。

## タスク
- [x] ステートマシンの基本構造
- [x] Data / Tag open / Tag name / Attribute 等の基本ステート
- [x] 開始タグ・終了タグ・自己閉じタグのトークン生成
- [x] 属性のパース（名前・値・クォート処理）
- [x] コメントトークン
- [x] DOCTYPEトークン
- [x] 文字参照（&amp; &lt; &#x41; 等）のデコード
- [x] EOF処理
- [x] パースエラーの報告
