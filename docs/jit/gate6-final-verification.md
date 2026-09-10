# Gate 6: 独立化と性能改善の最終検証

2026-09-10、#515 / #546〜#553 の完了条件を、実装・実行ケース・画像・配布物で照合した。
既存Boaを `engine/boa/` へ取り込み、Omoikaneの同じPRとCIで保守できる状態にした。
ブラウザ動作を先に検証・修正し、次にGC内部のポインタ集合のハッシュ計算を改善した。
運用は[採用ADR](gate6-engine-ownership.md)、具体的な期待値は
[機能と検証の対応表](gate6-validation-map.md)に記録する。

## 採用した変更と検証ソース

| 変更 | 採用結果 |
| --- | --- |
| ブラウザ操作・描画・保存領域の修正 | [PR #654](https://github.com/ieee0824/omoikane/pull/654)、merge `508998b84d171adf804e5684dae6e8d1fd69ccaa` |
| ソース取り込み・依存・CI・配布と時計修正 | [PR #656](https://github.com/ieee0824/omoikane/pull/656)、merge `6a5f6411fdecc5cc42f44d05758d3f105a6a96af` |
| GC内部集合の性能改善・11計算結果の回帰テスト | [PR #660](https://github.com/ieee0824/omoikane/pull/660)、merge `46a08df8744f24bf693531a7bd4867cd7df65083` |

最終実装headは `f721a750b18ca58c1686e45c24febc0303ec997b`、CI checkoutは
`b3eee561af1e8849ba1db77ff05d94420d7be909`。上記PR #660のmergeを含め、
3つのGit treeは同一と照合した。本報告はその実装に対する結果を記録する文書変更である。

元のforkのcommit `ee98fdacffb38093d9d220c2ac21a4ed6839ce37` とtree
`ef1116ba957e98da406752da8838094b99295098` を追跡できる。
[来歴manifest](../../engine/boa-origin.json)の886元ファイルを保持し、元treeは
OmoikaneのGit履歴から復元する。旧forkへのアクセスや上流へのマージに依存しない。
rootの9クレートはpath依存で、384パッケージのバージョンを維持した。
最終source検査の元ファイルからの変更は時計5ファイルとGC2ファイルで、欠落は0。
元のlicense、23 package manifests、元workspaceの20メンバーと除外設定も保持する。

## 最終CIと実行したケース

Linux x86_64、Linux ARM64、macOS ARM64で確認した。macOS Intelは対象外。
以下6 workflowはすべて成功し、同じrevision・lock・固定fixture・toolchainの
artifactを取得して内容を照合した。件数はこのrevisionの実測で、将来の保証ではない。

| 検証 | 結果と実行証拠 |
| --- | --- |
| production default全suite | [CI](https://github.com/ieee0824/omoikane/actions/runs/34472500095): ignoredを含む2,354件成功、0 failed / 0 ignored。移行前2,353件のsuite名・case名を全保持し、11計算結果のテスト1件を追加 |
| engine workspace | [Engine](https://github.com/ieee0824/omoikane/actions/runs/34472500100): x86 1,576件、ARM64各1,580件、doc tests各128件成功。59バイナリのcase inventoryと個別PASSを照合。既存unit ignore 1件・doc ignore 3件を成功数へ加算しない |
| 固定Test262 | [Test262](https://github.com/ieee0824/omoikane/actions/runs/34472500106): 原本/現行×3環境の各50,595件を比較し、全ケースの結果が一致。ケース欠落・追加失敗・panicなし |
| 実際のブラウザ操作と画像 | [Browser](https://github.com/ieee0824/omoikane/actions/runs/34472500184): 3環境×JIT無効/有効で、操作7件とAcid3 harness4件が成功。42操作JSONと36PNGを取り込み最終版と照合 |
| GC・deopt・exception・timeout | 上記CIのGate 4の全8 partition成功。集計2,404実行はpartition間の重複を含む。173 seed / 601.83秒で3 backendの結果・中断・復帰を照合、各seedの自動GCと再現source/code/logを保持 |
| native JIT | [Native](https://github.com/ieee0824/omoikane/actions/runs/34472500311): 各環境38契約テスト、4 seed × 2形状 × JIT無効/有効の16件成功。計算値と実際のcompiled entry、property guardを確認 |
| 3環境の実配布物 | [Release gate](https://github.com/ieee0824/omoikane/actions/runs/34472500372): 下記3 archiveの生成・展開・実行・内容照合に成功 |

各環境の配布suiteは `baseline-jit` 有効で2,392件成功、0 failed / 0 ignored。
PR #656の2,391件の完全なsuite名・case名・個別成功を保持し、計算結果テスト1件を追加した。
stdoutを出すケース、空白を含むshould-panicの表示名、doc testsも照合対象に含む。
各環境でAcid3両方式100/100、WPTは固定
`dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead` の69件でregression 0、
Web APIは97項目でerror/regression 0を確認した。全項目supportedという意味ではない。

Test262の固定revisionは `a073f479f80b336256b7fc4e04700c827293e2fe`。
x86は47,606成功・933失敗・2,056 ignore、ARM64の2環境は各47,616成功・923失敗・
2,056 ignoreである。原本との非退行の判定であり、全Test262成功とは扱わない。
元workspaceに時計の回帰テスト4件を追加し、その後のGC変更では既存ケースを削除していない。

GCのみを変えたPR #660ではparser専用workflowは起動条件外。
時計修正を含むPR #656の[parser stack](https://github.com/ieee0824/omoikane/actions/runs/34469595988)と
[parser ASAN](https://github.com/ieee0824/omoikane/actions/runs/34469595995)は成功済みであり、
今回もnested workspace内のparserの通常ケースは実行した。

## 要件ごとの確認

[対応表](gate6-validation-map.md)にある機能のうち32ケースはソースのアサーションを
直接確認し、最終3環境の全suiteまたはengine個別ログで成功を照合した。
循環module 3件とPromise thenable 3件の固定Test262も、原本/現行×3環境で成功した。
単なる件数やテスト名の検索だけを機能の成立の根拠にしていない。

| Issue | 完了を判断した実装・証拠 |
| --- | --- |
| #546 | 原本の構文・VM・built-ins・bootstrapとテストを棚卸し、元sourceの内容・来歴、workspace各ケースを照合 |
| #547 | parse/link/evaluate、module cache・identity・cycle/error・HTTP referrer、thenableの値/例外、nested microtaskとcheckpoint順序を継承 |
| #548 | native call、41→42のresume、drop後の取消し、再入、host activationの復元、host callback/dialog中のforced GC、例外・timeout分類と復帰を継承 |
| #549 | PR #654で入力・フォーム・CSS/座標・描画を修正し、同じHTTP→入力→fetch/module→DOM→描画、送信・履歴の操作と画像で再検証 |
| #550 | 同originタブのlocalStorage共有とsessionStorage分離を修正し、Realm/Worker・clone・task順序・cancellation・dropを再検証 |
| #551 | 元case/fixture/期待値を保持し、過去のIC・array GC・bytecode/JIT回帰を実行。新しい11計算結果テストを追加 |
| #552 | source・9 path依存・両lock・CI・license付き配布物をOmoikane管理へ移し、更新責任・rollback手順を記録・実行確認 |
| #553 | 上記6 workflow・3実archiveと同条件の性能比較を照合し、既知の失敗と残る性能課題を区別 |

全36PNGは同じ環境・modeで変更前後のhashが一致し、異なる13画像を目視確認した。
入力前後、フォーム送信、戻る履歴、stylesheet変更の形状・色・表示文字を確認したが、
system fontの太さ・傾きには変更前から環境差があった（#661）。
環境間のpixel一致や正しいfont face選択まで保証する結果ではない。
固定fixture、lazy stylesheet、ローカル専用snapshotの範囲は
[ブラウザ基準](gate6-browser-baseline.md)と[ADR](gate6-engine-ownership.md)を参照する。

## 実配布archive

以下はRelease gateから取得した実ファイルで、全memberのサイズ・SHA256、build manifest、
来歴・両licenseを照合した。展開したlibraryでFFI各2件、診断probe各80件が成功した。
default libraryのfeatureは空、診断バイナリのみ `baseline-jit` である。
production defaultのJIT方針は変えていない。PRの検証archiveであり、新しいリリースタグを
公開したという意味ではない。

| archive | bytes | SHA256 |
| --- | ---: | --- |
| `omoikane-linux-x86_64.tar.gz` | 19,489,518 | `d4b6c4fd0f9a004e2f406db47851240dae22bece8cabdd2293f5809f4b0adf2b` |
| `omoikane-linux-aarch64.tar.gz` | 18,960,704 | `b49f2d460fec27114be2eb32359e93b95ff8754b4828491dfcf87318379f52f6` |
| `omoikane-macos-aarch64.tar.gz` | 17,379,965 | `23dfee3baa0eda386689639cbf287e92e033aeaa65a875a4ffa97a6ce22c225d` |

## 性能と未解決事項

[性能報告](gate6-gc-performance.md)と[全サンプル](measurements/gc-pointer-sets-2026-09-10.json)を保存した。
比較元は `484e26e5573a3381fe9505cc2a5a6113cc97edd2`、比較後は同じ基準にGC変更を加えたもの。
時計修正との統合前の同一Linux ARM64 / Rust 1.98.0 / cargo test profileで測り、
統合後のGC実装が測定したものとbyte単位で同一であることを確認した。
元11 body・回数・4 passを保ち、実行順を交互にした新しいプロセスで比較した。
releaseや他CPUの速度を測定したものではない。

全11項目を実行するテスト全体の中央値はJIT無効9.79%、有効4.64%短縮。
代表HTTPページの追加20組では無効4.84%、有効6.25%短縮し、全100回の操作と画像が一致した。
各項目の意味論は元の回数で個別に確認した。全項目が改善したわけではなく、
JIT有効の文字列参照4.87%・文字列連結2.58%の遅延は #659 に残した。
改善の選定、初回/追加測定、途中終了143と再開、不採用候補も性能報告に記録する。

| Issue | 最終時点で残る内容 |
| --- | --- |
| [#655](https://github.com/ieee0824/omoikane/issues/655) | nested lockのfloat16 0.1.5はソフトウェアf64→f16の丸め前に下位ビットを捨て、x86でTest262 10件がARMと違う。原因・最小再現・0.1.7候補を記録。root lockは0.1.7で区別し、nested更新後の全3環境検証は未実施 |
| [#658](https://github.com/ieee0824/omoikane/issues/658) | 元array workloadがundefinedを読みNaNになる。今回は元fixtureを保持し個別結果を検査。fixture修正時はbaseline再測定が必要 |
| [#659](https://github.com/ieee0824/omoikane/issues/659) | JIT有効の文字列2項目の遅延。反復で再現し、直接原因は未確定。全体と代表ページの改善を理由にGC変更を採用 |
| [#661](https://github.com/ieee0824/omoikane/issues/661) | familyのみ・順序不定のsystem font検索とstyleを除去した名前照合で、通常文字にbold/italic faceを選び得る。実際の選択file/faceは未取得、環境ごとの画像差は変更前から存在 |

時計の #657 は、SystemTimeで作る期限と単調時計による測定の差を診断し、
期限を `Instant` ベースの `Clock::monotonic_now()` に揃えて修正した。
320回の診断、修正前に失敗する回帰ケース、修正後の全CIを確認済みで、Date/Temporalの
日時の意味論は保持する。OSのどの機構が時計差を生んだかまで特定したとは扱わない。

rollbackは取り込みcommitの別checkoutで旧git pinへの復帰を実行し、9クレート・
384 version不変とLinux ARM64の操作7件・build/guiを確認した。mainへrollbackを
適用したり、rollback版の3環境archiveを検証したりした記録ではない。
将来実際にrollbackを採用する際の全suite・3環境配布条件はADRに残す。
