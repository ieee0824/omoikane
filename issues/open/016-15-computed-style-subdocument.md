---
number: 016-15
slug: computed-style-subdocument
parent: 016
status: open
---

# getComputedStyle のサブ文書（iframe contentDocument）対応

## 目的

getComputedStyle の解決基盤をクエリ対象ノードの owner document 基準にし、
Acid3 bucket3（selectorTest, test 33〜44,46,47）を解放する。

## 背景（PR #105 レビューでの発見）

- 016-8 で実装した `ensure_style_resolver` は **メイン文書の `<style>` のみ**を収集する
- Acid3 の selectorTest は `getTestDocument()` = iframe の contentDocument に `<style>` を追記してから
  `doc.defaultView.getComputedStyle(node, '').zIndex` を読む（acid3.html:196-210 付近）
- そのため 016-9（iframe/contentDocument）と 016-8（getComputedStyle 実値化）を統合しても、
  サブ文書側の `<style>` が resolver に取り込まれず z-index ルールが欠落し、selectorTest は失敗し続ける
  （統合ブランチ実測 58/100 時点で test 33-43,46,47 が空文字返却で fail）

## スコープ

- StyleResolver の構築をクエリ対象ノードの owner document 基準にする（文書ごとの resolver キャッシュ + dirty 管理）
- サブ文書の `defaultView` を正しく返す（現状は暫定で globalThis）。少なくとも
  `doc.defaultView.getComputedStyle` がサブ文書のカスケードを解決できること
- selectorTest は z-index の**カスケード結果のみ**を読むため、サブ文書のレイアウト計算までは不要（切り分けること）

## 受け入れ条件

- サブ文書に `<style>` を追記した後の getComputedStyle がそのルールを反映する
- 既存 matcher で判定可能なセレクタの selectorTest（test 33, 35, 36, 41, 42 など）が PASS する
- セレクタ自体の拡充は 016-10 の範囲（本issueでは扱わない）

## 関連

- 016-8（同根基盤・closed 予定）、016-9（サブ文書基盤）、016-10（matcher 拡充）
