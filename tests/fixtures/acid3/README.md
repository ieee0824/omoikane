# Acid3 テストフィクスチャ

Acid3 テスト（http://acid3.acidtests.org/）一式をローカル実行するためにベンダリングしたリソース。

## 取得元・取得日

- 取得元: `http://acid3.acidtests.org/`
- 取得日: 2026-07-10
- 取得方法: `curl -sL http://acid3.acidtests.org/<path>`

`acid3.html` はメインページ（`http://acid3.acidtests.org/` のレスポンスそのまま）。

## ファイル一覧

| ファイル | 用途 |
| --- | --- |
| `acid3.html` | Acid3 メインページ（ピュア版、無改変） |
| `empty.css` | `<link href="empty.css">` 対象。**意図的に `text/html` で配信される**（CSS として適用されないことのテスト） |
| `empty.html` | iframe 用の空 HTML |
| `empty.png` | iframe 用 PNG（HTML としてパースされないことのテスト） |
| `empty.txt` | `text/plain` 配信（中身は HTML だが HTML としてパースされないことのテスト） |
| `empty.xml` | 不正な UTF-8 バイトを含む XML（パーサが中断することのテスト） |
| `reference.html` | 合格時の参照レンダリング |
| `font.ttf` | `@font-face` 用 TrueType フォント |
| `svg.xml` | `image/svg+xml` 配信の SVG（iframe/object 経由でロード） |
| `xhtml.1` / `xhtml.2` / `xhtml.3` | `text/xml` 配信の XHTML（XML パーステスト） |
| `support-a.png` / `support-b.png` / `support-c.png` | `<object>` フォールバックと HTTP ステータス処理のテスト（test 16） |
| `test.html` | URL 解決テスト（test 64）。中身はロードされない |

## HTTP 挙動（manifest.json）

Acid3 は各リソースの **HTTP ステータスコードと Content-Type の正確な値** に依存する
（例: `empty.css` を `text/html` で返す、`empty.txt` を `text/plain` で返す、
`support-a.png` は 404 を返す）。`manifest.json` に取得時点の
ステータス＋Content-Type を記録し、ローカルテストサーバー（`examples/acid3.rs`）が
これを忠実に再生する。

## サーバー劣化に関する注意（2026-07-10 時点）

取得時点で `acid3.acidtests.org` のサーバーは一部のリソースで本来と異なる応答を返していた:

- `support-a.png`, `support-c.png`, `test.html` は **同一の汎用 PNG**
  （md5 `598bb642501c8e969800036d46ce3965`, 2312 bytes）を返す。
  本来 `test.html` は HTML であるべきだが、test 64 は URL 解決のみを確認し
  中身をロードしないため、ベースライン取得には影響しない。
- `support-a.png` は 404、`support-b.png` は `text/html` のフォールバック HTML を返す。

これらは取得時点のオリジンサーバーの実挙動をそのまま保存したもの。
`manifest.json` に記録した値がテストサーバーの真実の source となる。

## 自作ファイルの有無

**なし。** 本ディレクトリの全ファイルは `acid3.acidtests.org` から取得した本物の
バイト列そのままである（自作・改変ファイルは存在しない）。

## 実行方法

```sh
# ローカルにフィクスチャを HTTP 配信し、エンジンで Acid3 を実行してスコアを表示
cargo run --example acid3

# フィクスチャ配信・DOM パースの回帰テスト（スコアに依存しない）
cargo test --test acid3_harness
```

`examples/acid3.rs` と `tests/acid3_harness.rs` は共有ハーネス
`tests/acid3_common/harness.rs` を `#[path]` で取り込んで利用する。

## 現状のスコア（更新: 016-5 時点）

`cargo run --example acid3` の実測で **Faithful / DirectDrive 両モードとも 28/100**。
016-2〜016-5 の実装で以下が解消済み:

- **016-2**: トークナイザに script-data / RAWTEXT / RCDATA 状態を実装し、下記の
  パーサ問題を解消（`<script>` が 10 個に正しく分割され、`update`/`tests` が定義される）。
- **016-3**: `setTimeout` の関数コールバック保持 + イベントループ統合。
- **016-4**: `<body onload="update()">` を実 `load` イベントで起動（ハーネスの手動
  `update()` エミュレーションは撤去）。`.data` / `defaultView` / `Node` 定数 / `localName` を追加。
- **016-5**: `data:` スキームのスクリプト取得に対応（d1〜d5 の 5 ベクタが実行され test 97 が PASS）。

残る主要ブロッカーは iframe / `contentDocument` / `document.write`（P7/P8）、
`getComputedStyle` 実値（P5）、DOMException（P9）など。詳細な次アクションは
`issues/` の 016 系子 issue で追跡する。

### 参考: 016-2 実装前の 0 点だった根本原因（履歴）

`cargo run --example acid3` の実測では **スコアは取得できない（0 相当）**だった。
根本原因は JS ではなく HTML パーサ:

- エンジンのトークナイザに script-data / raw-text 状態が無く、`<script>` 内の
  インライン JS の `<`（比較演算子や `'<div>'` 等の文字列リテラル）を HTML タグとして
  誤って解釈する。結果、パース後の DOM には `<script>` 要素が本来の 10 個ではなく
  14 個でき、Acid3 の本体スクリプト（約 173KB）が 3 断片に分割される。
  `var tests` と `function update` が別々の断片に分かれるため、どちらも構文として
  不完全になり、`execute_document_scripts` が `SyntaxError: abrupt end` と
  `SyntaxError: unexpected token 'return'` を報告する。
- このため `update` / `tests` が定義されず、`typeof update === "undefined"`、
  テストループが一度も回らずスコア 0。Faithful/DirectDrive 両モードで同結果。
- 副次的に、`data:` スキームの外部スクリプト（d1〜d5）が
  `fetch_script_source` の非対応でフェッチできず、`document.write` も未実装。

詳細な次アクションは `issues/open/016-acid3-conformance.md` 系の子 issue で追跡する。
