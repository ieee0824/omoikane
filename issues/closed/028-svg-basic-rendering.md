---
number: 028
slug: svg-basic-rendering
parent:
status: open
---

# SVG 基本レンダリング

## 概要

実サイト（実サイト等）で頻出する SVG アイコンの基本レンダリングを実装する。

## 背景

実サイト ページの未対応HTMLタグログで SVG 関連が最多:
- `circle` (26回), `path` (20回), `g` (11回), `svg` (13回), `rect` (1回)

多くのモダンサイトがインラインSVGアイコンを使用しており、対応しないとアイコンが完全に欠落する。

## 対応内容

### Phase 1: SVG コンテナとビューポート
- `<svg>` 要素を inline-block としてレイアウト
- `width`/`height`/`viewBox` 属性からサイズを決定
- SVG 内部の子要素は通常のレイアウトに含めない

### Phase 2: 基本図形の描画
- `<rect>` — 矩形
- `<circle>` — 円
- `<path>` — パスデータ（`d` 属性の M/L/C/Z コマンド）
- `<g>` — グループ（transform 伝播）
- `fill`/`stroke` 属性による色指定

## 受け入れ条件

- SVG 要素が指定サイズで配置される
- 基本図形（rect, circle）が描画される
- path の直線コマンド（M, L, Z）が描画される
- 既存テスト全通過
