---
number: 066
slug: iframe-renavigation-dynamic-onload
parent: 016
status: open
---

# 接続済み iframe の src 再ナビゲーションと動的 on* 属性配線（Acid3 test 80）

## 概要

接続済み iframe の `src` 変更で load イベント付きの再ナビゲーションをスケジュールし、
`setAttribute('onload', 'コード')` で後から設定された on* content 属性をイベントハンドラとして配線する。

## 失敗内容

```
Test 80 failed: timeout -- could be a networking issue
```

test 48 が接続済み iframe#selectors に対して
`iframe.setAttribute("onload", "document.getElementById('linktest').removeAttribute('class')"); iframe.src = href + "?" + number;`
を実行し、test 80 は linktest の class が除去される（= 再ロード完了で onload 発火）まで retry → 現状は発火せずタイムアウト。

## 原因分析（調査済み）— 2つの独立した欠落の複合

### 欠落 A（主因）: src 再代入が再ナビゲーションをスケジュールしない
- load イベントを発火する `TimerPayload::ResourceLoad` をキューするのは `schedule_connected_resource_loads`（`src/js/mod.rs:269-290`）だけで、呼び出し元は「detached サブツリーの接続時」のみ（初期構築 :687、appendChild :2165、replaceChild :2823、document.write :3309）
- `set_attribute_native`（`src/js/mod.rs:2381-2407`）は属性設定と style dirty 化のみで ResourceLoad をキューしない
- `iframe_content_document`（:302-357）の lazy reload は contentDocument アクセス時のみ動き、**load を dispatch しない**。test 80 は contentDocument に触れないため発火しない

### 欠落 B: 動的 setAttribute('onload') が配線されない
- content 属性 → ハンドラ変換は `wireInlineHandlers`（`src/js/dom_bootstrap.js:3105-3126`）がページロード時に**一度きり**全ツリー走査するだけ。load 発火後の後付け setAttribute は配線されない
- 016-16 の3経路（パース時属性 / addEventListener / on* IDL）のどれにも該当しない第4経路

### 原因ではないと確認済みの項目
- クエリ付き URL 取得: `resolve_url` が query を保持し、フィクスチャサーバ（`tests/acid3_common/harness.rs:179`）が query を剥がして配信 → 動作済み
- `document.links[1]`: 正しく a#linktest を指す（links[0] は map 内 area）
- `.src` IDL セッターの content 属性反射: 動作済み（`src/js/dom_bootstrap.js:2499-2501`）
- notifications['xhtml.1/2/3']: `globalThis.parent = globalThis` 経由で記録される見込み（retry 突破後に実地検証）

## 実装方針

### Fix 1: set_attribute_native で再ナビゲーションをスケジュール（`src/js/mod.rs:2381-2407`）
- (iframe, "src") または (object, "data") への属性設定時、`document_root_for_node` で**接続済み**なら `TimerPayload::ResourceLoad { node_id }` を macrotasks へ push（`schedule_connected_resource_loads` の単一ノード版）
- `pending_resource_loads.insert` の戻り値で重複キューを防止（test 65 の「src 設定 → append」パターンでの二重ロード防止）
- **detached iframe では絶対にキューしない**（既存テスト `detached_iframe_load_waits_until_connected` :8289 の不変条件）
- `iframe_documents[id].loaded_src` と比較して src 実変化時のみキューし副作用を最小化
- 既存の ResourceLoad ハンドラ（:909-968）が旧サブ文書 unregister → 再ロード → load dispatch まで行うためハンドラ側の改修は不要

### Fix 2: 動的 on* content 属性の配線（`src/js/dom_bootstrap.js`）
- `wireInlineHandlers` の1属性分を `applyInlineHandlerAttribute(node, name)` に切り出し、(1) 初期一括配線、(2) `setAttribute`（:640-642、`/^on./i` の時）、(3) `removeAttribute`（:994、on* 削除時にハンドラ解除）から呼ぶ
- per-node・per-type で1つ保持（`node.__contentAttrHandlers[type]` 等）し、再設定時は旧ハンドラを removeEventListener してから `new Function('event', value)` を addEventListener（初期配線との二重登録を防ぐ）
- body/frameset の window 反射（`WINDOW_REFLECTED_HANDLERS` :3093-3097）分岐も流用

