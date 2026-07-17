---
number: 082
slug: render-phase-timing
status: closed
---

# 実サイトrenderのフェーズ別処理時間を表示

## 概要

実サイトのrender時間が長い原因を切り分けるため、screenshot CLIでレンダリングパイプラインのフェーズ別処理時間を表示する。

## 対応

- stylesheet取得・parse、font読込、JavaScript実行、timer処理を個別計測する
- JavaScript実行後のstylesheet再構築、layout、paint、PNG encodeを個別計測する
- framesetなど複数documentを描画する場合は、1回のscreenshot内の処理時間を合算する
- フェーズ別時間の出力formatに回帰テストを追加する
