---
number: 057
slug: boa-inline-cache-poisoning
status: closed
---

# Boa のインラインキャッシュ汚染で String メソッド解決が化ける（tokyo6.tokyo）

## 概要

boa_engine 0.21.1(最新)で、重い polyfill 実行後に**特定コールサイトのメソッド解決が別の
String メソッドに化ける**エンジンバグを確認した。dom_bootstrap.js の `isValidXmlName` 内の
`chars[0].codePointAt(0)` が文字列 `"s0"`(= `concat` 相当の結果)を返し、
`createElement('style')` / `('button')` が InvalidCharacterError で失敗する。

## 再現（実測済み、決定的）

1. https://tokyo6.tokyo/ を `cargo run --example screenshot` でレンダリング
2. または: `https://webfonts.sakura.ne.jp/js/sakurav3.js`(webpack + core-js polyfill 同梱、
   約130KB)をロードした後、`document.createElement('style')` を呼ぶローカルページ

## 診断結果（計装で確認）

- ページ内 JS では同時点で `Array.from("style")` = 正常、`"style".codePointAt(0)` = 115、
  数値比較・関数コピーすべて正常。`Array.from`/`codePointAt` は未パッチ
- bootstrap クロージャ内の**当該コールサイトだけ** `chars[0].codePointAt(0)` が `"s0"` を返す
  （"s".concat(0) と同じ結果 = メソッド解決が concat に化けている）
- 同じ式でも**新しいコールサイトでは正しい値**を返す(サイト単位の汚染)
- `if (!isXmlNameStartChar(chars[0].codePointAt(0)))` が false 判定 → 直後に別サイトで
  再呼び出しすると true、という非一貫動作を確認

## 対応候補（調査順）

1. Boa の Context 設定で optimizer / inline cache を無効化できるか調査し、無効化して
   tokyo6.tokyo が正常レンダリングされるか検証（速度とのトレードオフを実測）
2. 最小再現 JS を作成（sakura.js から core-js の該当部分を削って絞る）し、
   Boa upstream に issue 報告
3. 1 が不可能な場合: bootstrap 側の緩和（検証ロジックの native 化 =
   `__omoikane_is_valid_xml_name` を Rust 側に移す等）を検討。ただし IC 汚染は
   任意のコールサイトに起こりうるため根本対策にはならない点を明記

## 受け入れ条件

- tokyo6.tokyo のレンダリングで createElement の InvalidCharacterError が解消する
- 採った対策(エンジン設定/上流報告/緩和)と速度影響が記録される
- Acid3 88(実装時点の main)以上と既存テストの維持

## 関連

- 044 Web API 体系的カバレッジ（実サイト JS 完走トラック）
- 016-12 名前検証（isValidXmlName 自体は正しい実装。壊すのはエンジン側）

## 対応結果（2026-07-12）

Codex が着手した WIP（`isValidXmlName` の native 化）を引き継ぎ、実装担当（Opus）が
検証・完成させた。方針は issue の対応候補 3（緩和）に一致しており妥当と判断して採用。

### 採用した緩和策：`isValidXmlName` の native 化

- `src/js/dom_bootstrap.js` の JS 実装（`Array.from(value)` + `chars[i].codePointAt(0)`）を
  削除し、`__omoikane_is_valid_xml_name(value)` の呼び出しに置き換えた。
- `src/js/mod.rs` に Rust ネイティブ関数 `is_valid_xml_name_native` と補助関数
  （`is_valid_xml_name_start_char` / `is_valid_xml_name_char`、XML Name の
  コードポイント範囲は元の JS と 1:1 で一致）を追加し、host binding として登録した。
- これにより、汚染される JS コールサイト（`codePointAt`）を検証パスから除去した。
  `createElement` / `createElementNS` はこの native 検証を通るため IC 汚染の影響を受けない。
- 回帰テスト `xml_name_validation_does_not_depend_on_string_method_dispatch` を追加。
  `String.prototype.codePointAt = String.prototype.concat` でバグを模擬しても
  `createElement('style')` が成功し、不正名が正しく `InvalidCharacterError`(code 5)を
  投げることをアサートする。
