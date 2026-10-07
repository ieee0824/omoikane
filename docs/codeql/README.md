# Rust CodeQL のマクロ解析範囲

Issue [#905](https://github.com/ieee0824/omoikane/issues/905) の再測定手順と、
警告が残る箇所への対応を記録する。CodeQL workflow の成功は、全Rust式の
完全な解析を意味しない。warning の解消と対象式の展開ASTを別々に確認する。

## 測定対象

2026-10-07時点の最新安定版は CodeQL CLI 2.27.1。
[2.27.1の変更履歴](https://codeql.github.com/docs/codeql-overview/codeql-changelog/codeql-cli-2.27.1/)
では Rust extractor 内の rust-analyzer が0.0.347へ更新されている。
このリポジトリの [CodeQL workflow](../../.github/workflows/codeql.yml) は
`github/codeql-action` v4、`build-mode: none` を使用する。

CLIの版だけでなく、完了したmain run、commit SHA、DBの
`creationMetadata.sha` / `cliVersion` / `buildMode` を照合する。
DB download APIは後のmain実行で置き換わるため、runと異なるSHAのDBを
使った比較は採用しない。旧DB・ログを上書きせず、ZIPのSHA-256も保存する。

## 最新mainの再測定

[main run 37577730452](https://github.com/ieee0824/omoikane/actions/runs/37577730452)、
[Rust job 112650326399](https://github.com/ieee0824/omoikane/actions/runs/37577730452/job/112650326399)
は成功。対象SHAは `3462ece60d33e894abb0e01587ae43fc2a4af643`。
DB metadataのSHA、展開したDBのSHA・CLI版・build modeを照合済み。

| 測定 | scanned | extracted without error | extracted with errors |
| --- | ---: | ---: | ---: |
| #896の2026-09-25、`8770715f`、CLI2.27.1 | 914/914 | 908 | 7 |
| 直前main、`6316d61a`、run37570105469、CLI2.27.1 | 1020/1020 | 1014 | 7 |
| 最新main、`3462ece6`、run37577730452、CLI2.27.1 | 1025/1025 | 1019 | 7 |

これらはjob logの別々の指標をそのまま記録したもので、数を足して
scannedの分母と一致するよう補正しない。ファイル数の増加だけでは
対象マクロの解析改善とは判定しない。

最新DBの診断はwarning 18件・info 5件・error 0件。
ファイル・行・列・severity・tag・message・fullMessageで順序を無視して比較し、
直前mainの23診断と一致した。警告対象は以下の7ファイルである。

| ファイル | warning診断数 |
| --- | ---: |
| `engine/boa/core/engine/src/jit/arithmetic.rs` | 8 |
| `engine/boa/core/engine/src/jit/runtime_call.rs` | 1 |
| `engine/boa/core/engine/src/jit/mod.rs` | 1 |
| `engine/boa/core/engine/src/module/loader/mod.rs` | 2 |
| `engine/boa/tests/macros/tests/embedded.rs` | 4 |
| `tests/jit_code_memory.rs` | 1 |
| `engine/boa/tests/src/lib.rs` | 1 |

対象6ファイルのマクロ照会は593結果行で、578行に展開ASTがあり、15行は0。
マクロ展開warningの15位置はすべてAST 0の結果に対応した。
直前mainの結果とも順序を無視して一致した。`Fn!`の外側には9 AST node、
ARM64 assertionの外側には30 nodeがあっても、警告の出た内側の
`__Fn` / `panic_2021`は0である。外側だけの成功を改善と判定しない。
全プロジェクトのマクロ呼び出し総数や全Rust式の解析保証ではない。

共有可能な証跡:

- [run・DB・CSVのハッシュと集計](results/2026-10-07.json)
- [全23診断の相対パスCSV](results/2026-10-07-diagnostics.csv)
- [warning位置の内側マクロと同じ行の外側マクロ](results/2026-10-07-macro-locations.csv)

元DB ZIP、DB内ソース、全593行のCSV、BQRS、job logは
`/tmp/omoikane-issue905/.artifacts/issue905/`に保持した。
警告の解消は確認できておらず、以下の対応表は残る制限に対する具体策である。

同じSHAの [通常CI](https://github.com/ieee0824/omoikane/actions/runs/37577730462)、
[Native JIT](https://github.com/ieee0824/omoikane/actions/runs/37577730360)、
[Browser behavior](https://github.com/ieee0824/omoikane/actions/runs/37577730565)
も成功している。今回の変更は測定記録と照会QLで、Rustソース・Cargo manifest・
workflow・既存テストを変更していない。クエリ2本は最新DBと直前DBで実行し、
pack lock、QL書式、CSVとJSONの値・ハッシュ・リンクを検証した。

## 上流の対応状況

確認日: 2026-10-07。以下は関連する公開議論であり、Omoikaneの各警告と
同じ原因であることや、対象式の修正完了を証明するものではない。

`offset_of`、`panic_2021`、`__Fn`を対象に上流の公開Issueも検索した。
今回のABI検査と`Fn!`型位置を直接解決したという公開の修正済み証拠は
確認できていない。以下の一般的なマクロ警告の議論と、対象DBでの結果を
合わせて判断し、同じ文字列の警告という理由だけで原因を断定しない。

- [マクロ展開警告の説明](https://redirect.github.com/github/codeql/issues/19966)
  はclosed。maintainerの2026-08-04/05の説明では、マクロ展開はサポートされ、
  infoは未コンパイルのコードなどで生じ得る。一方、warningはDBの情報欠落を
  示す可能性があり、残るケースへの対応は進行中とされている。
- [標準マクロの展開失敗の追跡](https://redirect.github.com/github/codeql/issues/19982)
  はopen。2026-09-25に報告者のケースが2.27.1で改善したというコメントが
  あるが、`offset_of!` / `Fn!` / 本リポジトリのARM64テストについての
  修正完了とは扱わない。古いsysrootへ固定する他プロジェクトの回避策も、
  現在のRust APIと解析モデルが変わるため、証拠なしに導入しない。
- [未コンパイルのソースの診断をinfoへ下げる変更](https://redirect.github.com/github/codeql/pull/20288)
  はmerged。今回残るseverity 30の診断を一律にinfoや無視対象へ読み替えない。
- [proc macroの展開コード表示に関する議論](https://redirect.github.com/github/codeql/issues/20659)
  はopen。maintainerは展開ASTの取得と、展開後ソース位置でのalert表示を
  区別している。元ソース位置しか表示できない制限は、展開ASTが欠けている
  ことの証拠ではない。`Fn!`の失敗をこの表示制限で説明しない。

## 箇所ごとの対応

行番号は測定対象SHAのソースに対するもの。通常コンパイルの成功とCodeQLの
展開成功は別の証拠であり、既存のassertion・trait契約・テストは維持する。

| 対象 | 現在の診断・ソースの役割 | 本プロジェクトで取る対応 |
| --- | --- | --- |
| `engine/boa/core/engine/src/jit/arithmetic.rs:64,69,74,79,84,89,94` | `NativeFrame`のABIを検査する7つの`offset_of!`。baseline JITの本体側。 | ABIの`const_assert!`を保持。次のextractor更新で各内側マクロの診断と展開ASTを比較する。残る場合は同じ`repr(C)`構造体と先頭のassertionから再現例を切り出し、CLI・DB診断・AST照会結果を添えて上流へ報告する準備をする。offsetの数値を直接埋めたりassertionを削除して警告を減らさない。 |
| `engine/boa/core/engine/src/jit/runtime_call.rs:222` | trampolineが構造体先頭にあることを検査する`offset_of!`。本体側。 | 同じbuiltinマクロとして上記と一緒に再測定するが、別の型の箇所として記録する。`RuntimeCallFrame`のABI検査を保持し、Native JITのruntime-call契約を引き続き検証する。 |
| `engine/boa/core/engine/src/jit/mod.rs:356` | `#[cfg(test)]`内、さらに`target_arch="aarch64"`限定のassertionから出る`panic_2021`。アプリ本体のファイルにあるが実行式はテスト専用。 | テストを保持し、ARM64のNative JITチェックを使う。x86_64のDBで警告が消えただけではARM64式の解析成功としない。extractor更新時にcfg・severity・展開ASTを確認し、未コンパイル枝に対する診断なら上流のseverity処理の再現例として扱う。 |
| `engine/boa/core/engine/src/module/loader/mod.rs:214,234` | `DynModuleLoader`のtraitとimplの戻り値型を生成する`Fn!`内部の`__Fn`失敗。本体側。 | 生成型の借用・Future契約を保持する。次版でtrait/implの両方の型位置のASTを照合し、残る場合は依存クレート版と同じマクロ呼び出しを持つ小さなtrait/implの再現例を用意する。マクロを別の型へ書き換える場合は両位置の型情報と既存module-loaderテストを検証してから判断する。 |

過去のIssue本文の「arithmeticの`offset_of!`8箇所」は、7つのABI検査と
ARM64専用テストの`panic_2021`1件が合算されていた。`jit/mod.rs`の箇所も
本体の処理ではなくテスト内である。ファイルが本体ツリーにあることと、
診断対象式が本体の実行経路にあることを区別する。

テスト・manifest外の残る診断も件数から除かず記録する。

- `jit/arithmetic.rs`のARM64専用snapshotテストのassertionは、上記
  `jit/mod.rs`と同じ手順で追跡する。ソース追加で行番号が変わっても、
  関数とマクロ内容で旧結果に対応付ける。
- `tests/jit_code_memory.rs:53`のARM64 assertionもテストを保持し、
  Native JITのARM64結果と診断を分けて確認する。
- `engine/boa/tests/macros/tests/embedded.rs:67,77`の`embed_module!`は正常系。
  [#898](https://github.com/ieee0824/omoikane/issues/898)でfixtureと2テストの
  成功を確認済み。次の再測定でもfixtureの存在と展開ASTを確認する。
  `compile_error`という文字列だけで意図的なコンパイル失敗と分類しない。
- `engine/boa/tests/src/lib.rs`はworkspaceから除外されたテスト用クレート。
  manifest外という診断を保持し、警告数を減らすためだけにworkspaceへ追加しない。
  アプリ本体のマクロ式欠落と同一視しない。

## 再測定

必要なもの: `gh`、測定DBと同じ版のCodeQL CLI、ZIP展開ツール。
クエリの依存は [qlpack.yml](queries/qlpack.yml) とlockで固定する。
新しいCLIへ移行するときは同梱のRust libraryの版も比較記録する。

1. mainのSHAを取得し、Actions APIを`head_sha`と`event=push`で絞って
   CodeQL runとRust jobの**完了成功**を確認する。run一覧の順番だけで
   対象を決めず、Rust jobがskipされたworkflowもDBの再生成とは扱わない。
   実行中やcancelledのrunを完了として扱わない。最新mainに対応するDBが
   なければ待つか、測定した旧SHAを明記し、最新mainの解析結果とは記載しない。
2. DB metadataとZIPを取得し、metadataの`commit_oid`、展開後の
   `codeql-database.yml`のSHAとrunのSHAを照合する。
3. 以下のクエリを**同じDBに対して直列に**実行する。CodeQLのDBキャッシュは
   並列のquery evaluatorで共有できない。エラー時は終了を確認して再実行する。

```sh
# 一覧を取得したあとCodeQL runとRust jobを確認し、RUN_ID等を設定する。
MAIN_SHA=$(gh api repos/ieee0824/omoikane/branches/main --jq .commit.sha)
gh api --method GET repos/ieee0824/omoikane/actions/runs \
  -f head_sha="$MAIN_SHA" -f event=push -f per_page=100 \
  --jq '.workflow_runs[] | {id,name,status,conclusion,head_sha}'
# RUN_ID / RUST_JOB_IDは同じ完了済みmain runの値、OUTは新しい保存先。
mkdir -p "$OUT"
gh run view "$RUN_ID" --job "$RUST_JOB_ID" --log > "$OUT/rust-job.log"
gh api repos/ieee0824/omoikane/code-scanning/codeql/databases/rust \
  > "$OUT/database-metadata.json"
gh api -H 'Accept: application/zip' \
  repos/ieee0824/omoikane/code-scanning/codeql/databases/rust \
  > "$OUT/database.zip"
sha256sum "$OUT/database.zip" > "$OUT/database.zip.sha256"
unzip "$OUT/database.zip" -d "$OUT/database"
codeql pack ci docs/codeql/queries
codeql query run --database="$OUT/database/rust" --threads=2 --ram=2048 \
  --output="$OUT/diagnostics.bqrs" docs/codeql/queries/RawDiagnostics.ql
codeql bqrs decode --format=csv --output="$OUT/diagnostics.csv" \
  "$OUT/diagnostics.bqrs"
codeql query run --database="$OUT/database/rust" --threads=2 --ram=2048 \
  --output="$OUT/macros.bqrs" docs/codeql/queries/MacroCoverage.ql
codeql bqrs decode --format=csv --output="$OUT/macros.csv" "$OUT/macros.bqrs"
```

4. severity 20(info)、30(warning)、40(error)を分け、全診断のファイル・位置・
   内容を比較する。warning件数だけでなく、対象内側マクロの展開ASTと
   呼び出し位置を確認する。外側マクロが一部展開できたことは、内側の式の
   解析成功を証明しない。ASTがあることも型・dataflowを含む完全解析の保証ではない。
5. 上流Issueのopen/closed、コメント、CLI変更履歴を更新確認する。
   診断が残る場合は対応表の具体策を引き継ぐ。ソースやworkflowを変更した場合は、
   通常のlocked Rustテストと影響するfeatureの契約テストを実行し、
   変更前後のCodeQL DBを同じクエリで比較する。

DB、元CSV、BQRS、job logはローカル証跡として保持する。共有する小さな結果は
runnerの絶対パスをリポジトリ相対へ正規化し、元データのハッシュ・run・SHA・
CLI版を記録する。認証情報や思考過程は含めない。
