# ADR: 既存Boa workspaceをOmoikaneで管理する

- Status: Accepted / Implemented（PR #656 / #660、[最終検証](gate6-final-verification.md)）
- Date: 2026-09-10
- Scope: #515、#546〜#553
- Decision: 既存forkを `engine/boa/` に取り込み、Omoikaneの変更単位で保守する

## 決定と比較

上流へ再統合することを前提とせず、parser・VM・built-ins・GC・JITとブラウザ連携を
引き継ぐ。crate名とAPI、production defaultのインタプリタ実行を維持する。
新しい言語処理系の実装やproduction JITの自動有効化は、この移行に含めない。

| 方式 | 評価 |
| --- | --- |
| forkを独立branchで継続 | sourceの重複を避けられるが、エンジンとembeddingが別PRになり、pin更新と双方のCI・配布revisionの照合が毎回必要 |
| Omoikane内へ取り込み（採用） | 約24 MiBの既存workspaceを保持し、エンジン・embedding・検証・配布を同じPRで変更できる。別forkへのマージを待つ必要がない |

取り込み元は `ieee0824/boa` の `ee98fdacffb38093d9d220c2ac21a4ed6839ce37`、
treeは `ef1116ba957e98da406752da8838094b99295098`。
[来歴manifest](../../engine/boa-origin.json)は886ファイル・25,292,385 bytesの
元データ、Git blob、SHA-256、modeを記録する。元のcopyrightと両LICENSEを保持する。
移行だけの段階では全ファイルがbyte単位で一致し、以後の変更はOmoikaneのcommitで追跡する。

## 実装・検証の対応

構文・built-in・host連携・ブラウザ操作の具体的なケースと期待値は
[機能と検証の対応表](gate6-validation-map.md)を参照する。

| 領域 | 引き継ぐソース | 主な検証 |
| --- | --- | --- |
| 構文・AST・文字列 | `engine/boa/core/{parser,ast,interner,string}` | 同workspaceのunit/doc tests、`boa_parser` の `parser_stack`、固定Test262 |
| Script/Module・VM・built-ins | `engine/boa/core/engine` | エンジンworkspace suite、Test262、`tests/jit_gate1.rs` |
| Module・Promise・microtask | エンジンmodule/promiseと `src/js/module_loading_tests.rs` | `tests/module_identity.rs`、HTTP/module/fetch操作、module・非同期unit tests |
| host call・中断・rooting | エンジンnative function/VM、`src/js/` | dialog/fetch/crypto、suspend/resume/drop、timeout・GC testsとJIT runtime/interrupt契約 |
| GC・JIT・bytecode | `engine/boa/core/{gc,engine}` | engineの `jit::`、`tests/jit_{bytecode_contract,stack_map,runtime_call,gc_roots,deopt}.rs`、exception/interrupt/stress |
| DOM・CSSOM・入力・描画 | `src/js/`、CDP・frame・layout・paint | 全suite、`browser_journeys`、Acid3、Web API surface、固定WPT |
| Realm・Worker・保存領域 | 既存engine Realmと `src/js/` のhost連携 | iframe identity/lifetime、Dedicated/Shared/Service Worker、clone・event loop・tab storage |
| 過去のproperty回帰 #057/#058/#059 | engine ICと既存JS fixture | `tests/boa_inline_cache.rs`、`tests/fixtures/js/ic_prototype_reindex.js`、期待値115とmutation sentinel |
| #315/#491のGC回帰 | registered rootsとarray built-ins | engine/gc tests、`filter_roots_result_during_callback_allocations`、GC/JIT stress |

rootから使う9クレートは `boa_ast`, `boa_engine`, `boa_gc`, `boa_interner`,
`boa_macros`, `boa_parser`, `boa_string`, `small_btree`, `tag_ptr`。
`Cargo.lock` の384パッケージのバージョンを保持し、この9つのsourceだけをgitからpathへ
切り替える。rootとnested workspaceのlockfileを両方管理し、CIは `--locked` を使う。

CLI、runtime、interop、profiler、ICU provider、wasm、examples、macros tests、tester、
fuzz/WPT、ICU生成・開発scriptも削除しない。package manifestは23であり、これは
workspaceの必須メンバー数やrootからリンクするcrate数と同義ではない。
workspaceの `exclude` と各featureの範囲は元のmanifestを保持する。

