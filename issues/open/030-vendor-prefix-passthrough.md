---
number: 030
slug: vendor-prefix-passthrough
parent:
status: open
---

# ベンダープレフィックスの標準プロパティへのマッピング

## 概要

`-webkit-*`, `-ms-*` 等のベンダープレフィックス付きプロパティを
対応する標準プロパティに自動マッピングする。

## 背景

実サイト ページの未対応CSSログ:
- `-webkit-align-items` (67回), `-webkit-box-align` (67回), `-ms-flex-align` (67回)
- `-webkit-flex-shrink` (24回), `-webkit-justify-content` (18回)
- `-webkit-box-pack` (18回), `-ms-flex-pack` (18回)

これらは既に標準プロパティとして実装済みの機能のベンダープレフィックス版。
マッピングするだけで大幅に未対応を減らせる。

## 対応内容

### マッピングテーブル
| ベンダープレフィックス | 標準プロパティ |
|----------------------|---------------|
| `-webkit-align-items` | `align-items` |
| `-webkit-justify-content` | `justify-content` |
| `-webkit-flex-shrink` | `flex-shrink` |
| `-webkit-flex-grow` | `flex-grow` |
| `-webkit-flex-direction` | `flex-direction` |
| `-webkit-flex-wrap` | `flex-wrap` |
| `-ms-flex-align` | `align-items` |
| `-ms-flex-pack` | `justify-content` |
| `-ms-flex-negative` | `flex-shrink` |
| `-webkit-box-align` | `align-items` (旧仕様) |
| `-webkit-box-pack` | `justify-content` (旧仕様) |

### 実装箇所
- `src/css/style.rs` の `compute_style_with_pseudo` でプロパティ名変換

## 受け入れ条件

- ベンダープレフィックス付きプロパティが標準プロパティとして適用される
- 標準プロパティが既にある場合は上書きしない
- 既存テスト全通過
