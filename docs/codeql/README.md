# Rust CodeQL のマクロ解析範囲

Issue [#905](https://github.com/ieee0824/omoikane/issues/905) の再測定手順と、
警告が残る箇所への対応を記録する。CodeQL workflow の成功は、全Rust式の
完全な解析を意味しない。warning の解消と対象式の展開ASTを別々に確認する。

## 測定対象

以下の基準測定には CodeQL CLI 2.27.1を使用した。
[2.27.1の変更履歴](https://codeql.github.com/docs/codeql-overview/codeql-changelog/codeql-cli-2.27.1/)
では Rust extractor 内の rust-analyzer が0.0.347へ更新されている。
このリポジトリの [CodeQL workflow](../../.github/workflows/codeql.yml) は
`github/codeql-action` v4、`build-mode: none` を使用する。

2026-10-07公開のCLI 2.27.2への更新では、workflowの`tools`を
[公式2.27.2 Linux bundle](https://github.com/github/codeql-action/releases/tag/codeql-bundle-v2.27.2)
のURLに固定する。再測定用query packも公開版`codeql/rust-all` 0.2.23に更新し、
推移依存の版を[lockfile](queries/codeql-pack.lock.yml)に保存する。
2.27.1の測定記録は比較基準として保持し、2.27.2で生成されたDBの結果とは区別する。

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

### Issue #1266の再現例と追跡

[Issue #1266](https://github.com/ieee0824/omoikane/issues/1266)では、残る欠落を
[独立した再現crate](repros/macro-gaps/README.md)で縮小した。
本体のlockと同じ依存を固定し、ABI assertionと借用付きFutureの契約を維持する。
`offset_of!`の通常式との対照、およびARM64の文単位cfgと関数cfgの対照を含む。
警告を出す再現ソースはアプリ本体とは別に分類して記録する。

初回に測定したmain `903227c7`の[run37587266992](https://github.com/ieee0824/omoikane/actions/runs/37587266992)
が成功し、取得DBのSHA・CLI2.27.1・build-mode:noneを照合した。
[新しいDBのハッシュと比較結果](results/2026-10-07-main-903227c7.json)に記録したとおり、
全23診断と対象593マクロ行は上の基準と一致し、15位置の内側AST欠落が残る。
対象6ファイルのDB内ソースも照合済み。

同じ依存の最小例でも、ABIの`offset_of!`3箇所とtrait/implの`__Fn`2箇所が
両解析targetでwarningかつAST 0となった。通常式の`offset_of!`は各8 nodeがある。
文単位cfgのARM64 assertionはx86_64解析で`panic_2021`が欠け、ARM64解析では
展開された。これをcfg依存の診断として追跡し、既存cfgやassertionは変更しない。

ARM64 hostで再現crateの4テスト・locked build、および
`cargo test --locked --manifest-path engine/boa/Cargo.toml -p boa_macros_tests --test embedded`
の`simple` / `compressed_lz4`が成功。元fixtureを保持している。
`embedded.rs`のCodeQL診断は正常系テストで発生する展開失敗として扱い、
`compile_error`という文字列だけで意図的な失敗とは分類しない。
`CARGO_MANIFEST_DIR`とファイルI/Oに依存するproc macroだが、どの環境差が
失敗を起こすかはこの測定だけでは断定しない。
workspace外の`engine/boa/tests/src/lib.rs`診断は別分類のまま保持する。

この時点の最新CLIは2.27.1、最新Rust libraryは0.2.22で基準と同じ。
公開済み更新版での比較はできないため、以下では上流の開発版を明示して比較する。

## 開発版ライブラリとembeddedの転送修正

公開済みの`rust-all`は0.2.22。0.2.23を指定した取得も、公開版が存在しないため
失敗した。一方、公式`github/codeql`のcommit
`36994cdec4ffab77993ac386c85cf551e28f552f`には`rust-all 0.2.24-dev`がある。
この開発版と同commitのshared libraryを使い、CLI 2.27.1で照会した。
**extractorの更新ではなく、未公開ライブラリの比較**である。
[版・依存・main DBの記録](results/2026-10-07-upstream-library.json)に
ソースcommit、manifest hash、解決された依存を保存した。
開発版ライブラリのソースは変更していない。

測定対象は[#1268](https://github.com/ieee0824/omoikane/pull/1268)マージ後のmain
`8bb183dad528d420db2146f1996d2f7ad38a84b8`。
[run37593834964](https://github.com/ieee0824/omoikane/actions/runs/37593834964)とRust jobは
完了成功し、取得ZIP・DB metadata・SHAを照合した。
安定版での診断は従来の23件と、追加した独立再現crateのwarning 6件を合わせた29件。
従来6ファイルの593マクロ結果は変わらず、15位置のAST欠落が残る。
再現crateの診断をアプリ本体の悪化や改善として数えない。
このDBの[全診断](results/2026-10-07-main-8bb-diagnostics.csv)と
[対象マクロ](results/2026-10-07-main-8bb-macros.csv)は、安定版0.2.22と開発版0.2.24-devで
それぞれ順序を無視して一致した。解決先は上流checkoutのライブラリであり、
依存版の表示だけを変えた比較ではない。

embeddedの原因は、proc macroの解析前に入力を観測する隔離実験で確認した。
CodeQLは`$x:expr`で転送された`compress = "none"` / `"lz4"`を
`Group { delimiter: Parenthesis, ... }`として渡す。
`Argument::parse`は文字列か識別子を要求するため、`expected identifier`で失敗する。
`CARGO_MANIFEST_DIR`からfixtureを読む前に失敗しており、ディレクトリ不足が
原因という推測は採用しない。
[実際の入力CSV](results/2026-10-07-embedded-input.csv)には実行値の2行を抽出し、source rootを正規化した。
[観測用patch](results/2026-10-07-embedded-input-probe.patch)は診断worktreeでの
`build-mode:none`専用で、実行可能なModuleLoaderを返さない。
観測後にproc macroを元に戻し、製品ソースにはこの観測処理を入れていない。

`embed_module!`は`tt`として元のトークン列を渡し、引数の検証は従来のproc macroに
任せる。元fixture、圧縮処理、ファイルI/O、ABI assertion、cfgは保持する。
修正後の内側マクロは無圧縮で378 node、LZ4で375 nodeとなった。
その他の対象マクロの結果は変更前と一致する。
[変更前後の集計とソースhash](results/2026-10-07-embedded-fix.json)、
[変更後の全診断](results/2026-10-07-embedded-fixed-diagnostics.csv)も保持した。
本体側warningは18から14へ減り、info 5・error 0は不変だった。
この隔離DBは`903227c7`へ同じ2行修正を適用したもので、独立再現crateの6診断を含まない。

[EmbeddedExpansion.ql](queries/EmbeddedExpansion.ql)で、内側の展開が配列であり、
4つのfile-entry tupleと4つのfixture pathを含み、`compile_error`を含まないことを
確認する。単なる外側AST数の増加では判定しない。
[変更前](results/2026-10-07-embedded-baseline-expansion.csv)は両方とも
`array=0, tuples=0, paths=0, compile_error=1`、
[変更後](results/2026-10-07-embedded-tt-forwarding-expansion.csv)は両方とも
`array=1, tuples=4, paths=4, compile_error=0`。
既存の`simple`と`compressed_lz4`は転送変更後も成功した。
型推論・dataflowや他の13位置の完全解析を保証する結果ではない。
修正候補で`cargo test --locked`は3,433成功・15 ignored、`cargo build --locked`も成功。
`scripts/check-jit-native.sh`の8契約バイナリと16組のnative/interpreter比較が成功した。
元のengineソース保持・path依存、QL packと書式、Rust書式、差分も確認した。

回帰確認には同じクエリを変更前後のDBへ直列に実行し、CSVの行67・77がそれぞれ
`1,4,4,0`となることを確認する。観測用patchを適用したDBをこの確認に使わない。

## マージ後mainでの確認

[#1302](https://github.com/ieee0824/omoikane/pull/1302)は、全8workflow成功・52チェック成功・
条件付きskip 2件を確認して`7a9a1e3ab79ba2a2d922e868dfbfbb3571e7b657`へマージした。
このmainの[CodeQL run37602658585](https://github.com/ieee0824/omoikane/actions/runs/37602658585)も
完了成功し、DB metadataとZIP・展開後のSHA/CLI/build modeを照合した。
ZIPは350,230,657 bytes、SHA-256は
`bbcb1219907acbc44d2f743c81b8744ebf467a87e93ff07d8b73d7096a521405`。

[全診断](results/2026-10-07-main-7a9-diagnostics.csv)はwarning 20・info 5・error 0。
warningの内訳は本体14件と独立再現crate 6件で、embeddedの4件だけが消えている。
その他の本体診断と再現crateの診断は変更前と一致する。
[対象マクロ位置](results/2026-10-07-main-7a9-macro-locations.csv)では、
全591結果行のうち13位置の内側AST欠落が残る。
[embedded回帰結果](results/2026-10-07-main-7a9-embedded.csv)は両fixtureとも
配列1・file-entry tuple 4・fixture path 4・compile_error 0。

安定版0.2.22と開発版0.2.24-devの全診断・対象マクロ結果は一致し、
DB内ソース8ファイルのhashも照合した。
[run・版・依存元・DB/CSV/ソースhash・残存制限](results/2026-10-07-main-7a9.json)に記録した。
CLI/extractorは2.27.1のままであり、新しい公開extractorを検証した結果ではない。
Issue #1266の最初の更新条件は未完了として保持する。

### 再測定手順

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
