---
number: 016-16
slug: iframe-onload-event
parent: 016
status: open
---

# iframe の load イベント発火

## 目的

iframe のサブ文書ロード完了時に load イベント（`iframe.onload` / addEventListener('load')）を発火させる。

## 背景

- 016-9 で contentDocument（サブ文書の生成・遅延ロード）は実装済みだが、load イベントは未発火
- Acid3 test 65 は iframe onload で `kungFuDeathGrip` を蓄積し、test 69 がそれを検証するため、
  未発火だと test 69 が「kungFuDeathGrip.title was null」で fail する
- 016-4 で整備した load イベント基盤・on* インラインハンドラ配線と連携する

## スコープ

- サブ文書ロード完了（遅延ロードの初回確定時点）での load イベント dispatch
- `iframe.onload` プロパティ / `onload` 属性 / addEventListener の3経路
- document.write で動的生成された iframe でも発火すること（Acid3 の実パターン）

## 受け入れ条件

- Acid3 test 69 の kungFuDeathGrip 検証が前進する（test 65/69 の該当部分が PASS）
- 静的・動的生成の両方の iframe で load が発火するテストを追加
