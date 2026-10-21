# GUIの性能計測

GUIの起動・入力・描画を測るときは、同じコミットとビルド設定を使う。
通常のdebugビルドは次のコマンドで作成できる。

```sh
cargo build --locked --features gui --bin omoikane
OMOIKANE_TRACE_STARTUP=1 OMOIKANE_TRACE_PAINT=1 OMOIKANE_TRACE_INPUT=1 \
  OMOIKANE_TRACE_TASKS=1 ./target/debug/omoikane https://www.nicovideo.jp/ \
  2> startup-nico.log
```

`CARGO_TARGET_DIR`を指定している場合は実行ファイルのパスも合わせる。
`CARGO_PROFILE_DEV_OPT_LEVEL=2`を付けてビルドすると、debug assertionsを保ちながら
最適化したdebug版を作れる。設定が違うビルドの時間は同じ基準値として比較しない。

`OMOIKANE_TRACE_STARTUP=1` の起動ログ先頭には `OMOIKANE_BUILD` も出力する。
`commit` はビルド時の完全なGit commit ID、`tracked_dirty` はビルド時に追跡済みファイルの変更があったかを表す。
ログやsampleなどの未追跡ファイルはdirty判定に含めない。Git情報のない配布ソースでは `commit: "unknown"`、`tracked_dirty: null` とし、親ディレクトリの別リポジトリのIDを使わない。
`profile`、`opt_level`、`target`、`debug_assertions` も記録するため、O2 debugとreleaseを区別できる。
情報は実行時のcheckoutではなくバイナリへ埋め込まれる。linked worktreeのHEAD・branch ref・indexと追跡ファイルの変更もビルドの監視対象にする。

`OMOIKANE_TRACE_RETAINED_SCROLL=1` は、通常描画と同じ画像環境で画面外の保持画像を追加生成し、
`OMOIKANE_RETAINED_CAPTURE` に生成可否・未対応理由・生成時間・CSS座標のcoverageを出す。
GUIへ渡すsceneのraster scaleは現在1で、ウィンドウのdevice scaleとは別である。
この計測では追加の描画費用が発生する。通常GUIのnative scrollは既定で有効である。
入力経路を無効にした保持画像だけの診断には、`OMOIKANE_NATIVE_SCROLL=0`も併用する。

### GUIスクロール経路

通常GUIは、再利用可能な保持画像と入力の適格性を確認できた場合に、
ページのJavaScript処理を待たずGUI側で画像を移動する。
`OMOIKANE_NATIVE_SCROLL=0`は緊急時の明示的な無効化、`OMOIKANE_NATIVE_SCROLL=1`は
明示的な有効化である。環境変数を指定しない通常起動でも有効になる。
ページのホスト名ではなく、描画・入力状態によって利用可否を判定する。
未対応の効果や配置、入力の状態変更では通常の入力経路へ戻る。

`OMOIKANE_TRACE_INPUT=1`との併用で出る`OMOIKANE_NATIVE_SCROLL_PROBE`は、入力の受信、
`accepted`/`fallback`/`canonical`の判定、画像の表示、ページ側への配信を記録する。
保持画像の生成成功だけでは、高速経路を実際に利用したことの証拠にはならない。
受信した入力と判定・表示のsequenceを対応させて確認する。
ローカルfixtureでの成功と、実ページでの適格性・応答時間は別に検証する。

計測ログの保存先も操作に影響し得る。共有領域への読み書きが遅い環境では、
一時領域へ記録してから共有先へコピーし、元のログも保持する。
自動入力では画像比較やPNG保存を入力送信の後に行い、実際の受信時刻から入力間隔を検証する。

`OMOIKANE_TRACE_RETAINED_FILTER=1` を併用すると、保持画像を拒否した最初のフィルターについて
`OMOIKANE_RETAINED_FILTER` に要素ID・タグ・配置・ぼかし半径や影の数値を出す。
フィルター列は16個までで、CSS文字列やURL、本文は記録しない。
`OMOIKANE_TRACE_RETAINED_STICKY=1` は同様にsticky配置の最初の未対応箇所を記録する。
これらの診断は再利用の判定を変更しない。

