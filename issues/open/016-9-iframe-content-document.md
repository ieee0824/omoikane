---
number: 016-9
slug: iframe-content-document
parent: 016
status: open
---

# iframe / contentDocument サブブラウジングコンテキスト

## 目的

iframe 要素にサブブラウジングコンテキスト（独立 document）を持たせ、
`contentDocument` でアクセスできるようにする。

## 背景（GAP_ANALYSIS.md セクション1 P7、セクション3 領域E）

- 現状 iframe 要素はパースされるが、サブドキュメント / contentDocument の概念が無い。
- `getTestDocument()` = `document.getElementById("selectors").contentDocument` であり、
  test 1,2,3,6,9,11-13,14,15,33-44,46,48,65,69-71,74,80 等 約 35 テストの門。
- 実測でも test 14/15 が "no <iframe> support"、test 71/72 が "missing document for test" で失敗。

## スコープ

- iframe の独立 document 生成（空 HTML / src ロード）
- `contentDocument` / `contentWindow` アクセサ
- MIME 判定（image/png や text/plain を HTML としてパースしない: test 14/15）
- onload イベント（016-4 と連携）

## 受け入れ条件

- `iframe.contentDocument` が独立した document を返す
- 空 iframe に対する DOM 操作が親と分離して動く
- getTestDocument 依存テストの前提が満たされる

## 進捗 (2026-07-10)

ブランチ `issue/016-9-iframe-content-document`（`issue/016-7-document-write` から分岐）。

### 実装内容

- **サブブラウジングコンテキスト**: `HostState` に `iframe_documents`
  (`HashMap<iframe_node_id, IframeDocument>`) と `base_url` を追加。iframe 要素
  ごとに独立した Document ノードを生成し、その全ノードツリーを既存の `nodes`
  レジストリに登録することで、親文書と同じ DOM primitive で走査・変更できるよう
  にした。ドキュメントはツリーとして親文書に接続しないため走査は自然に分離される。
- **遅延ロード**: `contentDocument` 初回アクセス時に読み込む。`src` 無し /
  `about:blank` は空 HTML スケルトン (`<html><head></head><body></body></html>`)。
  `src` あり（相対は `base_url` で解決、`data:` は inline デコード）は HTTP 取得し、
  **MIME 判定**で `text/html` / `application/xhtml+xml` のみ HTML としてパース。
  それ以外（image/png, text/plain, application/xml, image/svg+xml 等）や取得失敗は
  空スケルトンを返す（Acid3 test 14/15 の「PNG/テキストを HTML として解釈しない」）。
- **再ロード**: `IframeDocument.loaded_src` に読込元 src を記録。`src` 属性が変われば
  次回アクセスで再取得。`HTMLIFrameElement.src` の getter/setter を追加。
- **JS バインディング**: ネイティブ primitive `__omoikane_iframe_content_document`
  を追加。`dom_bootstrap.js` に `HTMLIFrameElement`（`contentDocument` /
  `contentWindow` facade / `src`）を追加し `ELEMENT_CTORS`・グローバルへ登録。
  `wrapNode` が nodeType===9 のノードを Document として包むよう拡張。
  `Document.getElementById` を `this` 起点の `querySelector('#id')` に変更し、
  親子で id が混ざらないようスコープ化（メイン文書では従来と同一挙動）。
- `execute_document_scripts` が `base_url` を保存。テスト用に公開 API
  `JsRuntime::set_base_url` を追加。

### スコア変化

- 着手前: 52/100（016-7 document.write 完了時点）
- 完了後: **56/100**（Faithful / DirectDrive 一致）
- 新規 pass: test 44, 73, 76, 78。加えて bucket3（test 33–47）の getTestDocument
  依存が解消され、失敗理由が「null/undefined」から getComputedStyle 未実装
  （016-8 の領域）へ前進。test 14/15/16/70 は既存 pass を維持（回帰なし）。

### テスト

`src/js/mod.rs` に 12 件追加:
- `is_html_mime_type_matches_html_essences_only`, `blank_html_document_has_html_head_and_body_but_no_content`
- 空 iframe の独立 document / 親子 DOM 分離 / getElementById のドキュメント別スコープ /
  アクセス間の同一性 / HTML src パース / image/png・text/plain 非パース /
  相対 src の base 解決 / src 変更による再ロード

### レビュー指摘対応 (PR #104)

`issue/016-7-document-write` の最新（レビュー修正コミット）をマージした上で、以下を修正:

- **ownerDocument の親子分離**: 全ノードの `ownerDocument` がトップレベル文書を返して
  いた不具合を修正。ネイティブ `__omoikane_owner_document`（ツリー根まで遡り、根が
  Document ならその id を返す）を追加。デタッチ済みの生成ノードは
  `Document.create*` が付与する生成元文書 (`__ownerDoc`) にフォールバック。document
  自身は null。
- **URL スキーム判定の大小無視**: `HTTP://` / `HTTPS://` を取りこぼしていた判定を
  `eq_ignore_ascii_case` に変更。
- **再ロード時の旧ツリー解放**: `HostState::unregister_tree` を追加し、`src` 変更に
  よる再ロード前に旧サブ文書ツリーを `nodes` レジストリから除去（リーク防止）。
- **contentWindow の同一性**: iframe ごとに安定した Window facade を返すよう変更
  （`document` は動的 getter で再ロードを反映）。プロパティ保持・同一性が成立。
- **URL 解決ロジックの共通化**: `resolve_resource_ref` を抽出し
  `load_iframe_document` と `fetch_script_source` で共有。
- **アンバインド getElementById の堅牢化**: `this` が Document でない場合はメイン文書
  起点にフォールバック。
- **サブ文書の open()/write()/close()**: `document.write` がサブ文書ではなくメイン
  文書へ書き込んでいた配線を修正（`__omoikane_document_write` に対象 document id を
  渡す）。サブ文書書き込みはメイン文書の挿入点を変更しない。

### 残課題（本 issue 外・後続へ）

- **iframe ロードは HTTP ステータスを見ない**: iframe のドキュメントロードはレスポンス
  のステータスコードを検査せず、本文をそのままサブ文書として採用する（実ブラウザは
  エラーページの本文を iframe に描画するため本文採用が妥当）。これはスクリプトロード
  （`fetch_script_source`、200 必須）との意図的な非対称であり、`load_iframe_document`
  にコメントで明記済み。
- **iframe onload イベント**（016-4 連携）: `iframe.onload` は未発火。test 65 で
  `kungFuDeathGrip.title` が蓄積されず test 69 は依然 fail。
- **SVG / XML ドキュメント DOM**（016-14）: svg.xml / empty.xml は空スケルトンを返す
  のみ。`getSVGDocument`・SVG/XML DOM は未実装のため test 69/70/71/72/74/75/77/79/80 は
  未達（回帰はなし）。
- **contentDocument 上の getComputedStyle / defaultView**（016-8）: selectorTest
  (test 33–47) は getTestDocument が通るようになったが getComputedStyle 実値が
  必要。サブ文書の `defaultView` は暫定で globalThis を返す。
