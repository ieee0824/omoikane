---
number: 070
slug: flex-column-grow-child-reflow
parent:
status: open
---

# flex column の flex-grow 分配後に子を再レイアウトする

## 概要

`flex-direction: column` の `flex-grow` による余剰スペース分配が、子のレイアウト完了後に
`content.height` を直接加算するだけなので、子の内部（子孫）が新しい高さで再レイアウトされない。

## 背景（PR #169 Copilot レビューより）

https://github.com/ieee0824/omoikane/pull/169#discussion_r3595517695

`src/layout/flex.rs` の column 方向の grow 分配（`layout_flex_container` 内）は、
`layout_node` で子をレイアウトした後に

```rust
child.dimensions.content.height += free_space * item.flex_grow / total_grow;
```

と外形の高さだけを書き換える。このため、伸長した高さに依存する子孫
（`height: 100%` などのパーセント高さ、align-items: stretch 相当の内容）は
伸長前のサイズのまま残り、column 方向の `flex-grow` で余剰が分配されたときに
子の内部レイアウトが不正確になる。

なお `resolve_flex_main_sizes` は事前に grow/shrink を分配した主軸サイズを
containing rect の height として子に渡しているため、子自身がパーセント高さを
持つケースはそちらで解決される。問題になるのは「auto 高さの grow アイテムの内部」。

## 対応方針（案）

- フレックスアイテムに確定した主軸サイズ（used height）を強制して `layout_node`
  を再実行する機構を導入する（現状 containing rect の height はパーセント解決にしか
  使われず、used height を外から与える手段がない）
- 高さが変化した grow アイテムのみ再レイアウトし、コストの増加を抑える
- row 方向の align-items: stretch にも同種の課題がないか合わせて確認する

## 受け入れ条件

- column flex コンテナで `flex-grow: 1` のアイテム内の `height: 100%` の子孫が、
  分配後の高さいっぱいに広がる（テストで期待値を明示する）
- 既存の flex テスト（min-height 分配含む）が通り続ける
