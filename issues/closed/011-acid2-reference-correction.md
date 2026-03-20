---
number: 011
slug: acid2-reference-correction
status: closed
---

# Acid2 比較基準の修正

現状の Acid2 統合テストは、公式 `reference.html` ではなく `omoikane` 自身が描いた PNG を比較基準として固定している。
このため「Acid2 に合格した」ことを示すテストにはなっておらず、ローカル回帰検知と公式比較が混同されている。

## タスク
- [x] 公式 `reference.html` を fixture として追加する
- [x] 公式 `reference.png` を fixture として追加する
- [x] 既存の `acid2.reference.png` をローカル baseline であることが分かる名前へ変更する
- [x] paint 統合テストの名称とメッセージを実態に合わせて修正する
- [x] issue / ドキュメント上の「Acid2 合格」表現を、ローカル回帰比較と区別できる表現へ直す
- [x] `cargo test` と `cargo build` を通す

## 完了条件
- 公式 fixture とローカル baseline が別物として明示されている
- テスト名と失敗メッセージが比較対象を誤認させない
- closed issue を読んでも、公式 Acid2 pass を確認済みだと誤解しない

## 相談
- 将来的に公式 `reference.html` のレンダリング比較まで行うには、`<img src="reference.png">` のローカル資産解決か、外部ブラウザでの reference 画像生成手段が必要

## 結果
- `tests/fixtures/acid2/reference.html` と `tests/fixtures/acid2/reference.png` を追加
- 既存の自己生成 PNG は `tests/fixtures/acid2/acid2.baseline.png` へ改名
- テスト名を local baseline 比較であることが分かる名称へ更新
- closed 済みの `007` / `007-5` issue に補足を入れ、公式 Acid2 pass と誤解しない表現へ修正
