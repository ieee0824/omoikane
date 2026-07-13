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

## 関連メモ（016-15 由来）

- 016-15 で iframe reload 時に旧サブ文書の `iframe_documents` / `document_styles` エントリを掃除する
  処理を入れたが、この cleanup は**非再帰**である。ネストした iframe を含む外側 iframe を reload すると、
  内側 iframe のサブ文書に対応する `iframe_documents` / `document_styles` エントリが解放されずに残留する。
- `document_styles` は文書ルートの identity でキーされるが、iframe 経路以外で生じた文書
  （`cloneNode` 等で作られたスタンドアロン文書ツリー）の `document_styles` エントリを解放する経路が無く、
  それらは解放されないまま残る。
- いずれもメモリの単調増加要因だが、1 ページ処理で終了するヘッドレス用途では有界であり正しさには影響しない。
  再帰的 cleanup / 世代管理を入れる際にまとめて解消する。

## 進捗（066 での部分解消、2026-07-13）

- 066（iframe src 再ナビゲーション）で「Acid3 では stale 参照は発生しない」という本 issue の前提が崩れ、
  `NodeHandle::identity()`（旧 `Rc::as_ptr`）の**アドレス再利用による JS ラッパキャッシュのエイリアシング**が
  実際に発現した（解放済みノードのアドレスが新規ノードに再利用され、型違いの stale ラッパが返る）。
  066 で identity を単調カウンタ化して解消済み（`src/dom/mod.rs` の `NEXT_NODE_ID`）。
- 本 issue の残スコープは変わらず: (1) unregister 済みノードへの primitive 呼び出しが throw する挙動を
  detached 相当のフォールバックにする、(2) ネスト iframe reload の非再帰 cleanup、
  (3) レジストリ/`document_styles` の単調成長。

## 優先度

低 — 実サイトで iframe src を動的に切り替えつつ旧文書参照を使い続けるケースは限定的。
実サイト互換の問題が観測されたら着手する。ただし 066 で再ナビゲーション経路が増えたため、
stale 参照が起きうる面は従来より広がっている点に留意。
