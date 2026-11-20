# ローカルの補助テスト診断

`scripts/local-test-diagnostics.py` は、Gate 5で定義されている8件をexact指定の直列実行へ分け、残りを通常のlibtest並列数で実行する補助ツールです。各テストの上限とassertionは変更しません。[AGENTS.mdの必須検証](../AGENTS.md#検証)と既存CIを引き続き実施してください。補助側の成功で、既定並列の不具合が修正されたとは判断できません。

Python 3.11以上とCargo、Gitを使います。対象はこのリポジトリのroot packageと、ローカルで直接実行できるnative binaryです。cross targetやCargoのcustom runnerを使う実行は対象外です。

## 実行例

作業するworktreeのrootから実行します。Linux sandboxでは、共有resource queueを取り、worktree専用のext4 targetを指定します。

```sh
CI=1 CARGO_TARGET_DIR=/target/issue1688-local-test-diagnostics \
  flock -x /tmp/omoikane-resource-queue.lock \
  python3 scripts/local-test-diagnostics.py \
  /workspace/.artifacts/issue1688-local-diagnostic/run-01
```

別環境では、そのhostに設定された共有queueを使ってください。ツールは既存の容量guardを内部で起動します。queueの取得は実行する側の責務です。

出力先は毎回新しいディレクトリにし、Cargo targetの外へ置きます。既存の出力先は再利用しません。リポジトリ内に置く場合はgitignore対象にしてください。既定並列で失敗した元ログがあれば、`--baseline-log /path/to/default-test.log` でコピーとSHA-256を保持できます。

次のオプションを使用できます。

| オプション | 動作 |
| --- | --- |
| `--plan-only` | compileと一覧取得・集合照合まで。テスト本体とdoctestは実行しない |
| `--include-ignored` | ignoredも実行する。無指定ではignoredを記録して実行対象から外す |
| `--features gc-profile` | Cargoへ明示的に渡すfeature。無指定では追加featureを有効化しない |
| `--required-free-gib` | 容量guardの開始時空き容量。既定22 GiB |
| `--minimum-free-gib` | 容量guardの空き容量下限。既定2 GiB |
| `--maximum-target-gib` | Cargo targetの容量上限。既定40 GiB |

guardの開始条件は、開始時空き容量と「下限＋targetが上限まで増える余地」の大きい方です。新しいtargetで既定設定を使う場合は42 GiBの空きが必要です。容量不足時は元ログと停止理由を確認し、リポジトリのguardによるcache整理手順に従ってください。

## 選択と証跡

`cargo test --locked --no-run --message-format=json` からroot packageのtest executableを取得し、同じbinaryで全件とignoredの一覧を取ります。各binaryのSHA-256と解決済みfeaturesを記録し、実行直前にも照合します。既存[Gate 5の定義](../scripts/jit-gate5.py)を読み取るため、選択定義を二重管理せず、Gate 5本体やそのcache keyも変更しません。

並列本体の`--skip`は部分一致、直列群は`--exact`です。除外集合と直列集合の一致、重複・欠落の不在、exact対象が1件であることを事前に確認します。未知の一覧形式やbenchmarkは、黙って省略せずpreflightで失敗します。

通常のテスト失敗後も残りの群とdoctestを実行し、最終exitを非0にします。クラッシュ・ケース欠落・summary不整合も成功にしません。doctestは別のCargo invocationとして記録します。

ツール自身の予期しない例外では中断し、後続を未実行として残します。tracebackはguardのstderrログで確認してください。

各processのstdoutとstderrを分けて保存し、結果照合にはstdoutだけを使います。`manifest.json`にはargv、exit、ログのSHA-256、実行予定と結果、ignored方針、source/lockfileの実行前後のidentityを保存します。容量guardが停止した場合は`resource-blocked`、実行中だった群は`incomplete`、後続は`not-run`として残ります。guardの詳細は`guard.stdout.log`と`guard.stderr.log`で確認できます。

| 最終status | 意味 |
| --- | --- |
| `passed` | 補助実行のlibtest群とdoctest invocationが成功し、source identityも一致 |
| `planned` | `--plan-only`の照合完了。本体は未実行 |
| `failed` | テスト失敗、不完全な結果、入力・source変更、guard失敗など |
| `resource-blocked` | 容量guardが容量不足・使用中targetなどで停止 |
| `interrupted` | guardがterminal interruptで終了 |

外側のguard終了確認前は成功を確定しません。`running`や`worker_status`だけが残った場合も、全体の成功として扱わないでください。

Ctrl-Cを受けた場合もguardの終了を待ち、実際の`guard_exit`と外側の`interrupted`を別々に記録します。signalによるguardの負のreturncodeはmanifestに保持し、ツール自体は正の失敗exitを返します。

## WPTの範囲

WPTを自動取得したり、`WPT_REQUIRED`を暗黙に変更したりしません。`WPT_ROOT`、`resources/testharness.js`の有無、manifestのhash、pinned revision、取得可能ならcheckout revisionを記録します。`WPT_REQUIRED`は値が`0`や空文字でも、環境変数が存在すれば必須扱いです。

checkoutがないと既存WPT集約テストはearly returnすることがあります。そのlibtestの`ok`を上流WPT実行済みと解釈しないでください。このツールは上流WPTレポートの内容を検証しません。必要なWPT検証は既存の取得・実行手順で行い、結果を別途確認します。