## 比較条件と既知の制限

移行前のブラウザ基準はPR #654の `0d2a89bdecbcad15ead5e6614157e763a6483e84`。
通常実行で2353ケースを実行し、全ケースのsuite名とcase名を保存した。移行後も同じ
ソース・fixture・feature・期待値で実行し、名前と件数、結果を照合した。
最終GC変更後は計算結果テスト1件を加えた2354件で、元の2353ケースを全保持して成功した。
JIT有効時は既存Gate 4/native契約と同じfeature・seed・stress条件を使う。
caseの消失、既存assertionの弱化、未実行を合格として扱う変更は認めない。

ローカルの取り込み後検証は同じ2353ケースがすべて成功し、欠落・追加は0。
`cargo build --locked`、GUI checkも成功した。Acid3は両駆動方式100/100、
固定WPTは69ケースでregression 0、Web APIは97項目でerror/regression 0。
PR #654の後続修正はmacOSのテスト用HTTP socketとBash互換性を調整するもので、
ブラウザ実装や期待値を変えない。移行前後の完全比較と、このfixture修正後の検証を区別する。

`engine-test262.yml` は保持した原本と現在のengineを3環境で個別に実行する。
原本はOmoikaneのGit履歴中のtreeから復元し、旧forkへのアクセスを要しない。
886ファイルの内容・実行属性を照合してから実行し、同じ固定suite、lock、toolchainの
ケース名・結果を比較する。未実行、古いrevision、ケース消失、成功の失敗化、
失敗のskip化、panicは比較失敗となる。生ログ・case一覧・比較結果をartifactへ残す。

元のエンジン単体Test262の既知差分は #655 に記録した。原本のlockは
`float16 0.1.5`、rootのlockは `0.1.7` であり、x86のソフトウェア丸めの不具合は
0.1.5で再現する。初回取り込みでは原本を保持し、この依存変更を暗黙に混ぜない。
原本との一致と既知不具合の解消は別に判定する。

最終採用したPR #656のheadは `a55b6ff8bcb18c1a6db18fb6fe27521cb9486e44`、
mergeは `6a5f6411fdecc5cc42f44d05758d3f105a6a96af`。その後のGC性能変更PR #660は
`46a08df8744f24bf693531a7bd4867cd7df65083` でマージした。
[最終検証](gate6-final-verification.md)のengine workspaceはx86 1576件、ARM64各1580件と
各128 doc testsが成功した。ARM64の4件の差は専用JITテストであり欠落ではない。
既存unit ignore 1件・doc ignore 3件は成功数へ加えていない。

取り込み途中のhead `484e26e5573a3381fe9505cc2a5a6113cc97edd2` では、
macOS ARM64の `Atomics.waitAsync` で原本と現行の成否が変わり比較が失敗した。
この問題を #657 で診断・修正した。修正を含むPR #656の全CI、さらにPR #660の
原本/現行×3環境の全50,595ケース比較が成功した。PR #660では全結果が原本と一致し、
原本の既知の失敗を解消したという意味ではない。

#657では、タイマーの期限を測る`SystemTime`とTest262の`Instant`が異なる速度で
進むことを確認した。macOSの診断では前者の200msに対し後者が約199.90msで、
16ファイルの反復320回中34回が200ms未満の終了として失敗した。
`Clock::monotonic_now()`を追加し、標準時計と各executorの期限を`Instant`に揃えた。
`Date.now()`とTemporalは従来の日時を使い、既存の制御用Clockはdefault methodで
タイマーを進められる。来歴manifestは原本のまま保持し、この修正を別commitで追跡する。

