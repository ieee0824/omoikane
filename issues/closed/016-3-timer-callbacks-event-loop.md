---
number: 016-3
slug: timer-callbacks-event-loop
parent: 016
status: closed
---

# setTimeout/setInterval の関数コールバック保持とイベントループ統合

## 目的

`setTimeout` / `setInterval` に渡された関数コールバックを保持・再実行できるようにし、
レンダリングパイプラインがイベントループを仮想時間で駆動できるようにする。

## 背景（GAP_ANALYSIS.md セクション1 P2a/P2b、セクション4 フェーズ0）

- 現状 `schedule_timer_from_js`（`src/js/mod.rs`）は関数引数を `to_string()` して
  ソース文字列として保存するため、後で eval してもクロージャが失われ呼び出されない。
- `execute_document_scripts` 後に `tick()` が呼ばれずランタイムも drop されるため、
  マクロタスクが消化されない。
- Acid3 ドライバの `update()` は `setTimeout(update, 10)` の再帰チェーンで回るため、
  これが無いと Faithful モードで 1〜数回で停止する（DirectDrive では回避済み）。

## スコープ

- `setTimeout(fn, ms)` / `setInterval` の関数コールバックを JsValue のまま保持
- 仮想時間での消化（`tick(ms)` / `run_until_idle`）とランタイム永続化
- レンダリングパイプラインへの統合

## 受け入れ条件

- 関数コールバックを渡した `setTimeout` が指定遅延後に実行される
- Acid3 Faithful モードで `update()` チェーンが自走し、DirectDrive と同等まで index が進む

## 完了メモ

- `TimerTask` のペイロードを `TimerPayload { Source(String) | Callback { callback: JsValue, args: Vec<JsValue> } }`
  の両対応に変更。関数は `JsValue` ハンドルとしてそのまま保持し（クロージャ scope を保存）、
  発火時に `callable.call(&this, &args, context)` で呼ぶ。`setTimeout(fn, ms, arg1, ...)` の
  追加引数も透過。文字列ソース（`setTimeout("code", ms)`）の従来経路は維持。
- `advance()` は発火時刻順（同時刻は登録順）で `macrotasks` に積む。再スケジュールと
  clearTimeout/clearInterval が正しく動作。
- レンダリングパイプライン（`paint::render_document_with_url`）で `execute_document_scripts`
  実行後にランタイムを保持したまま `run_timers()` でタイマーキューを仮想時間消化。
  上限: 仮想時間 10,000ms / step 10ms / タスク数 100,000。
- 検証: `cargo test` 全パス（lib 810 + harness 4 + 9）。`cargo run --example acid3` で
  Faithful モードが index 1 → 100 まで自走し、DirectDrive と同点の 26/100 を達成
  （従来 Faithful は 0/100・index 1 で停止）。
