---
number: 036
slug: css-animation-final-state
status: open
---

# CSS animation の最終状態即時適用

## 概要

JS なし環境で `animation-fill-mode: forwards` のアニメーションの最終状態を即時適用する。

## 背景

- モダンサイトで `.fade { opacity: 0; animation: fadein ... forwards }` が一般的
- JS が `.on` クラスを追加してアニメーションを開始するが、JS 未対応では永遠に opacity: 0
- `--force-opacity` で回避可能だが、animation の最終状態を直接適用する方が正確

## 修正方針

1. `@keyframes` ルールをパースし、最終フレーム（to / 100%）のプロパティを抽出
2. `animation-fill-mode: forwards` が指定された要素に最終フレームのスタイルを適用
3. `animation-name` から対応する `@keyframes` を参照

## 受け入れ条件

- `animation: fadein 1s forwards` の要素に fadein の最終状態が適用される
- `animation-fill-mode: none` の場合は適用されない
- 既存テスト全通過
