---
number: 016-5
slug: data-uri-scripts
parent: 016
status: closed
---

# data: URI スクリプト対応

## 目的

`<script src="data:...">` をサポートし、`data:` スキームのスクリプトソースを取得できるようにする。

## 背景（GAP_ANALYSIS.md test 97、セクション3 領域N）

- `fetch_script_source`（`src/js/mod.rs`）が `data:` スキーム非対応のため、
  Acid3 の d1〜d5（`<script src="data:text/javascript,...">`）が全て fetch 失敗する。
- 実測でも `[0]`〜`[4]` の "failed to fetch script: data:..." が残っている。
- Acid3 の d1〜d5 が実テストベクタであり、test 97 に直結する。

## スコープ

- `data:` スキームのパース（`fetch_script_source`）
  - percent-decoding（`d1`, `d5`）
  - base64（`d2`, `d3`）
  - base64 + 空白/改行混入の許容（`d4`）
- 併せて画像等の data: 取得も視野に入れる（本issueの主眼はスクリプト）

## 受け入れ条件

- d1〜d5 の 5 パターンが正しくデコード・実行される
- Acid3 test 97 が data: 系で fail しなくなる

## 完了メモ

- **HTTP 層に汎用 `data:` URI パーサを新設** (`src/http/data_uri.rs`,
  `parse_data_uri` / `DataUri`)。スキーム大小無視、`,` で metadata/data 分離、
  data 部を先に percent-decode（d3 のように data 部全体が percent-encode された
  ケースに対応）、`;base64` 時は ASCII 空白（改行含む）を除去してから base64 デコード
  （d4 の空白混入に対応）。RFC 2397 準拠。画像等でも再利用可能な設計。
- **`fetch_script_source`** (`src/js/mod.rs`) が `data:` を分岐し、
  JavaScript MIME type essence（`text/javascript` 等、HTML 仕様の一覧）のみ実行。
  非 JS mediatype は `None`（フェッチ失敗扱い）とし、doc コメントに記録。
- d1〜d5 の 5 ベクタをユニットテストで網羅（デコード結果 + end-to-end で
  d1..d5 グローバル定義を確認）。

### 設計判断 / スコープ外
- HTTP `Client::get` の `data:` ルーティングは今回入れず、スクリプト取得経路
  (`fetch_script_source`) のみで消費。パーサ自体は http 層の公開 API なので、
  画像経路の統合は将来対応可能（既存の `src/paint/image.rs::parse_data_uri` は
  空白非許容のため本パーサへ寄せる余地あり）。過剰設計回避のため今回は非統合。
- test 97=XHR の `data:`（016-14）は対象外。
- Acid3 スコア: 27 → 28（test 97 PASS）。document script errors は 6 → 1
  （残 1 は `document.write` 未実装 = 016-7 スコープ）。
