---
id: 002-3
title: DOMインターフェース
phase: 1
status: open
parent: 002
---

# DOMインターフェース

DOM仕様に基づく基本的なノードインターフェースの実装。

## タスク
- [ ] Node trait（nodeType, nodeName, parentNode, childNodes等）
- [ ] Document構造体
- [ ] Element構造体（tagName, attributes）
- [ ] Text構造体
- [ ] Comment構造体
- [ ] DocumentType構造体
- [ ] ツリー操作（appendChild, insertBefore, removeChild）
- [ ] ツリー走査（querySelector相当の基本検索）
- [ ] メモリ管理戦略の決定（Arena, Rc<RefCell>, slotmap等）
