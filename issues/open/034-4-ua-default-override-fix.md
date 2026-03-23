---
number: 034-4
slug: ua-default-override-fix
parent: 034-css-spec-compliance-fixes
status: open
---

# UA default が author CSS を上書きする問題

## 概要

heading 要素の UA default（font-size, font-weight, margin）が `or_insert` で適用されているため、author CSS で明示的に上書きしても UA default が優先されてしまう。

## 問題

```rust
if defaults.font_weight_bold {
    properties
        .entry("font-weight".to_string())
        .or_insert(ComputedValue::Keyword("bold".to_string()));
}
```

`or_insert` は key が存在しない場合のみ挿入する。author CSS で `font-weight: normal` が設定されていれば、`properties` に既に `font-weight` が存在するため `or_insert` は何もしない。

**これは正しい動作。** cascade で author > UA なので、author CSS が先に適用された後に UA default を `or_insert` するのは仕様通り。

## 実際の問題

UA default の適用タイミングが cascade の後なので、実際には正しく動作している。ただし:
- `margin` の UA default は `or_insert` ではなく毎回上書きしている可能性
- `font-size` の UA default が `compute_style_with_pseudo` 内で cascade 前に適用されていないか確認が必要

## 確認事項

1. `h1 { font-weight: normal }` が正しく font-weight: normal になるか
2. `h1 { margin: 0 }` が正しく margin: 0 になるか
3. `h1 { font-size: 12px }` が UA の 2em を上書きするか

## 修正箇所

- `src/css/style.rs` の heading UA default 適用ロジックを確認・テスト追加