## テスト計画

`src/js/mod.rs` の `#[cfg(test)]`（`pump_zero_delay_tasks` / `spawn_static_http_server` 利用）:

- 接続済み iframe の `.src` 変更 → pump → load リスナ発火が +1（`same_document_direct_reinsertion_does_not_reload_iframe` :8456 と対になるケース）
- `setAttribute('src', ...)` 経路でも同様に発火
- 動的 `setAttribute('onload', "code")` → src 変更 → pump → コード実行（test 48 の最小再現）
- detached iframe の src 変更は load を出さない（:8289 の不変維持）
- `removeAttribute('onload')` でハンドラ解除
- 接続済み object の `data` 変更でも再ナビゲーション

## 回帰リスク

- **049（unregister 済みノード）**: 再ロードは旧サブ文書を unregister する。selectorTest（test 33-46）は毎回ローカル変数で doc を取得しグローバル保持しないため参照喪失は起きない — リスク低
- **test 65 の「src 設定 → append」**: detached 時は接続チェックで弾かれ二重ロードなし。「接続済みかつ src 実変化」を厳密条件にする
- `iframe_changing_src_reloads_content_document`（:8897、lazy reload テスト）: pump しないためアサート不変
- **on* 動的配線の二重登録**: 初期 wireInlineHandlers との重複を per-node/per-type 解除で防ぐ。`event_handler_idl_attribute_registers_and_replaces_listener`（:6616）、`body_onload_attribute_fires_on_load`（:4635）との整合を回帰確認
- FAITHFUL の stall 判定（`harness.rs:334`）: 修正後は test 48 の tick 内で ResourceLoad が消化され class 除去 → retry ループに入らない。DirectDrive も update() 後の `tick(0)` で消化される

## 実装時の追加知見（2026-07-13）— ノード identity のアドレス再利用バグ

Fix 1/2 の実装により、本 issue の回帰リスク欄で「リスク低」とした 049 系の問題が**実際に発現した**:

- Fix 1 の eager 再ナビゲーションは test 48 時点で旧 #selectors サブ文書を `unregister_tree` する
  （従来は tests 49-79 の間 reload が起きず解放もされなかった）
- `NodeHandle::identity()` が `Rc::as_ptr`（アドレス）だったため、解放済みノードのアドレスが後続の
  `createElement` に再利用されると、identity をキーとする JS ラッパキャッシュが**新規ノードに
  解放済みノードの stale ラッパ（型違い）を返す**エイリアシングが発生
- 症状: Fix 1+2 のみでは FAITHFUL/DIRECT とも 98〜99 で flaky に破損
  （Test 50 rowIndex / Test 62 htmlFor / Test 72 style 等、実行ごとに変わるアドレス再利用の典型）

**同伴修正**: `NodeHandle::identity()` を `Rc::as_ptr` から単調カウンタ（`NEXT_NODE_ID: AtomicUsize`、
`fn new` の単一経路で採番、再利用なし）に変更（`src/dom/mod.rs`）。identity の意味論
（ノードごとに一意、clone 間で共有）は不変で、「再利用されない」性質だけが加わる。
これにより両モード安定 100/100（9/9 クリーン vs 非単調ビルドは破損）。
049 の残スコープ（stale 参照の "node not found" セマンティクス・レジストリの単調成長）はこの修正では触れておらず、引き続き 049 で追跡する。

## 受け入れ条件

- Acid3 test 80 が FAITHFUL/DIRECT 両モードで PASS（notifications の3アサート含む）
- 上記単体テストの追加と既存テスト全通過

## 関連 issue

- [049 iframe 再ロード時の旧サブ文書ノードの寿命管理](049-iframe-reload-node-lifetime.md) — 本 issue で再ナビゲーション経路が増えるため、stale 参照の実害が観測されたら 049 に着手する
