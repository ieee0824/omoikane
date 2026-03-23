---
number: 034-1
slug: missing-inherited-properties
parent: 034-css-spec-compliance-fixes
status: open
---

# 継承プロパティの不足修正

## 概要

CSS 2.1 §6.2 で定義される継承プロパティのうち、`apply_inheritance()` に含まれていないものを追加する。

## 不足しているプロパティ

| プロパティ | 影響 | 優先度 |
|-----------|------|--------|
| `font-style` | イタリック体が子要素に継承されない | 高 |
| `font-weight` | 太字が子要素に継承されない | 高 |
| `text-align` | テキスト揃えが子要素に継承されない | 高 |
| `visibility` | visibility:hidden が子要素に継承されない | 高 |
| `border-collapse` | テーブルの枠線結合が子要素に継承されない | 中 |
| `border-spacing` | テーブルの枠線間隔が子要素に継承されない | 中 |
| `text-decoration-line` | 下線等が子要素に継承されない | 中 |
| `text-decoration-color` | 装飾色が子要素に継承されない | 中 |
| `text-decoration-style` | 装飾スタイルが子要素に継承されない | 中 |
| `direction` | テキスト方向が子要素に継承されない | 低 |
| `text-indent` | テキストインデントが子要素に継承されない | 低 |

## 修正箇所

- `src/css/style.rs` の `apply_inheritance()` にプロパティを追加

## テスト

- 各プロパティが親→子で継承されることを確認するテスト
- 子要素で明示的に上書きした場合に継承されないことを確認
