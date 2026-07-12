---
number: 058
slug: boa-fork-ic-fix
status: open
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
