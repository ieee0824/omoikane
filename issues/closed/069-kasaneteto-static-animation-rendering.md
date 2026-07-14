---
number: 069
slug: kasaneteto-static-animation-rendering
parent:
status: closed
---

# kasaneteto.jp の静止アニメーション・Grid レンダリング改善

## 概要

`https://kasaneteto.jp/` を 1366x900 でスクリーンショットすると、キービジュアルの
キャラクターが透明なまま表示されず、`KASANE TETO` の名前付き Grid 配置も崩れる。
実サイト CSS を縮小した回帰テストを追加し、静止レンダリング向けの決定論的な
CSS animation 評価と Grid 配置を改善する。

## 原因

- キャラクターの基底スタイルは `opacity: 0` で、
  `animation: kvCharacterFade 8s infinite linear` の 5%〜25% だけ可視になる
- 現実装は `animation-fill-mode: forwards | both` の最終キーフレームだけを適用し、
  実行中・無限 animation を評価しないため基底値の透明状態が残る
- 見出しは名前付き area を含む `grid-template` shorthand を使用するため、
  実サイトと同じ構文を回帰 fixture として固定する必要がある

## タスク

### Phase 1: 回帰テスト

- `kvCharacterFade` と `animation-delay` を縮小した computed-style テストを追加
- 実サイトと同じ3列・3行の `grid-template` shorthand/area 配置テストを追加
- 1366x900 のスクリーンショット smoke test 用 fixture を追加

### Phase 2: 静止 animation 評価

- `@keyframes` の全オフセットを保持する
- スクリーンショット用の決定論的な静止時刻で duration/delay/iteration を評価する
- 少なくとも数値・opacity は隣接キーフレーム間を線形補間する
- `forwards`/`both` の既存挙動とテストを維持する

### Phase 3: Grid 配置

- 実サイト形式の `grid-template: "..." auto/...` を正しく展開する
- named area の row/column/span とコンテナ原点を回帰テストで固定する

### Phase 4: 検証

- `cargo test --lib` を全件通過させる
- `https://kasaneteto.jp/` を 1366x900 で再取得し、キャラクターと見出し配置を比較する

## 受け入れ条件

- キービジュアルで少なくとも1体のキャラクターが不透明に描画される
- `KASANE` と `TETO` がデスクトップ用 Grid の同一行に配置される
- 静止結果が実行ごとに変化しない
- 既存 Acid2/Acid3 およびライブラリテストを退行させない

## クローズ記録（2026-07-14）

- `@keyframes` の全オフセットを保持し、静止スクリーンショットを固定時刻1.2秒で評価
- `animation` shorthand の `infinite` を `animation-iteration-count` へ展開
- `margin-inline`/`margin-block`/`padding-inline`/`padding-block` shorthand をstart/endへ展開
- `grid-template` の空白なしslash（`auto/1fr`）を正規化
- kasaneteto.jp縮小fixture 2件を追加し、実サイト1366x900でキャラクター表示と同一行Grid配置を確認
- `cargo test --lib`: 1151 passed / 0 failed / 26 ignored

## 関連

- [061-5 CSS Grid 名前付きエリア](../closed/061-5-grid-named-areas.md)
- [068 cascade important / animation 後処理](068-cascade-important-origin-order.md)
