# 一般エラーの記録と送信

Omoikaneの一般エラーreporterは既定で無効です。GUI版は起動時に環境変数を読み、JavaScript、CSS、HTTP、layout、描画、CDP、GUIなどの失敗を固定コードで分類します。Rustから利用する場合は`ReporterConfig`と`ErrorReporter`を作り、対象の`JsRuntime`、`CdpSession`、`Client`などへ設定します。未対応CSS/HTMLの観測ログは別の設定とテーブルを使います。

## 設定と保存先

| `OMOIKANE_ERROR_REPORT_MODE` | 動作 |
| --- | --- |
| 未設定、`off` | DBを作らず、GitHubへ送信しない |
| `record-only` | ローカルSQLiteへ記録し、送信しない |
| `submit` | ローカルへ記録し、明示したrepositoryへの送信を許可する。これだけでは自動送信しない |

GUIでローカル記録を始める例です。

```sh
OMOIKANE_ERROR_REPORT_MODE=record-only \
OMOIKANE_ERROR_REPORT_DB=/path/to/private/error-reports.sqlite \
  cargo run --locked --features gui --bin omoikane -- https://example.com/
```

`OMOIKANE_ERROR_REPORT_DB`を省略すると、OSを問わず`$XDG_STATE_HOME/omoikane/error-reports.sqlite`を優先します。`XDG_STATE_HOME`がなければ、Linuxでは`$HOME/.local/state/omoikane/error-reports.sqlite`、macOSでは`$HOME/Library/Application Support/omoikane/error-reports.sqlite`を使います。保存先はGUI起動時に決まり、`off`では開きません。ディレクトリを含めて閲覧権限を管理してください。無効化するにはmodeを`off`に戻してプロセスを再起動します。既存のDBは自動削除されません。

`submit`には`OMOIKANE_ERROR_REPORT_REPOSITORY=owner/repo`が必須です。URLや暗黙の既定送信先は受け付けません。不正な設定は起動時に拒否され、入力文字列をエラーへ転載しません。`OMOIKANE_ERROR_REPORT_AUTO_SUBMIT`は未設定または`0`なら無効、`1`だけが有効です。`1`は`submit`と明示repositoryの両方がある場合に限ります。

## SQLiteと秘匿化

一般エラーDBはSQLiteのWALモードを使います。schema version 2の`error_reports`は、秘匿化後のfingerprintを主キーに、category、severity、固定error code、定型message、許可リスト内のcontext JSON、初回・最終発生時刻、発生回数、version、検証済みcommit、platform、execution surface、送信状態、Issue URL、再試行状態と配送leaseを保持します。同じfingerprintは行を増やさず発生回数を更新します。version 1の同じDBは開くとversion 2へ移行します。異なるschemaのDBは流用しません。

| 列 | 内容 |
| --- | --- |
| `fingerprint` | 秘匿化後に計算した主キー |
| `category`, `severity`, `error_code`, `message`, `context_json` | 定型分類と許可リスト内の内容 |
| `first_seen_ms`, `last_seen_ms`, `occurrences` | Unix時刻のミリ秒値と集約回数 |
| `version`, `build_commit`, `platform`, `surface` | 発生したビルドと実行面 |
| `submission_status`, `issue_url` | `pending`/`submitted`と送信済みIssue |
| `delivery_failures`, `next_retry_ms`, `last_submitted_ms`, `delivery_lease_until_ms` | 再試行・cooldown・重複配送抑止 |

既定の保存上限は10,000 fingerprint、最終発生から30日、DB・WAL・SHM合計64 MiBです。期限切れを除き、件数・容量の削減では古い送信済み行を先に整理します。上限維持のため未送信行も消える場合があるので、DBを送信保証のキューとして扱わないでください。

