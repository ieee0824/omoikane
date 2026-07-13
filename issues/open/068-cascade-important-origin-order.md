---
number: 068
slug: cascade-important-origin-order
parent:
status: open
---

# カスケードの important 段位・animation 後処理の仕様準拠

## 概要

CSS Cascade の important origin 順位に、047 のレビュー（critical-reviewer）で発見された
既存の仕様逸脱が2点ある。いずれも 047 以前からの挙動で、047 の PR スコープ外として切り出す。

## 指摘内容

### 1. UA `!important` の段位が低い

- `cascade_rank`（`src/css/style.rs` 1050 行付近）は `(true, Author)=4 > (true, UserAgent)=3` としている
- 仕様（CSS Cascade 4 §6.1）では UA `!important` が最上位（transition を除く）:
  `transition > UA !important > user !important > author !important > animation > author normal > ...`
- 現状 UA origin の stylesheet は production では未使用（UA デフォルトは `apply_ua_defaults` の
  ハードコード）のため実害は顕在化していないが、UA origin を使い始めた時点で誤動作する

### 2. animation 後処理が important 宣言を上書きする

- `apply_animation_final_state`（fold 完了後の後処理）は `animation-fill-mode: forwards`/`both` の
  最終キーフレーム値を `contains_key` ガードなしで挿入するため、author `!important` や
  インライン `!important`（047 で統合）すら上書きする
- 仕様では important author 宣言は animation より強い（上記順位参照）
- 対応: animation 値の適用時、当該プロパティが important 宣言由来かを判定してスキップする
  （fold 時に important 適用済みプロパティ集合を保持する等）

## 受け入れ条件

- `cascade_rank` の順位が CSS Cascade 4 §6.1 と一致するテストを追加
- `style="color: blue !important"` + `animation-fill-mode: forwards` の色アニメーションで
  computed color が blue のままであるテストを追加

## 優先度

低〜中 — UA origin 未使用・animation と important の併用は稀のため実害は限定的。
仕様準拠の正しさの問題として追跡する。

## 関連

- 047 のレビューで発見（[047](047-inline-style-cascade.md) のカスケード統合自体は正しい）