通常GUIのnative scrollは既定で有効だが、保持画像の生成要求は入力位置があり、
ownerに恒久的なblockerがない場合に限る。
`OMOIKANE_NATIVE_SCROLL=0`と`OMOIKANE_TRACE_RETAINED_SCROLL=1`を併用した場合は、
診断用の保持画像だけを生成し、native入力のauthorityは与えない。
対象・画像・ページ状態が一致する入力をGUIで処理し、その通知と既定動作の進捗をページ側へ引き渡す。
未対応の描画効果や入力、状態変更による失効は通常の入力処理へ戻す。
入力の成功数だけでなく、連続操作中の表示位置、通常処理への待ち時間、失効後の再認可を確認する。

`OMOIKANE_STARTUP`の窓の初回描画と、ページ本文の表示完了は異なる。
本文の表示は画面でも確認する。`OMOIKANE_PAINT.frame_ms`にはページの処理・
レイアウト・描画が含まれるので、`paint_ms`を再び加算しない。

非同期画像の完了は、成功したmain/child全体レイアウトが読んだHTTP画像URLと
完了batchのURLを照合する。既知の依存集合に含まれない背景画像ならpaintと派生metrics
だけを無効化し、geometryを保持する。実寸を読む画像とURLを共有する場合や、
レイアウト未完了・失敗・セキュリティ条件変更で依存が不明な場合は再レイアウトする。
背景SVG内部のレイアウトはページ全体の依存情報を確定しない。

親ページと子フレームの描画時間を分ける場合は、`OMOIKANE_TRACE_PAINT=1`と
`OMOIKANE_TRACE_PAINT_ATTRIBUTION=1`を両方指定する。
`OMOIKANE_PAINT_ATTRIBUTION`の`snapshot_collection_ms`は子フレームの準備と再帰的な
画像生成の合計、`child_raster_ms`はその内数となる各子の描画時間の合計、
`main_raster_ms`は子の画像生成後に行う親ページの描画時間である。
子の描画区間は孫の画像生成を含まないため、子同士を二重に加算しない。

`child_style_*`と`main_style_*`の件数は通常要素だけで、入れ子の別profileへ渡った
計数とpseudo専用の計算は含まない。style区間は描画時間と重なるので加算しない。
`document_id`で既存の`OMOIKANE_LAYOUT_STYLE`と対応付けて調べる。
子の詳細行は最大32件で、省略数は`child_raster_detail_overflow`に出る。
候補owner数と`snapshot_draw_calls`は候補数・実描画呼出し回数であり、可視画素の証明や
同じ母集団ではない。差し引いて不可視数を求めない。`completed=false`の記録も
可視性の根拠にしない。通常の`OMOIKANE_TRACE_PAINT`だけではこの追加計測は行わない。

## スタイル再計算と属性変更の計測

`OMOIKANE_TRACE_PAINT=1`は、レイアウトを再構築したときに
`OMOIKANE_LAYOUT_STYLE`も記録する。`cache_hits`と`cache_misses`は通常要素の
computed style解決回数で、pseudo専用の計算は含まない。`outer_compute_ms`は
親の解決を除くcache missの計算時間で、再帰計算の時間を二重に加算しない。
`scope_ms`と同じくレイアウト時間の内数なので、フレーム時間へ加算しない。
キャッシュされたレイアウトを使った場合には、この行は出ない。

属性変更を対応付けるには、起動時に`OMOIKANE_TRACE_ATTRIBUTE_SET=1`を追加する。
`OMOIKANE_ATTRIBUTE_SET`の`cached_style_state`は変更前の既存キャッシュを調べた
結果であり、診断のために新たなスタイル計算やレイアウトは行わない。
`null`や`unknown`は情報不足を示し、可視・非表示の判定結果として扱わない。

