---
number: 037-6
slug: nowrap-cross-segment
parent: 037-rendering-precision-improvements
status: open
---

# nowrap が複数 inline segment 境界で折り返される

## 問題

`white-space: nowrap` のコンテナ内で `<em>` 等のインライン要素があると、
テキストが複数の InlineSegment に分割され、segment 間で折り返しが発生する。

## 例

```html
<div style="white-space: nowrap">Hello <em>world</em> this is long text</div>
```

各 segment は nowrap だが、`layout_inline_segments` の行折り返し判定が
segment 単位で動くため、segment 境界で折り返される。

## 修正方針

`layout_inline_segments` で前の segment と同じ nowrap コンテナに属する場合は
折り返しを抑制する。
