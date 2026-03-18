---
id: 005-1
title: JSエンジン埋め込み & イベントループ
phase: 4
status: closed
parent: 005
---

# JSエンジン埋め込み & イベントループ

## タスク
- [x] JSエンジン選定（V8 / SpiderMonkey / Boa）と評価
- [x] Rustからのビルド統合（build.rs / cc crate）
- [x] JSコンテキストの生成・破棄
- [x] スクリプトの評価（eval）
- [x] イベントループの実装（タスクキュー、マイクロタスク）
- [x] setTimeout / setInterval の実装
- [x] Promise の非同期実行