`OMOIKANE_STYLE_CACHE_INVALIDATION.outcome`はresolverの無効化範囲を示す。
`local`と`local_raw_style_subject`では変更要素とlight-DOM子孫の通常・pseudo
キャッシュを破棄し、他の要素のキャッシュを保持する。`fallback_*`は全破棄を示す。
この値だけではレイアウトを保持したか判断できないため、同じ入力区間の
`OMOIKANE_SCROLL_TARGET`の`phase: "end"`にある`ensure_layout_rebuilds`と
`ensure_layout_cache_hits`、`ensure_layout_ms`も確認する。読出しごとのログは出さず、
対象ごとに集計する。合計・最大時間と再構築時間は入力処理時間の内数である。

`OMOIKANE_STYLE_CLOCK`は、`OMOIKANE_TRACE_PAINT=1`でclockの更新がgeometryの
無効化対象になったときに出る。transitionとanimationの変更フラグ・layout要求を
区別し、animationは保持状態数と今回のsample変更数も記録する。保持状態には
完了済みのanimationも含まれる。変更フラグだけで最終CSS値が変化したとは判断しない。
paint-onlyを含むchanged clockも同じschemaで記録する場合は、起動時に
`OMOIKANE_TRACE_STYLE_CLOCK_ALL=1`を指定する。この変数は単独で有効にでき、既定の
`OMOIKANE_TRACE_PAINT=1`によるgeometry対象だけの出力条件は変えない。

`transition_cache_before_animation`はtransition clock処理直後のcache結果を示す。
`outcome`は`none`・`subtree`・`full`のいずれかで、通常・pseudo cacheごとの
`ordinary_evicted`・`ordinary_retained`・`pseudo_evicted`・`pseudo_retained`はentry数で
ある。同じsampleで後からanimationがcacheを破棄する前の値であり、処理時間ではない。

transitionのclockが変わった記録には、`running_transition_properties`としてnode数と
property数、geometry・transform・color・background・filter・opacity・otherの排他的な
内訳も出る。通常styleの再sample前に保持されているtransitionの集計であり、すべての値が
そのtickで変化したという意味ではない。animationだけの記録ではこの欄は`null`になる。
分類は調査用で、layoutを省略できるpropertyの一覧ではない。CSS名や値は記録せず、
診断無効時はこの内訳を集計しない。

実サイトの読み込み状態も比較条件に含める。本文の一部が表示されても、スクロール先が
遅延読み込みのプレースホルダーだけなら、通常コンテンツ表示後の性能を検証したことには
ならない。画像とログを保存し、操作・表示位置・読み込み状態が揃った結果を比較する。

## タスクと後続処理の計測

`OMOIKANE_TRACE_TASKS=1`は、タスク本体とその後の処理を合わせて50ms以上かかった
区間を`OMOIKANE_TASK`へ記録する。`elapsed_ms`と`success`は従来どおりタスク本体の
時間と成否である。`background_ms`はWorker等の背景処理、`jobs_ms`は後続の
microtask checkpoint、`cookie_ms`はCookie変更の配信を示す。`turn_ms`はこれらを
含む区間全体で、`turn_success`は後続処理のエラーも反映する。未到達の区間は`null`。
これらはフレーム時間の内数なので、フレーム時間へ加算しない。

authorの`setTimeout`/`setInterval`由来なら、`timer_id`と登録時の
`timer_source`（`file`・`line`・`column`）も出る。HTTP(S) URLはcredential・query・
fragmentを除き、data URLやローカル/inline sourceは本文を出さない。intervalの各発火は
同じID/sourceを使う。resource loadなど内部Timerと、登録位置を取得できなかったauthor
Timerでは`timer_source`が`null`になるが、後者は`timer_id`で区別できる。診断無効時は
登録時のstackを読まず、Timer metadataを保持しない。

`gc-profile`有効時は`gc.callback`・`gc.background`・`gc.jobs`・`gc.cookie`・
`gc.turn`にminor/major別のGC差分も出る。`gc.turn`は各区間を含む合計であり、
内訳と重ねて加算しない。別スレッドで実行されるWorkerのGCは含まない。

この計測はキューから取得したタスクごとの区間である。タスク取得前に行うresourceの
poll、最初のPromise jobs・Cookie配信・背景処理は対象外で、
`run_until_idle()`全体を測る値とは一致しない。診断を無効にすると追加の時計と
GCカウンターの取得は行わない。

