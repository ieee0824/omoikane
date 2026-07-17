---
number: 081
slug: screenshot-cli-render-timing
status: closed
---

# screenshot CLIへURLと処理時間を表示

## 概要

実サイトの性能比較をしやすくするため、screenshot保存時にURL、navigation時間、render時間を表示する。

## 対応

- `navigate`: URL取得と遷移に要した時間
- `render`: screenshot APIによるPNG生成時間（base64 decodeとファイル書き込みは除外）
- 各時間を小数1桁のmillisecondsで成功行へ追加
- 出力formatの回帰テストを追加
