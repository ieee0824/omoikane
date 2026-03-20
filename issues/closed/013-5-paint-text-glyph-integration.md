---
number: 013-5
slug: paint-text-glyph-integration
parent: 013-real-world-rendering
status: open
---

# paint_text() へのグリフ描画統合

## 概要

013-1 で実装した font モジュールを paint_text() に統合し、
テキストを実際のグリフとして描画する。現在は固定幅の黒四角で描画されている。

## 前提 (013-1 で実装済み)

- `src/font/mod.rs`: Font struct, グリフラスタライズ, システムフォント検索
- `Font::rasterize(ch, size_px)`: 文字をビットマップ化
- `Font::glyph_advance(ch, size_px)`: 文字幅取得
- `load_system_font(family)`: sans-serif/serif/monospace 等のフォント読み込み
- `FontCache` / `GlyphCache`: キャッシュ機構

## スコープ

- `src/paint/mod.rs` の `paint_text()` 関数を修正
- フォント読み込み → グリフラスタライズ → ビットマップ描画 のパイプライン
- フォント読み込み失敗時のフォールバック（placeholder 矩形）
- 既存テストとの整合性維持（特に ACID2）

## 実装ステップ

1. `paint_text()` に font loading を追加
2. `paint_glyph_raster()` ヘルパー関数でビットマップ描画
3. `paint_placeholder_glyph()` フォールバック描画
4. グリフの y 座標計算（baseline offset）
5. ACID2 テスト通過確認

## 技術課題

- グリフの baseline 位置合わせ
  - ab_glyph の `px_bounds()` と fragment.rect.y の整合
  - ascent/descent を考慮した y offset 計算
- キャッシュの活用
  - FontCache を paint 全体で共有
  - GlyphCache を font ごとに管理

## テスト戦略

- `paints_inline_text_fragments` テストの更新（pixel 判定方法変更の可能性）
- ACID2 baseline の再生成（テキスト描画が変わるため）
- regression テストの追加

## 参考

- 013-1 の実装: Phase 1b で `paint_text()` 統合を試みたが、
  y 座標計算とテスト整合性の問題で一旦 revert
- 必要な変更箇所: `src/paint/mod.rs:697-741` (`paint_text` 関数)
