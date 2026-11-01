# 通常CIのテスト分割と容量証跡

Issue #1640では通常CIのtest step中にrunner自身の診断ログも書けなくなった。
失敗側の容量内訳は失われているため、特定のcache・font・targetだけを原因とは
断定しない。通常CIを分割し、runnerの空き容量を残すguardと段階記録を加える。
Gate4の分割（#1629）やActionsへのcache保存予算（#1451）とは別の経路である。

## 実行範囲

`scripts/ci-test-partitions.py` は `cargo metadata --locked --no-deps` の
通常featureで有効な対象から、次の実行計画を作る。

| partition | 対象 |
| --- | --- |
| `unit-build` | library・有効なbinary・`test=true` exampleのテスト、compile-only example、doc test、Gate2、locked build |
| `integration-0` | 統合テスト名のSHA-256による固定分割の前半 |
| `integration-1` | 同じ分割の後半 |

統合テストの追加で既存の対象は別partitionへ移らない。新しいtargetはmetadataから
自動で加わる。GUIなどrequired-featureが通常実行で有効でないtargetは除外理由を
記録する。対応していない新しい種類を黙って無視しない。

通常の`cargo test`はexampleをコンパイルするが、`test=false`のexampleを
テストharnessとして実行しない。この区別を保つため、compile-only exampleは
`cargo build --locked --profile test --example ...` で検査する。
[Cargoの対象選択](https://doc.rust-lang.org/cargo/commands/cargo-test.html#target-selection)を参照。

以前の通常CIと同じくignored testを含め、既存の手動計測・baseline更新utility
12件だけを同じ名前で除外する。Gate2のfeature指定と`cargo build --locked`も
維持する。固定WPTを取得して実行する専用jobの条件には変更を加えない。

## 完全性と再試行

prepareでrevision・Rust toolchain・lockfile hash・tracked diff hash・対象一覧・
全コマンドを保存する。各partitionと最終`test` jobは同じ入力であることを照合する。
終了コード0だけで成功とはせず、`Running`と各harnessの成功summaryを対象ごとに
照合する。compile-only exampleは非harnessのcompiler artifactとbuild完了を照合する。
最終jobは全ログを再検査し、欠落・重複・別対象・別source・未完了を拒否する。

既存の`test` checkは全partitionの集約結果になる。matrixの一部が失敗しても
他のpartitionを中止せず、集約jobは失敗時にも証跡を収集する。benchmark reportは
全partitionに1件だけ存在することを確認し、既存のreport validatorも実行する。

入力・partition・最終reportのartifact名にはrun attemptを付ける。再試行では
`select-gate-artifacts.py`で各論理artifactの最新attemptを選ぶ。古い成功partitionを
再利用する場合もsource identityとcoverageを再検査し、失敗した旧attemptのartifactは
消さない。sourceが異なる証跡を組み合わせることはできない。

## 容量とartifact

各Cargoコマンドを容量guardで包み、target上限10 GiB・空き2 GiBを設定する。
小さなローカルfixtureでは上限を下げられるが、10 GiBを超えて緩めることはできない。
実CIでの完走と容量の確認は、この構成を導入しただけでは完了扱いにしない。

cache復元前後・font導入後・partition終了時にはfilesystemとtarget/Cargo homeの
容量を保存する。各実行段階の前後にもtargetと空き容量を記録する。`du`は10秒で
打ち切り、その場合は測定不能と記録する。テストやJSの期限は緩和しない。

`normal-test-part-<partition>-attempt-N`にはそのpartitionのログ、source入力、
容量記録、browser report、painted frame、benchmark reportを保存する。
最終browser artifactの論理名は従来と同じ
`browser-x86_64-unknown-linux-gnu-interpreter`で、末尾に`-attempt-N`を加える。
中身はpartition別のディレクトリで、元ログや画像を上書きして混ぜない。
JS benchmark artifactも`js-benchmark-report-attempt-N`になる。

cacheは従来の`workspace-v2`を復元し、mainの`unit-build`だけが保存する。
統合テストpartitionは保存主体を増やさない。
成功した旧通常CIと同じ`CARGO_INCREMENTAL=0`をjob側でも明示し、容量記録に含める。
旧cacheにはguardのworktree bindingが無い場合があるため、復元前の空targetを
guardで初期化する。rust-cacheはpartial hit時の整理でbindingも消すため、cache外へ
保存した自分のbindingだけを戻す。別target・別worktree・別UIDや既存の異なるbindingを
上書きせず、復元後もguardの所有者・worktree・容量検査を必ず再実行する。
[cache側の整理処理](https://github.com/Swatinem/rust-cache/blob/v2/src/cleanup.ts)を参照。

## ローカル検証

```sh
python3 -m unittest discover -s scripts/tests -p 'test_ci_test_partitions.py'
python3 -m unittest discover -s scripts/tests -p 'test_select_gate_artifacts.py'
python3 scripts/ci-test-partitions.py plan
```

実際のCargoを含む検証は、worktree専用のext4 targetと共通resource queueで直列化する。
rootとnested Boa workspaceのtargetは共有しない。
