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
- 差分ピクセル数は 66,465
- 差分画像は `tests/output/acid2/acid2.official-reference.{actual,expected,diff}.png` に出力される

## 進捗メモ
- `background: none` が `background-image` しか消していなかった問題を修正し、`.picture` の赤ベタ背景を解消
- `float: left/right` と `clear: both` の最小対応を入れ、Acid2 部品の横配置を少し改善
- `border-top` / `border-left` など side shorthand を展開し、side ごとの border color paint に対応
- color keyword に `yellow` / `navy` / `purple` / `maroon` を追加し、Acid2 の配色を改善
- ローカル baseline `acid2.baseline.png` は今回の描画改善に合わせて更新済み
- `head` / `title` / `meta` / `style` / `script` / `link` をレイアウト対象から除外し、`reference.html` 側の不要テキスト描画を抑制
- `title` テキストが `<head>` ではなく本文側へ落ちていた tree builder の扱いを修正し、`&nbsp;` の文字参照も decode するようにした
- ignored の公式比較テスト差分は `66,465 -> 26,020 -> 23,114` まで減少
- `reference.html` 側では、本文テキストが `Hello World!` のみになることをテストで確認できた
- `content: ''` の擬似要素を border 図形として描けるようにし、legacy な `:before` / `:after` も pseudo-element として扱うようにした
- inline 画像フラグメントが自身の style（padding / border / background）を持てるようにし、`object` fallback では実際に画像を解決した内側要素の style を使うようにした
- 現時点の主な残差分は、Acid2 本体側で目・鼻・口まわりの形状が大きく崩れていること

## タスク
- [x] 公式比較の差分画像を見て、主要な未実装要素を特定する
- [x] 参照ページ側で欠けている描画要素（背景、見出しテキスト、画像配置など）を切り分ける
- [ ] Acid2 本体側で欠けている描画要素（object/img、背景、フォント shorthand、テキスト配置など）を切り分ける
- [ ] 必要なら子 issue に分割して段階的に差分を減らす
- [ ] ignored の公式比較テストを常時通る状態へ近づける

## 相談
- まずは `reference.html` 側が期待どおりに描けているかを確認してから Acid2 本体に戻るほうが安全そう
