---
id: 002-3
title: DOMインターフェース
phase: 1
status: closed
parent: 002
---

# DOMインターフェース

DOM仕様に基づく基本的なノードインターフェースの実装。

## タスク
- [x] Node trait（nodeType, nodeName, parentNode, childNodes等）
- [x] Document構造体
- [x] Element構造体（tagName, attributes）
- [x] Text構造体
- [x] Comment構造体
- [x] DocumentType構造体
- [x] ツリー操作（appendChild, insertBefore, removeChild）
- [x] ツリー走査（querySelector相当の基本検索）
- [x] メモリ管理戦略の決定（Arena, Rc<RefCell>, slotmap等）
