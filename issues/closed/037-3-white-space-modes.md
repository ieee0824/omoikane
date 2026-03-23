---
number: 037-3
slug: white-space-modes
parent: 037-rendering-precision-improvements
status: open
---

# white-space nowrap/pre-wrap/pre-line 対応

## 現状

`normal` と `pre` のみ対応。

## 追加対応

| 値 | 空白折りたたみ | 改行 | 折り返し |
|----|-------------|------|---------|
| `nowrap` | する | なし | しない |
| `pre-wrap` | しない | する | する |
| `pre-line` | する | する | する |