- **速度影響**: 検証を Rust に移すため実質ゼロ〜微減（追加の JS 実行が消える）。
- **限界（明記）**: IC 汚染は任意のコールサイトで起こりうるため、これは根本対策ではなく
  当該症状（createElement 失敗）に対する回避。根本解決は上流（Boa）修正を要する。

### Boa 設定調査の結論（optimizer / inline cache 無効化の可否）

- **結論: boa_engine 0.21.1 にインラインキャッシュを無効化する公開 API も cargo feature も存在しない。**
- `Context::set_optimizer_options(OptimizerOptions)` は公開されているが、`OptimizerOptions` は
  `CONSTANT_FOLDING` と `STATISTICS`(統計出力)の 2 ビットのみ（`src/optimizer/mod.rs`）。
  これは AST レベルの定数畳み込みであり、プロパティアクセスのインラインキャッシュとは無関係。
  `OptimizerOptions::empty()` にしても本バグは解消しない。
- インラインキャッシュ本体は VM 実装 `src/vm/inline_cache/mod.rs` にあり `pub(crate)`。
  外部からの無効化手段（API・feature フラグ）はなく、プロパティアクセス命令に不可分に組み込まれている。
- 定数畳み込みは実行時のコールサイト単位で値が化ける挙動（診断で確認済み）を説明できず、
  症状は VM のプロパティアクセス IC（shape→slot キャッシュ）に一致する。
- したがって「エンジン設定で無効化」は不可能で、対応候補 3（native 化緩和）が現実的な唯一の選択肢。

### 最小再現の絞り込み結果

外部の victim（`function victim(s){ return Array.from(s)[0].codePointAt(0); }`）をポリフィル実行前に
一度呼んで IC を warm させ、バンドル実行後に同一コールサイトを再呼び出しして汚染を検出する形で切り分けた。

| 再現物 | 内容 | 結果 |
| --- | --- | --- |
| 完全な sakurav3.js（≈130KB, core-js + webpack） | 実サイト由来 | **再現**（`actual="s0"`, `"s".concat(0)` 相当） |
| polyfills.js（≈130KB, ポリフィル部分抽出） | core-js ポリフィル群 | **再現** |
| reduce.js / module-257.js（≈130KB, 部分削除版） | バンドルの一部を削減 | 再現せず |
| minimal-repro.js（17KB, core-js 2.6.11 `es6.string.*` HTML メソッド + `es6.date.*`） | モジュール単位で抽出 | 再現せず |
| synthetic.html（`Object.defineProperty(String.prototype, [14 個の HTML メソッド名])`） | 手書きで shape 変異を模擬 | 再現せず |

- **結論**: モジュール単位の削減・合成では再現しなくなり、ほぼ完全な core-js ポリフィル束
  （polyfills.js ≒ sakurav3.js）でのみ決定的に再現する。単一のポリフィルが引き金ではなく、
  ポリフィル群による shape 遷移／生成バイトコードの累積が閾値を越えて IC 汚染に至ると考えられる。
- 30 分のタイムボックス内でこれ以上は絞れなかったため、上流報告用の再現物は
  「victim を warm → core-js ポリフィル束を実行 → 同一コールサイト再呼び出し」パターン
  （victim は DOM に一切依存しない純 JS）で確定とする。再現物は scratchpad の boa-repro/ に保存。

### 検証結果

- `cargo build`: パス
- `cargo test --lib -j1`: 1026 passed / 0 failed（回帰テスト含む）
- `cargo test --tests -j1`: 1026 + 4 passed / 0 failed
- `cargo run --example acid3 -- --all`: FAITHFUL 90/100(index 100)、DIRECT 90/100(index 100)。両モードで維持
- tokyo6.tokyo（`cargo run --example screenshot -- --insecure https://tokyo6.tokyo/`）:
  createElement の `InvalidCharacterError` は 0 件に解消（無関係な js-error が 2 件残るが本 issue 対象外）
- ローカル再現の before/after（sakurav3.js + `document.createElement('style')`）:
  緩和前 = `InvalidCharacterError` を再現、緩和後 = クリーンにレンダリング
