---
number: 016-3
slug: timer-callbacks-event-loop
parent: 016
status: open
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
