---
number: 016-6
slug: bucket6-ecmascript
parent: 016
status: closed
---

# bucket6 (ECMAScript, test 81-96) の実測と微修正

## 目的

ドライバ起動後に bucket6（test 81〜96）を実測し、Boa の素通し率を確認、
必要な微修正を行う。理論上最大 16 点の「タダ取り」領域。

## 背景（GAP_ANALYSIS.md セクション3 領域C、セクション5）

- bucket6 は基本的に Boa 0.21 で通る想定だが、Boa のバージョン依存で失敗し得る要注意 4 件がある。
- 実測（016-2 実装後）では bucket6 の大半が通り、test 85 のみ "not a callable function" で失敗。

## 要注意項目（GAP_ANALYSIS.md セクション5）

- test 88: 識別子中の Unicode エスケープ不可（parse error にできるか）
- test 89: 正規表現の空クラス `/[]/`・孤立ブラケット
- test 90: 正規表現の NUL・前方参照 backref・否定先読み
- test 93: 名前付き FunctionExpression の名前が ReadOnly 束縛になるか

## スコープ

- bucket6 の各テストを実測し、pass/fail を確定
- Boa 素通しで通らないもののうち、Omoikane 側で対処可能な微修正を実施
- Boa 自体の制約に起因するものは切り分けて記録

## 受け入れ条件

- 81〜96 の各 pass/fail が実測ログで確認できる
- 要注意 4 件（88/89/90/93）と test 85 の原因が切り分けられている

## 進捗

### test 85 の原因特定と対処（完了）

- **原因**: `"scathing".substr(-7, 3)` の `String.prototype.substr` が未定義で「not a callable function」。Boa 0.21 では `substr`（ならびに Annex B 由来の各種レガシー API）は `annex-b` フィーチャの背後にあり、既定フィーチャ（`float16`,`xsum`）には含まれない。
- **対処**: `Cargo.toml` の `boa_engine` を `features = ["annex-b"]` 付きに変更。test 85 が PASS。bucket6 の他テスト（86〜96）に回帰が無いことを `cargo run --example acid3` で確認済み。

### bucket6 実測（43/100 到達時点、direct-drive）

- PASS: 81-84, 86-97（86〜96 の要注意 4 件 88/89/90/93 含め Boa 素通しで通過）
- FAIL: test 98（"not a callable function"）。これは ECMAScript ではなく XHTML/XML DOM 依存（`document.implementation.createDocument` 等）で、016-12/016-14 スコープ。bucket6 の ECMAScript 本体は 85 の解消で全通過。

## クローズ記録（2026-07-13）

受け入れ条件を達成したためクローズ。

- 81〜96 の pass/fail は実測ログで確認済み（全 PASS）
- 要注意 4 件（88/89/90/93）は Boa 素通しで通過、test 85 は `annex-b` フィーチャ有効化で解消と切り分け済み
- スコープ外として残っていた test 98 も、その後 016-14-1（XML/XHTML サブ文書）の実装で PASS 済み
