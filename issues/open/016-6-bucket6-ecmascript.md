---
number: 016-6
slug: bucket6-ecmascript
parent: 016
status: open
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