URLのquery/fragment、Cookie、Authorization、通信本文、DOM本文、JavaScriptソース、画像データ、ローカル絶対パス、環境変数、tokenはイベントに含めません。任意のメッセージとcontextをそのまま保存せず、定型messageと許可リストの分類値へ変換した後にfingerprintを計算します。保存・送信前にプレビューするのは、この変換後の`StoredReport`です。GitHub Issueへ送れば、そのrepositoryの公開範囲に応じて内容が見えるため、送信先とプレビューを確認してください。

## 手動送信

現時点で手動送信の専用CLIはありません。Rustの利用側は`submit`設定でDBを`EventStore::open`し、`ManualSubmission::new(&config, &store)`を作ります。`list_pending(limit)`で候補を列挙し、選んだfingerprintを`preview`してから利用者に表示します。利用者がその1件を承認した後だけ、`GitHubRestApi::new(token)`を`GitHubIssueBackend::new(api)`へ渡し、`submit_selected(fingerprint, SubmissionApproval::Confirmed, &mut backend)`を呼びます。`Unconfirmed`は送信されません。tokenは呼び出し元が安全な方法で取得し、DBやログへ書かないでください。

GitHub backendは`api.github.com`の対象repositoryのopen Issueを調べ、同じfingerprintの報告ブロックがあれば更新し、なければ新規Issueを作ります。open Pull Requestは再利用対象から除きます。検索が完了しない場合は新規Issueを作らず失敗にします。成功したIssue URLはDBへ残ります。

手動・自動とも、既定の配送上限はプロセス実行あたり10回、同じfingerprintの成功後cooldownは1時間です。ネットワーク失敗は1分から最大24時間の指数backoff、rate limitは応答の再試行時刻、認証失敗は待機扱いです。連続8回失敗した行は、原因を修正した後に`EventStore::reset_submission_retry(fingerprint)`で明示的に解除できます。別プロセスとの重複送信を避けるため30分のSQLite leaseを使います。

## 自動送信

自動送信は次の3条件をすべて満たす場合だけ開始します。

1. `OMOIKANE_ERROR_REPORT_MODE=submit`
2. `OMOIKANE_ERROR_REPORT_REPOSITORY=owner/repo`
3. `OMOIKANE_ERROR_REPORT_AUTO_SUBMIT=1`

さらに専用の`OMOIKANE_ERROR_REPORT_GITHUB_TOKEN`が必要です。tokenはプロセスの秘密設定から渡してください。欠けていてもローカル記録は続き、送信ワーカーだけ起動しません。ワーカーは起動時と約60秒ごとに未送信行を調べ、手動と同じ上限、cooldown、再試行、leaseを使います。実際のGitHubへの送信はこの明示opt-in時だけです。通信中の終了でもブラウザの終了待ちを長引かせません。

## 検証の対応

| 経路 | 回帰テスト |
| --- | --- |
| JS document scriptとoff | `tests/error_reporting_document_script.rs` |
| timer、event listener、animation task | `tests/error_reporting_task_errors.rs` |
| worker、module | `tests/error_reporting_worker_module.rs` |
| CSS取得・解析 | `tests/error_reporting_stylesheet.rs` |
| HTTP transport、redirect、resource | `tests/error_reporting_http.rs` |
| layoutの画像decodeと描画・撮影 | `src/layout/tests.rs`、`src/screenshot/mod.rs` |
| CDP、panic | `tests/error_reporting_cdp.rs`、`tests/error_reporting_panic.rs` |
| SQLite集約・保持上限・migration | `src/error_reporting/store_tests.rs` |
| GitHub Issue再利用、mock送信、rate limit、認証失敗、retry、自動送信 | `src/error_reporting/github.rs`、`src/error_reporting/manual.rs`、`src/error_reporting/auto.rs` |

送信テストはmock backend/APIを使い、実GitHubへ通信しません。通常の`cargo test --locked`で実行されます。関連テストは、reporterの記録や送信の失敗を元のブラウザ操作から分離することも確認します。
