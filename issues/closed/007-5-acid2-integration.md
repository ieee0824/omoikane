---
number: 007-5
slug: acid2-integration
status: closed
parent: 007
---

# Acid2 統合テスト

Acid2テストページをレンダリングし、ローカル回帰比較用 baseline とピクセル比較できる状態を整備する。

## 補足
この issue 完了時点の比較基準は `omoikane` 自身が生成した baseline であり、公式 `reference.html` ベースの Acid2 pass 判定ではない。
公式比較基準への修正は後続 issue で扱う。

## タスク
- [x] Acid2テストHTMLをローカルに配置（テストリソース）
- [x] HTML→DOM→スタイル計算→レイアウト→ペイントの統合パイプライン
- [x] レンダリング結果のPNG出力
- [x] ローカル baseline とのピクセル単位比較テスト
- [x] 差分がある場合の差分画像出力（デバッグ用）
- [x] CI で実行可能な統合テストとして整備
