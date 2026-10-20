# リソース取得の共通queue/worker

画像・フォント・スクリプト・Fetch/XHRの通信を、それぞれ専用の無制限なスレッドで実行すると、同時取得数を制御できない。一方、ページを所有するスレッドで通信を待つと、入力や描画まで止まる。

[`ResourceQueue`](../src/resource_worker.rs)は、所有したデータだけを受け取るジョブと型付きの完了ハンドルを提供する。取得処理はworkerへ渡し、DOM・JavaScript・描画状態の更新は所有者へ戻す。

## 所有境界

1. 所有者がURL・基準URL・要求元・Cookieの要求コンテキスト・セキュリティ設定などを確定する。
2. `submit`へ`Send + 'static`な入力を渡す。workerへDOMハンドルやBoaの値を渡さない。
3. workerが通信やデコードを実行し、所有する結果を返す。
4. 所有者が`try_recv`で完了を取り出し、Documentの生存、適用時のポリシー、実行Realmを確認する。JSの完了通知は対応するイベントループのタスクとして配送する。

ネットワーク処理中は、DOMやランタイム状態の借用・ロックを保持しない。Cookie jarなど、明示的に共有する状態の同期は通信処理と区別する。Fetchのpreflightキャッシュはworkerへスナップショットを渡し、完了時に有効期限を確認して併合する。

## queueの契約

| API | 契約 |
| --- | --- |
| `ResourceQueue::new` | 並列数を指定する。最初の投入までworkerを起動しない。既定は4。 |
| `ResourceQueue::clone` | 同じworker群と並列数の上限を共有する。 |
| `submit` | `ResourcePriority`、`ResourceKind`、scope、所有する処理を受け取り、`ResourceTask<T>`を返す。 |
| `ResourceTask::try_recv` | 呼び出し側で処理を実行せず、完了を非同期に確認する。 |
| `ResourceTask::recv` | 同期待機。非対話的な利用やテスト向け。GUIの所有者では使わない。 |
| `ResourceScope` | 寿命を管理する所有者。破棄または`cancel`でキャンセルする。 |
| `CancellationToken` | scopeを延命せず、workerからキャンセルを確認する。 |

優先度は`Critical`、`Normal`、`Background`の順。同じ優先度では投入順を保つ。実行中のジョブを優先度によって中断する機能や、低優先度ジョブの待ち時間の上限はない。

上限は**共有するqueue単位**であり、プロセス全体の上限ではない。独立したqueueは別々のworker群を持つ。待ちジョブ数は無制限なので、呼び出し側が重複取得や不要な投入を抑える必要がある。

`CdpSession::set_resource_worker_count`は並列数を設定する。top-level navigationの
replacement runtimeには同じ並列数で新しいqueueを作り、退役したページの遅い通信が
次のページのworker枠を塞がないようにする。ひとつの世代ではiframe Documentと
Dedicated/SharedWorkerもqueueを共有する。

GUIでは`OMOIKANE_RESOURCE_WORKERS`へ1から64の整数を指定できる。既定は4で、
不正値は既定値へ戻る。`CdpSession::set_resource_queue`はnavigationをまたぐ共有の
明示的な選択肢であり、退役後も実行中の通信が占めるworker枠も共有する。
並列数の効果は同一workloadで計測し、ページ全体の改善とは区別する。

## キャンセルと終了

Documentの退役時には、そのDocumentが持つ未完了要求を削除してscopeを破棄する。完了済みでも、退役したDocumentへの通知は配送しない。Dedicated/SharedWorkerは作成元のqueueと非同期取得の設定を引き継ぐ。

接続元ページの`RenderDemand`には、SharedWorkerの通信待ちと次のタスク期限も反映する。ページ時間と壁時計の期限は区別し、通信完了後のタイマーも常時ポーリングに置き換えない。閉じた接続は対象から外し、同じ問い合わせ中に訪問済みのランタイムを記録して循環参照による再帰を防ぐ。

キャンセル済みの待ちジョブは実行しない。実行中の処理は協調的なキャンセルであり、ブロック中のI/Oを強制停止しない。取得アダプターは処理の区切りでtokenを確認できるが、即時停止を保証してはならない。通信自体のタイムアウトも引き続き必要になる。

最後のqueueハンドルの破棄では、待ちジョブをキャンセルしてworkerへ終了を通知する。実行中のI/OをjoinしてGUIを待たせない。ジョブのpanicは完了結果`Panicked`になり、workerは後続ジョブを処理する。

## 利用経路と残る範囲

GUIは画像・フォント・動的な外部スクリプト・Fetch/非同期XHRのworker経路を有効にする。ライブラリ利用では`JsRuntime::set_nonblocking_*_loading`で選択し、`set_resource_queue`で共有するqueueを指定できる。通信結果の適用とスクリプトの実行順序は、各アダプター側が管理する。

この仕組みだけでは、初期のparser経由のスクリプト取得、CSSの取得・import、子Documentの取得などに残る同期待機は解消しない。module graphは別の固定4-worker poolを使う。JS実行・GC・スタイル計算・レイアウトも所有者側に残る。通信の並列化を、ページ全体の表示速度やフレームレート改善の保証として扱わない。

## 検証の分担

- [queueのテスト](../src/resource_worker/tests.rs)：異なるリソース種別間の並列数、優先度とFIFO、キャンセル、panic、終了時の非待機。
- [Fetch/XHRのテスト](../src/js/fetch_loading/async_fetch/tests.rs)：独立した完了、タイマー・GC・Realm、キャンセル、Document退役、Cookie、CSP、Workerとの共有上限。
- [GUIのHTTP待機テスト](../scripts/test-gui-fetch-loading.py)：複数の応答を保留したまま、ホイール入力と描画が進むこと。

実サイトの起動時間やスクロール停止は、これらの契約テストとは別に計測する。