`style_invalidations`は同じタスク区間内の無効化回数を集約する。
`local`・`local_raw_style_subject`・`local_data_attribute`は局所無効化である。
`local_data_attribute`はHTMLの`data-*`属性変更について、セレクターへの影響が
変更要素のlight-DOM部分木に収まると確認できた場合を数える。兄弟要素などへ
影響する場合は全体無効化へ戻す。レイアウトと描画の要求範囲は縮めない。
`fallback_*`はinline style変更が
全体無効化へ戻った理由を表す。`ordinary_node_mutation_full`は通常のDOM変更に
よるresolverキャッシュの全消去、`other_full`はそれ以外の共通全消去経路である。
`document_dirty`はstylesheetを含むresolver再構築の要求回数で、同じDocumentへの
重複要求も数える。これらはノード数やDocument数ではない。

カウンターはタスクのcallback・背景処理・jobs・Cookie配信の合計で、区間別には
分けない。タスク取得前の処理、CSS内部で直接行うキャッシュ消去、別スレッドの
Worker処理は含まない。未知のinline理由は`unknown_inline`へ集約し、属性値や
任意の理由文字列を保持しない。診断を無効にすると集計用TLSにもアクセスしない。

## 任意のGC計測

既存のBoa GC計測を有効にするには、`gc-profile`を追加する。
このfeatureはJITを有効にせず、GC閾値や収集方針も変更しない。

```sh
cargo build --locked --features gui,gc-profile --bin omoikane
OMOIKANE_TRACE_TASKS=1 OMOIKANE_TRACE_STARTUP=1 OMOIKANE_TRACE_PAINT=1 \
  OMOIKANE_TRACE_INPUT=1 ./target/debug/omoikane https://www.nicovideo.jp/ \
  2> startup-nico-gc.log
```

`OMOIKANE_CLASSIC_GC`は`eval_script_in_document_realm`を通る同期classic
script処理で、構文解析(`parse`)、トップレベルのコード生成(`compile`)、その後の
評価(`evaluate`)、microtask checkpoint全体(`jobs`)のGC差分を記録する。初期文書と
nonblocking dynamic scriptが使う`eval_async_source`経路は現時点では対象外である。
minor/majorごとに収集回数と`total_ms`・`mark_ms`・`finalize_ms`・`sweep_ms`を出す。
`total_ms`には各内訳の区間外で行う管理処理も含まれる。

各GCログは処理が到達したphaseだけ出る。構文解析に失敗した場合は`parse`だけ、
トップレベルのコード生成に失敗した場合は`parse`と`compile`だけを記録し、
`evaluate`と`jobs`は出ない。評価に失敗した場合も`jobs`は出ない。

`OMOIKANE_CLASSIC_SCRIPT.evaluate_ms`は初回コード生成とその後の評価の合計で、
従来の計測範囲を保つ。構文解析に成功し、`parse_ms + evaluate_ms`が50ms以上の場合だけ
この集約行を出す。コード生成に失敗した集約行の`run_ms`は`null`になる。
`gc-profile`使用時には`compile_ms`と`run_ms`も出す。
通常ビルドのこの2項目は未計測を示す`null`になる。評価中の関数呼び出しに伴う
遅延コード生成などは`run_ms`に含まれ得る。

このログはすべてのeval・module・Realm初期化を測るものではなく、Workerの別スレッドも
含まない。ネストしたスクリプトの区間は重なるため合算しない。
`jit-stress`だけではこのclassic scriptログ用のroot featureは有効にならない。
計測自体にも負荷があるので、通常ビルドとの比較ではfeatureと環境変数を記録する。

JSON診断行はメモリ上で整形してからstderrへ出力し、JSONの各断片ごとの
書き込みを避ける。イベント名・JSON・末尾改行の形式は従来と同じである。
共有ファイルシステムへの保存や診断行数自体にも負荷があるため、実際の操作性能を
比較するときは診断の有無とログの保存先を揃える。
