# Gate 6: 継承する機能と検証の対応

#546〜#551の対象を、実装・具体的な期待値・実行経路から追跡するための表。
採用方式・原本886ファイル・9つのpath依存・来歴とライセンスは
[運用ADR](gate6-engine-ownership.md)に記録する。ここでのケース名は実装中の
アサーションを参照する索引であり、名前の一致だけを動作保証には使わない。
実行revision・feature・caseごとの結果はCI artifactと一緒に確認する。

## 構文・built-inとbootstrap

[DOM bootstrap](../../src/js/dom_bootstrap.js)は関数・クロージャ・class/extends、
プロパティ記述子、Symbol、Map/WeakMap/Set、Promiseなどを使う。
これらを既存のparser・AST・bytecompiler・VM・built-insから引き継ぐ。

| 対象 | 具体的な検証と期待値 |
| --- | --- |
| Scriptのparse・compile・evaluate、let/for/算術 | [jit_gate1](../../tests/jit_gate1.rs)の`public_boa_bytecode_pipeline_compiles_and_runs_arithmetic`がCodeBlockを生成し、0〜99の和4950を検査 |
| 関数・クロージャ・プロパティ・文字列・配列 | 固定[11 workload](../../tests/js_benchmark/shapes.js)と[jit_gate1](../../tests/jit_gate1.rs)。shapeのown/prototype/accessor、old-to-young参照、Promise 45・timer 46を検査 |
| Object/Reflect/SymbolとDOM receiver・identity | [DOM bootstrap](../../src/js/dom_bootstrap.js)、[Web API surface](../../tests/web_api_surface.rs)、[browser_journeys](../../tests/browser_journeys.rs)。APIの存在に加え、実際の入力・DOM変更・画面座標・画素を検査 |
| class/extends・Symbol.species・Array callback | [array tests](../../engine/boa/core/engine/src/builtins/array/tests.rs)の`filter_species_callback_can_collect`。speciesが返す関数オブジェクトに要素7が残り、callback中のGCを越えることを検査 |
| Map/Set・Date/RegExp・ArrayBuffer/TypedArray・BigInt・例外 | [JS tests](../../src/js/mod.rs)の`structured_clone_snapshots_cycles_builtins_and_rejects_unsupported_values`と`dedicated_worker_messages_preserve_supported_clone_builtins`。値・循環・realm側prototype・NaN/負のゼロ・DataCloneErrorを検査 |
| 言語機能全体と既知の未対応 | [固定Test262設定](../../engine/boa/test262_config.toml)と[比較runner](../../scripts/engine-test262.py)。原本と現行を同じ50,595ケースで比較し、case消失・退行・skip化・panicを拒否 |

`gate1-scope.json`とGate 1のrevision定数は過去の設計基準を示す。
現在リンクするエンジンの証拠はroot/nestedのCargo.lock、来歴manifest、
`check-engine-source.py`の依存解決結果とCI checkout revisionである。

## Module・非同期・host連携

| Issue / 対象 | 具体的なケース・アサーション |
| --- | --- |
| #547 Module cache・identity、後続taskのdynamic import | [module_identity](../../tests/module_identity.rs)の`separate_module_scripts_and_later_imports_share_document_module_identity`。static/dynamicのtoken同一性、singleton実行1回、取得URLを検査 |
| #547 並行import・referrer・失敗の分離 | [module_loading_tests](../../src/js/module_loading_tests.rs)の`imports_overlap_share_identity_and_cookies_and_keep_dynamic_imports_lazy`と`failed_imports_settle_without_blocking_other_imports`。共有module実行1回、相対URL、cookie、遅延取得、404/構文エラー/成功の`rejected,rejected,fulfilled`を検査 |
| #547 Module cycle・link error | 固定Test262の`language/module-code/instn-named-iee-cycle.js`、`instn-star-props-circular.js`、`instn-iee-err-circular.js`等。これは言語処理系のmodule graph検証で、HTTP操作は上のembeddingケースで検査 |
| #547 Promise・thenable・再入・GC | [promise tests](../../engine/boa/core/engine/src/builtins/promise/tests.rs)の`resolving_promise_survives_collection_in_then_getter`。getter中のGCと重複resolve/rejectを経てもgetterは1回、3分岐のfulfilled/rejected結果を検査 |
| #547 microtask順序・checkpoint | [JS tests](../../src/js/mod.rs)の`queue_microtask_shares_the_microtask_queue_with_promise_reactions`。同期実行0件、`qm1,p1,qm2,p2,outer,p3,nested`の順序、callbackの引数・this・TypeErrorを検査 |
| #548 native callのsuspend/resume/drop | [JS tests](../../src/js/mod.rs)の`async_eval_suspends_and_resumes_a_native_host_call`と`dropping_async_eval_cancels_its_suspended_host_call`。停止中は後続処理を実行せず、41をresumeすると42、drop後のresumeは失敗しruntime再利用は2を返す |
| #548 hostのthrow・Promise・dialog・crypto | [JS tests](../../src/js/mod.rs)、[JIT exception](../../tests/jit_exception/mod.rs)、[CDP tests](../../src/cdp/mod.rs)。DOM例外とfinallyの順序・opaqueなthrow値のidentity・host error、dialog応答と暗号APIの値を検査 |
| #548 timeoutの分類・復帰 | [JS tests](../../src/js/mod.rs)の`async_evaluation_honors_wall_clock_timeout_and_recovers_runtime`と`user_thrown_timeout_message_is_not_classified_as_wall_clock_timeout`。無限実行を中断し、同じ文字列のユーザー例外と区別してruntimeを再利用 |
| #548 GC/JIT内部契約 | [jit_gc_roots](../../tests/jit_gc_roots.rs)、[jit_runtime_call](../../tests/jit_runtime_call.rs)、[jit_interrupt](../../tests/jit_interrupt/mod.rs)、[jit_exception](../../tests/jit_exception/mod.rs)。native entryの実行カウンタ、minor/major後の生存・weak解放、例外順序、JIT off/onの中断結果を検査 |

