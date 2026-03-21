---
number: 016
slug: acid3-conformance
parent:
status: open
---

# Acid3 対応

## 概要

Acid2 相当の静的レイアウト・描画互換性に続いて、
より広いブラウザ互換性を確認するために Acid3 を通す。

Acid3 は DOM / CSS / HTML parser / scripting / networking まで含む複合テストのため、
単一機能の修正ではなく段階的な分解と検証が前提になる。

## 背景

- 現状は Acid2 系の描画互換性までは一定の到達がある
- ただし実ブラウザ相当の互換性を測るには、動的挙動や API 面の不足がまだ大きい
- Acid3 を目標に置くことで、HTML/CSS 描画エンジン単体からブラウザ互換基盤全体へ進める

## 想定スコープ

- HTML parser / DOM の仕様差分の洗い出し
- CSS parser / selector / cascade の不足補完
- JavaScript 実行と DOM bindings の互換性向上
- タイマー・イベント・例外処理などブラウザ API の最小整備
- HTTP / data URI / encoding / resource loading の互換性向上
- Acid3 実行結果を継続確認できるテスト・観測基盤の整備

## 進め方

1. まず Acid3 を実行できる最小 harness を用意する
2. failure をカテゴリ別に分解して子issue化する
3. 影響範囲の狭い基盤差分から順に潰す
4. 最終的にスコアだけでなく、安定して再現できる CI/ローカル検証手順を固める

## 受け入れ条件

- Acid3 の実行とスコア取得がローカルで再現できる
- 主要 failure が子issueに分解され、追跡可能になっている
- 最終的に Acid3 が通過、または少なくとも未達成項目が明確に整理されている

## 備考

- Acid3 は広範囲の互換性を要求するため、短期完了前提ではなく段階実装とする
- 必要に応じて parser / DOM / CSS / JS / networking / harness の各観点で子issueへ分割する
