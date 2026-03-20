---
number: 014
slug: paint-text-glyph
status: closed
---

# paint_text() を実際のグリフ描画に置き換え

## 概要

✅ **既に実装完了** (commit 8adca84, PR #6)

`src/paint/mod.rs` の `paint_text()` 関数は既に実際のグリフ描画に対応済みです。
013-1 で実装されたフォント・グリフラスタライズ機能が統合されており、
テキストはビットマップグリフとして正確に描画されます。

## 実装内容

- ✅ フォント読み込み (`load_system_font("sans-serif")`)
- ✅ グリフラスタライズ (`font.rasterize(ch, font_size)`)
- ✅ Baseline 計算 (`baseline_y = rect.y + metrics.ascender`)
- ✅ カーニング対応 (`font.glyph_kerning(prev_ch, ch, font_size)`)
- ✅ Canvas 描画 (`canvas.draw_glyph_mask()`)
- ✅ フォールバック処理 (placeholder rectangles)

## 関連 commit

- `8adca84` — feat: paint_text() に実際のグリフ描画を統合
- `bf56e46` — Merge PR #6 from ieee0824/feat/013-5-paint-text-glyph
- `0808b5e` — フォントカーニング対応と実フォントによるテキスト幅計測の統合

## テスト状況

- ✅ paint tests: 67 passed
- ✅ ACID2 baseline: 通過 (10,000px tolerance)
- ✅ cargo build/test: 成功