## ブラウザ操作・Realm・Worker・過去の回帰

| Issue / 対象 | 具体的なケース・アサーション |
| --- | --- |
| #549 HTTP→入力→fetch→DOM→描画 | [browser_journeys](../../tests/browser_journeys.rs)の`http_page_input_fetch_and_dom_changes_reach_the_painted_frame`。固定HTTP応答、クリック/キー入力、表示文字・色とPNGを検査 |
| #549 フォーム・履歴・CSS/geometry/paint | 同ファイルの`form_submission_and_back_navigation_preserve_saved_state`、`normal_flow_form_controls_accept_input_and_submit_using_painted_coordinates`、stylesheetの2ケース。送信内容、保存値、戻る操作、描画位置を検査 |
| #549 全体互換性 | [Web API](../../tests/web_api_surface.rs)、[WPT smoke](../../tests/wpt_smoke.rs)、[Acid3](../../tests/acid3_harness.rs)、DOM/CDPを含む全suite。両Acid3駆動方式100/100とWPT/Web API regression 0が必要 |
| #550 child Realm・WindowProxy | [module_loading_tests](../../src/js/module_loading_tests.rs)の`concurrent_iframe_imports_keep_separate_realms_and_csp`、[JS tests](../../src/js/mod.rs)のWindowProxy・iframe回帰。各childでmodule実行1回、別token、親のglobal非変更とorigin境界を検査 |
| #550 Dedicated/Shared/Service Worker・messaging | [JS tests](../../src/js/mod.rs)の`dedicated_worker_messages_are_fifo_cloned_and_microtask_checkpointed`、`shared_worker_is_shared_across_same_origin_runtimes`、`service_worker_registration_scopes_lifecycle_and_controller_selection`等。FIFO・clone後の値、共有接続、登録scope/lifecycleを検査 |
| #550 event loop・cancellation・drop | [event_loop](../../src/js/event_loop.rs)、[JS tests](../../src/js/mod.rs)の`every_timer_task_gets_a_microtask_checkpoint`、`dropping_runtime_clears_worker_cycle_before_collection`。task/microtask/rAF順序、破棄対象のcallback取消し、GC後のruntime再生成を検査 |
| #550 タブ保存領域 | [browser_journeys](../../tests/browser_journeys.rs)の`tabs_share_local_storage_but_keep_session_storage_separate`。同originのlocalStorage共有、sessionStorage分離、タブ再開・別origin・別browserの分離を実際のタブ経路で検査 |
| #551 #057/#058/#059 | [boa_inline_cache](../../tests/boa_inline_cache.rs)と[固定fixture](../../tests/fixtures/js/ic_prototype_reindex.js)。mutation sentinel到達後に同じwarm call siteが115を返すこと、own/prototype/accessorとshape分岐の分離を検査 |
| #551 #315/#491・回収中のcallback | [array tests](../../engine/boa/core/engine/src/builtins/array/tests.rs)の`filter_roots_result_during_callback_allocations`等。各callbackが4096個確保しても32要素の結果と入力が一致することを検査 |
| #551 bytecode・deopt・差分・stress | [jit_bytecode_contract](../../tests/jit_bytecode_contract.rs)、[jit_differential](../../tests/jit_differential.rs)、[jit_stress](../../tests/jit_stress/mod.rs)、[Gate 4 runner](../../scripts/jit-gate4.py)。ケース、seed、生成コード・失敗artifactと必須partitionを保持 |

画像の確認方法と操作fixtureの制限は[ブラウザ動作基準](gate6-browser-baseline.md)に従う。
固定fixtureの成功を全Webサイト互換とは扱わない。

## 実行と証拠の照合

rootの`cargo test --locked -- --include-ignored`はproduction defaultのブラウザ基準。
JIT差分・native・stressは[CI](../../.github/workflows/ci.yml)、
[native CI](../../.github/workflows/jit-native.yml)、[配布gate](../../.github/workflows/jit-release-gate.yml)の
featureとコマンドを使い、無効featureでの0件実行を成功証拠にしない。
[engine CI](../../.github/workflows/engine.yml)はnested workspaceのcase inventoryと
個別PASSログ・doc testsを保持する。[Test262 CI](../../.github/workflows/engine-test262.yml)は
元treeと現行を比較する。source・lock・固定fixture・toolchainもartifactに含める。

取り込み前後のローカル2,353ケースはsuite名・case名・結果が一致した。
この過去の照合と、時計修正・性能変更を含む最終revisionの検証を区別する。
各変更後に全suite・ブラウザ操作・JIT契約・3環境の実archive検証を行い、
追加ケースは理由を記録する。原本のfloat16差分は#655、時計修正は#657、
既存array benchmarkのNaNは#658として追跡し、既知の失敗を黙ってskipしない。
