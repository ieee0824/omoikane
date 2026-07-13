---
number: 046
slug: document-write-spec-gaps
parent:
status: open
---

# document.write の仕様残差

## 概要

016-7（PR #103）で実装した document.write は Acid3 の使用パターン（単一 write・script 混在なし）を
仕様どおり満たすが、汎用ケースで HTML 仕様と乖離する既知の残差がある。実サイト互換のために追跡する。

## 背景（PR #103 レビューより）

現実装はパース完了後の一括 script 実行方式のため、トークナイザ挿入ポイントを
「実行中 script の直後の兄弟」で近似し、書き込まれた script は断片挿入後に eval される。

## 残差一覧

1. **script 混在断片の挿入・実行順**: `document.write('<script>document.write("X")<\/script><div>tail</div>')` で、
   実ブラウザは X が script と tail の間に入るが、現実装は断片全挿入後に eval するため X が tail の後ろに落ちる。
   複数の書き込み script が互いに write する場合も同様に順序が乖離する
2. **外部 `<script src>` / `type="module"` の実行**: 書き込まれた外部/モジュール script は取得・実行されない
   （PR #103 修正でフィルタし「実行対象として返すのに実行しない」不整合は解消済み。実行自体が未対応）
3. **再帰深度ガード**: write が write する script の連鎖に深さ制限がなく、理論上ネイティブスタック
   オーバーフローに至る（Boa 0.21 に interrupt API がなく実行タイムアウト全般が未強制、という既知制約と同根）
4. **タグをまたぐ分割 write**: `write('<b')` + `write('>x</b>')` のような入力ストリーム連結は未対応
   （断片は write 1回で完結する前提）

## 補足（065 による分岐追加、2026-07-13）

065（Acid3 test 71）で write のパース経路が2分岐になった:
対象文書に documentElement が無い場合（`doc.open()` 直後）は**完全文書パース**
（doctype + 暗黙 html/head/body を構築）、documentElement がある場合は従来どおり
`<body>` ラップのフラグメントパース。項目 4 の分割 write に取り組む際は、
初回 write が documentElement を作った後の2回目以降がフラグメント経路になる、
という新挙動を前提にすること。

## 優先度

低〜中 — Acid3 には未発現。実サイトの広告タグ・計測タグ等で顕在化し得る。

## 受け入れ条件

- 上記 1〜4 のうち対応する項目について、仕様準拠の挙動をテスト付きで実装する
- 全対応が難しい場合は、対応項目と残す項目を明確にして本issueを分割する
