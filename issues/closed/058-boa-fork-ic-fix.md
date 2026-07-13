---
number: 058
slug: boa-fork-ic-fix
status: closed
---

# Boa をフォークして inline cache バグを根治する

## 概要

boa_engine 0.21.1 の inline cache 不整合(重い polyfill バンドル実行後、既存コールサイトの
メソッド解決が別のメソッドに化ける)を、Boa 本体のフォークで修正して使う。

## 背景

- 057 で `createElement` の名前検証を native 化して回避したが、tokyo6.tokyo では続けて
  `element.style` の宣言パース(`parseDecls`)内の呼び出しが同じバグで「not a callable
  function」となり、webfont loader が中断 → ページが真っ黒のまま(2026-07-12 実測)
- IC 不整合は任意のコールサイトで発生しうるため、bootstrap の native 化もぐら叩きでは
  根治しない。ユーザー判断で Boa フォークによる根本修正を選択
- 決定的再現あり: scratchpad の boa-repro(sakurav3.js ロード後、warm 済みコールサイトの
  `"s".codePointAt(0)` が `"s0"` = concat の結果を返す)。単一 polyfill では再現せず、
  core-js バンドルほぼ全体の累積で発生(057 調査)。IC は `boa_engine/src/vm/inline_cache/`
  に pub(crate) で、無効化 API なし

## 進め方

1. **ローカル修正**: boa-dev/boa の v0.21.1 相当を clone し、Omoikane の Cargo.toml に
   `[patch.crates-io]`(path)で接続。再現を Boa 側計装で追跡し、IC の invalidation /
   guard の欠陥を特定して修正
2. **検証**: 最小再現の解消、tokyo6.tokyo のレンダリング(黒画面解消)、Acid3 90/100 以上、
   全テスト、Boa 自身のテストスイート(該当領域)
3. **公開**: ieee0824 アカウントに boa をフォークし修正ブランチを push、
   `[patch.crates-io]` を git 指定に切り替え(CI が取得できる公開 URL)
4. 057 の native 化緩和は無害なので残す

## 受け入れ条件

- 最小再現(boa-repro)で `codePointAt` が正しい値を返す
- tokyo6.tokyo の parseDecls エラーが解消しページが描画される
- Acid3 90/100・全テスト維持
- フォーク運用が Cargo.toml に固定され、修正内容が issue に記録される

## 関連

- 057 Boa IC バグの回避(closed。症状回避のみ)
- 118 系 element.style(被害側。コードは正しい)

## 対応結果（2026-07-13）

Boa 0.21.1 をフォークし、インラインキャッシュ（IC）のプロトタイプ・プロパティ汚染バグを
Boa 本体で根本修正した。実装担当（Opus）。

### 根本原因

property IC のファストパス（`GetPropertyByName` / `GetNameGlobal` / `SetPropertyByName`）は、
プロトタイプ上のプロパティ（例: `"s".codePointAt`）を解決する際、キャッシュした
`Slot { index }` が**プロトタイプオブジェクトの storage への添字**であるにもかかわらず、
一致判定 `InlineCache::match_or_reset`（`core/engine/src/vm/inline_cache/mod.rs`）が
**レシーバの shape のみ**を検証していた。プロトタイプの shape ガードが無い。

core-js polyfill 束は `String.prototype` の一部メソッドを delete / 再定義して storage を
詰め直す（`UniqueShape::remove_property_transition` が削除位置以降の slot index を再割り当て）。
これでプロトタイプ側の index は前方へシフトするが、レシーバ（`"s"` の String ラッパ）の
shape は不変なので IC はヒットし続け、**古い index が別プロパティを指す**。

Boa 側に計装を入れて確定した実測値（`sakurav3.js` ロード後、warm 済みコールサイト）:

```
[IC proto-STALE] name=charAt      recv_shape=0x..87c0 proto_shape=0x..2690 cached_idx=3  cur_slot=index 1
[IC proto-STALE] name=toLowerCase recv_shape=0x..87c0 proto_shape=0x..2690 cached_idx=21 cur_slot=index 19
[IC proto-STALE] name=codePointAt recv_shape=0x..87c0 proto_shape=0x..2690 cached_idx=5  cur_slot=index 3
```

