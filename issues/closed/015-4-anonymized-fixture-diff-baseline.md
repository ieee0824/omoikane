---
number: 015-4
slug: anonymized-fixture-diff-baseline
parent: 015-anonymized-real-world-rendering-gap
status: closed
---

# 匿名化fixtureと差分比較基盤

## 概要

サイト固有情報を含まない形で継続的に描画品質を追跡するため、
匿名化fixtureと画像差分比較の運用を整備する。

## スコープ

- 匿名化HTML/CSS/画像fixtureの配置ルール定義
- baseline画像の生成と更新手順を文書化
- 差分画像の出力先/命名規則を統一
- CIで実行可能な比較テスト追加（必要なら `ignored` で段階導入）

## 受け入れ条件

- ローカルで baseline / actual / diff を再現できる
- リポジトリ内にサイト名/URL等の固有情報を含めない
- 差分改善が時系列で追える状態になる

## 実施結果

- `src/paint/tests.rs` に fixture/diff 用の共通パスヘルパーを追加
- 差分画像の出力命名を `<fixture>.<scenario>.<variant>.png` へ統一
- `tests/fixtures/README.md` を追加し、匿名化fixtureの配置ルールと baseline 更新フローを文書化
- `tests/README.md` を追加し、actual/expected/diff の出力命名規約と運用フローを文書化
- 命名規約を担保するテスト `fixture_paths_follow_anonymized_diff_convention` を追加

## 検証

- `cargo test paint::tests::fixture_paths_follow_anonymized_diff_convention` 通過
- `cargo test paint::tests::acid2_fixture_matches_local_baseline_png` 通過
- `cargo test paint::tests::acid2_fixture_matches_official_reference_rendering` 通過
