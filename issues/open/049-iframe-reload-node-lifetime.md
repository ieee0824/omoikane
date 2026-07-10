---
number: 049
slug: iframe-reload-node-lifetime
parent:
status: open
---

# iframe 再ロード時の旧サブ文書ノードの寿命管理

## 概要

iframe の `src` 変更で contentDocument が再ロードされた際、旧サブ文書ツリーのノードを保持し続ける
JS ラッパの挙動を、実ブラウザの「detached だがアクセス可能」なセマンティクスに近づける。

## 背景（PR #104 Copilot レビューより）

- 016-9 の再ロード処理は、メモリの単調増加を防ぐため旧サブ文書ツリーを `HostState.nodes`
  レジストリから unregister する（`unregister_tree`、テスト `iframe_reload_unregisters_previous_sub_document_tree`）
- その結果、旧文書への参照を保持していた JS コード（`oldDoc = iframe.contentDocument` のキャッシュや
  旧文書から query したノード）が `__omoikane_node_type` / `__omoikane_get_attribute` 等の primitive で
  「node not found」を throw する
- 実ブラウザでは旧文書のノードは detached な文書として生存し、アクセス自体は可能

## トレードオフ

- **現状（評価: 許容）**: メモリは有界。1ページ処理で終了するヘッドレス用途・Acid3 では旧文書への
  stale 参照は発生しない
- **保持する場合**: 実ブラウザ相当のセマンティクスだが、`src` を繰り返し切り替えるページで
  ノードレジストリが無限に成長する（PR #104 1巡目レビューの指摘）

## 対応候補

- JS ラッパ側で「unregister 済みノード」への primitive 呼び出しを throw ではなく
  detached ノード相当のフォールバック値（nodeType 等はラッパ側キャッシュ）にする
- または旧文書ツリーを世代管理し、上限付きで保持（LRU 的に解放）

## 優先度

低 — 実サイトで iframe src を動的に切り替えつつ旧文書参照を使い続けるケースは限定的。
実サイト互換の問題が観測されたら着手する。
