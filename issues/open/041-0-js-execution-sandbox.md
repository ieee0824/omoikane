---
number: 041-0
slug: js-execution-sandbox
parent: 041-javascript-engine-integration
status: open
---

# JS 実行サンドボックス

## 概要

外部サイトの JS を安全に実行するためのサンドボックス機構を実装する。041-1〜041-4 の前提として先に実装する。

## 制限項目

### 実行時間制限
- スクリプト実行のタイムアウト（デフォルト 5秒）
- 無限ループ・再帰の検出と強制停止
- boa_engine の `OpcodeLimiter` または定期的な割り込みチェック

### メモリ制限
- JS ヒープの上限設定（デフォルト 64MB）
- 超過時に RangeError をスロー

### ネットワーク制限
- `fetch` は same-origin のみ（既存の SSRF 保護を活用）
- WebSocket 禁止
- data: URI のみ許可（外部リクエスト制限モード）

### ホスト API 制限
- 公開するのは DOM 操作 API のみ
- `eval()` は許可（CSS/JSON パース用途）
- `Function()` コンストラクタは許可（ライブラリ互換性）
- ファイルシステムアクセスなし
- プロセス操作なし（`process`, `require` 等を未定義）

### エラー隔離
- スクリプトエラーがホスト（Rust 側）をクラッシュさせない
- `JsError` を catch してログに記録、レンダリングは続行
- 複数 `<script>` の1つが失敗しても他は実行継続

## 実装方針

1. `SandboxConfig` 構造体でタイムアウト等の設定を保持
2. `eval_safe()` でエラーを catch して続行可能に
3. `process`/`require` 等のホスト API が undefined であることを保証
4. タイムアウト強制停止: boa 0.21 にはランタイム割り込み API がないため、
   将来バージョンで `can_execute` 等が追加された場合に対応予定
5. 現時点の無限ループ対策: `run_until_idle` に呼び出し回数上限を設定

## 受け入れ条件

- `while(true){}` がタイムアウトで停止する
- スクリプトエラーがホストを crash させない
- `fetch('http://evil.com')` が same-origin チェックでブロックされる
- 未定義のホスト API（`process`, `require`）にアクセスすると ReferenceError

## 修正箇所

- `src/js/mod.rs` — `JsSandbox` 構造体、タイムアウト・エラーハンドリング
