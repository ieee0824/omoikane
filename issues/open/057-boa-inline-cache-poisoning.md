---
number: 057
slug: boa-inline-cache-poisoning
status: open
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
