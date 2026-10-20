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

`OMOIKANE_STARTUP`の窓の初回描画と、ページ本文の表示完了は異なる。
本文の表示は画面でも確認する。`OMOIKANE_PAINT.frame_ms`にはページの処理・
レイアウト・描画が含まれるので、`paint_ms`を再び加算しない。

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
`OMOIKANE_SCROLL_LAYOUT.rebuild`と処理時間も確認する。

実サイトの読み込み状態も比較条件に含める。本文の一部が表示されても、スクロール先が
遅延読み込みのプレースホルダーだけなら、通常コンテンツ表示後の性能を検証したことには
ならない。画像とログを保存し、操作・表示位置・読み込み状態が揃った結果を比較する。

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
