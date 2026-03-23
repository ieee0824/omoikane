---
number: 037-1
slug: ua-defaults-expansion
parent: 037-rendering-precision-improvements
status: open
---

# UA デフォルトスタイルの拡充

## 不足している要素のデフォルト

| 要素 | 不足プロパティ | CSS仕様値 |
|------|-------------|----------|
| `<blockquote>` | margin | 1em 40px |
| `<pre>` | font-family, white-space | monospace, pre |
| `<code>`, `<kbd>`, `<samp>` | font-family | monospace |
| `<dd>` | margin-left | 40px |
| `<th>` | font-weight, text-align | bold, center |
| `<a>` | text-decoration, color | underline, blue |
| `<sub>` | vertical-align, font-size | sub, smaller |
| `<sup>` | vertical-align, font-size | super, smaller |
| `<small>` | font-size | smaller |

## 修正箇所

- `src/css/style.rs` の `apply_ua_defaults()`
