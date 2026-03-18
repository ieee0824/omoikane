---
number: 012
slug: acid2-official-conformance
status: open
---

# Acid2 公式リファレンス一致

公式 `reference.html` と `reference.png` を fixture 化し、比較用の ignored テストも追加したが、
現状の `acid2.html` レンダリングは公式リファレンスと大きく乖離している。

## 現状
- `paint::tests::acid2_fixture_matches_official_reference_rendering` を `--ignored` で実行すると失敗する
- 差分ピクセル数は 207,517
- 差分画像は `tests/output/acid2/acid2.official-reference.{actual,expected,diff}.png` に出力される

## タスク
- [ ] 公式比較の差分画像を見て、主要な未実装要素を特定する
- [ ] 参照ページ側で欠けている描画要素（背景、見出しテキスト、画像配置など）を切り分ける
- [ ] Acid2 本体側で欠けている描画要素（object/img、背景、フォント shorthand、テキスト配置など）を切り分ける
- [ ] 必要なら子 issue に分割して段階的に差分を減らす
- [ ] ignored の公式比較テストを常時通る状態へ近づける

## 相談
- まずは `reference.html` 側が期待どおりに描けているかを確認してから Acid2 本体に戻るほうが安全そう
