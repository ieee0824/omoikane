---
number: 051
slug: css-property-value-validation
parent: 016
status: closed
---

# CSS プロパティ値検証と computed style serialization

## 概要

CSS 宣言をプロパティごとの文法で検証し、無効値を宣言時に破棄する。
最初の対象として `cursor` keyword と初期値・computed style serialization を整備する。

## 背景

Acid3 test 47 は `#bogus { cursor: bogus; }` を無効宣言として破棄し、computed style で
初期値 `auto` を返すこと、および有効な cursor keyword 群を指定どおり返すことを要求する。
現在は未知の値も生の keyword として保持されるため、selector 拡充では解決できない。

issue 031 は `cursor` を supported property として登録して未対応ログを抑制する基本対応であり、
値文法の検証までは扱わない。本 issue はその後続として実装し、issue 031 側からも参照する。

## 対応内容

- 宣言値を property grammar に照らして検証し、無効宣言をカスケード前に破棄
- `cursor` の許容 keyword 検証（Acid3 test 47 の keyword 群を含む）
- 無効な `cursor` 値から初期値 `auto` への fallback
- specified/computed value の正規 serialization
- CSS parser/style resolver/getComputedStyle の経路間で検証結果を統一

## 関連 issue

- 016 Acid3 対応（test 47）
- 031 cursor / transform-origin の基本対応（本 issue の前提・相互参照）
- 047 inline style カスケード（inline 宣言にも同一の値検証を適用）

## 受け入れ条件

- `cursor: bogus` が破棄され、computed style が `auto` を返す
- 有効な cursor keyword が受理され、正規化された値を computed style が返す
- stylesheet と inline style の双方で同じ値検証が適用される
- Acid3 test 47 が通り、既存テストも全て通る