[修正候補の診断](https://github.com/ieee0824/omoikane/actions/runs/34465810545)では
Linux ARM64・macOS ARM64とも同じ320回がすべて成功し、時計の前進・後退を扱う
回帰テストも修正前の2件失敗から修正後の3件成功へ変わった。
[CLIの回帰検証](https://github.com/ieee0824/omoikane/actions/runs/34466306688)も
修正前の失敗と修正後の成功、CLI・smol/tokioサンプルのbuildを確認した。
これは固定した対象ケースの修正確認である。修正後の全Test262・全ブラウザsuite・
3環境配布も成功し、[最終報告](gate6-final-verification.md)で実行ソースと照合した。

- Test262のrevisionと未対応featureは `engine/boa/test262_config.toml` を維持する。
  rootブラウザは `annex-b` を有効にするが、任意のIntl/experimental featureを
  新たにproductionへ追加しない。Test262の既知の非対応とpanicは区別する。
- parserの既存ignored診断 `illegal_code_point_following_numeric_literal` は
  元の「テスト妥当性の確認が必要」という理由を保持する。engine側の既存skipを
  未実装機能の新規成功として数えない。
- rootの `debug_blog_ast_moe_layout_snapshot` は既存のローカル診断で、untrackedの
  HTML snapshotを必要とする。ローカルの移行前後では同じhashのsnapshotを用いる。
  CIではsnapshot不在なら既存コードがearly returnするため、この診断にCIの動作保証を求めない。
- ブラウザの固定fixture、lazy stylesheet load、描画画像の確認範囲は
  [ブラウザ動作基準](gate6-browser-baseline.md)に記録する。

## 継続的な保守

エンジンの変更・依存更新・検証・配布はOmoikaneのmaintainerが所有する。
上流の変更を採用する場合も、元commit・必要性・licenseをPRへ記録し、現在の
Omoikane向け修正と互換性をレビューする。上流や旧forkのHEADへ自動追従しない。
来歴manifestは初回取り込みを示す記録として保持する。

変更目的ごとにcommitし、`scripts/check-engine-source.py` の差分報告、関連engine tests、
ブラウザsuite、JIT on/offと配布gateを確認する。依存の更新では両方のlockfileをレビューし、
crateの二重解決や旧git sourceの混入を確認する。
性能変更はブラウザ動作の基準確定後に同じ11 workload・代表ページを反復測定し、
結果と意味論を維持した改善を記録する。今回採用したGC変更の測定条件・改善・
残る文字列2項目の遅延は[性能報告](gate6-gc-performance.md)を参照する。

## 配布・rollback

対象はLinux x86_64、Linux ARM64、macOS ARM64。macOS Intelは必須対象に含めない。
実際に展開・起動するarchiveへ `boa-LICENSE-MIT`、`boa-LICENSE-UNLICENSE` と
`boa-origin.json` を含め、build manifestでhashを照合する。

rollbackはreview用branchで行う。rootの `boa_engine` と `boa_gc` を、取り込み元の
上記git URL/revisionへ同時に戻し、既存lockfileを起点に依存解決する。
関連する9クレートすべてが同じgit revisionに戻り、他のpackage versionが変わらないことを
照合する。未使用になったin-tree sourceを直ちに消す必要はない。
同じブラウザsuite・build/gui・3環境配布を確認してからrollbackを採用する。
独立化後のengine修正を戻す場合は、embeddingへの影響も含めて対応commitをrevertし、
実行方針を変えずに同じ検証を行う。

2026-09-10に、取り込みcommit `4bd2846d10d6d8b88ea99bc289da88920e385d1e` の
別checkoutでrootのCargo manifest/lockを旧git pinへ戻し、9クレートがすべて
`ee98fdacffb38093d9d220c2ac21a4ed6839ce37`、他を含む384パッケージのversionが
変更されないことを検証した。Linux ARM64でブラウザ操作7件、`cargo build --locked`、
`cargo check --locked --features gui` が成功した。これは旧pinへ戻す手順の実行確認で、
mainへrollbackを適用した記録や、rollback版の3環境配布検証ではない。

取り込み・時計修正・GC変更を含む[最終配布CI](https://github.com/ieee0824/omoikane/actions/runs/34472500372)は
3環境ともarchive生成・展開、展開したlibraryのFFI、JIT probe、ignoredを含む全suite、
Acid3、WPT、Web API、native契約に成功した。取得した実archiveの全memberのhash・サイズ、
来歴・licenseも照合した。CI checkout `b3eee561af1e8849ba1db77ff05d94420d7be909` は
PR #660のheadとmergeの両方と同じGit treeで、3環境が同じrevisionとroot lockを使う。

[最終検証](gate6-final-verification.md)に実行commit・ケース照合・画像・3 archiveのhash、
残る #655/#658/#659/#661 とrollback手順の検証範囲をまとめた。
独立化後の変更もこの保守・検証手順に従う。