`String.prototype` の全 index が一様に -2 シフト（先頭寄りの 2 プロパティが delete/再定義された）。
`codePointAt` の cached index 5 は再配置後の storage で `concat` を指すため、
`"s".codePointAt(0)` が `"s".concat(0)` = `"s0"` を返していた。057 の
「単一 polyfill でなく core-js 束の累積で発生」という観測とも整合する。

### 修正（最小）

`InlineCache` に `prototype_shape: GcRefCell<WeakShape>` を追加。プロトタイプ・プロパティ
slot をキャッシュするときにホルダ（＝レシーバの直接プロトタイプ。cachable な prototype slot は
`Slot::set_not_cachable_if_already_prototype` により必ず 1 段目）の shape も記録する。
`match_or_reset` は PROTOTYPE slot のとき、現在のホルダ shape のアドレスがキャッシュ時と
一致するかを追加検証し、不一致（再配置済み）や GC 済み/プロトタイプ喪失なら miss にして
スローパスへ落とす。own プロパティ slot は `WeakShape::None`。3 つの opcode は
`match_or_reset` 経由なので opcode 側の変更は不要。

変更ファイル（Boa fork）:
- `core/engine/src/vm/inline_cache/mod.rs`（フィールド追加・`set`/`match_or_reset` 更新）
- `core/engine/src/vm/inline_cache/tests.rs`（回帰テスト
  `prototype_property_inline_cache_survives_prototype_reindex` を追加）

### フォーク運用

- fork: <https://github.com/ieee0824/boa> branch `omoikane/ic-fix-0.21.1`（v0.21.1 起点）
- Omoikane `Cargo.toml` の `[patch.crates-io]` で `boa_engine` / `boa_gc` を上記フォークの
  **commit SHA `6b9778e`** に固定（可変ブランチではなく SHA 指定で再現性を担保）。

### 合成再現への差し替え（PR #126 Copilot 指摘対応）

当初の回帰テストは実サイト由来の `sakurav3.js`（Morisawa Inc. の proprietary バンドル、
`distributionKey` / `authToken` を含む）を vendoring していた。ライセンス・認証情報の両面で
リポジトリに残せないため**ファイルごと削除**し、根本原因を直接突く自作の最小再現
`tests/fixtures/js/ic_prototype_reindex.js`（第三者コード・認証情報なし）に差し替えた。
プロトタイプの storage で victim メソッドより前方のプロパティを delete して slot を前方
シフトさせ、warm 済みコールサイト（`obj.target()`、`codePointAt` 相当で 115 を返す）が
再配置後に別メソッド（`"s0"`、`concat` 相当）へ解決するかを突く。テストは fixture 末尾の
sentinel（`globalThis.__issue058Mutated`）で mutation 完了を確認してから再評価し、早期
throw による false-pass を防ぐ。

### 検証結果

- 最小合成再現（Rust 統合テスト `tests/boa_inline_cache.rs` /
  fixture `tests/fixtures/js/ic_prototype_reindex.js`）:
  warm 済みコールサイトが**修正前（フォーク親 commit `bc36c3f`）で `"s0"` を返し FAIL、
  修正後（`6b9778e`）で `115` を返し PASS**。A/B を実測で確認。
- Boa 側 `cargo test -p boa_engine --lib inline_cache`: 9 passed（既存 8 + 新規回帰 1）。
- Omoikane `cargo test --lib -j1`: 1048 passed / 0 failed。
- Omoikane `cargo test --test boa_inline_cache -j1`: 1 passed。
- Acid3（`cargo run --example acid3`）: FAITHFUL 97/100（index 100）、DIRECT 97/100（index 100）。
- tokyo6.tokyo（`examples/screenshot`）: **`parseDecls` の "not a callable function" を含む
  js-error 2 件（IC 汚染由来）が 0 件に解消**（修正前 stderr との before/after で確認）。
  なお当該ページのスクリーンショットは修正前後とも真っ黒のまま。これは IC バグとは独立の
  レンダリング未対応（ヒーロー領域の画像/動画/CSS 等）に起因し、JS はクリーンに完走する
  ようになった（example.com 等は非黒で描画されるため描画パイプライン自体は正常）。

### 限界・残課題

- tokyo6 の黒画面自体はレンダリング完成度の別問題であり、本 issue（Boa IC 根治）のスコープ外。
- 057 の native 化緩和（`__omoikane_is_valid_xml_name`）は無害なため残置。
