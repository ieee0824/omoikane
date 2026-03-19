---
number: 012-11
slug: smile-curve-border-trick
parent: 012-acid2-official-conformance
status: open
---

# smile の曲線形状（nested float/absolute/border）

## 概要

Acid2 の smile は `relative + absolute + nested float + border` の組み合わせで
曲線状の口を描画する。現在は黒い四角のままで曲線になっていない。

## CSS

```css
.smile div { margin-top: 0.25em; background: black; width: 12em; height: 2em; position: relative; bottom: -1em; }
.smile div div { position: absolute; top: 0; right: 1em; width: auto; height: 0; margin: 0; border: yellow solid 1em; }
.smile div div span { display: inline; margin: -1em 0 0 0; border: solid 1em transparent; border-style: none solid; float: right; background: black; height: 1em; }
.smile div div span em { float: inherit; border-top: solid yellow 1em; border-bottom: solid black 1em; }
.smile div div span em strong { width: 6em; display: block; margin-bottom: -1em; }
```

## 仕組み

1. `.smile div`: 黒い四角（12em × 2em）、relative + bottom:-1em で下に 1em オフセット
2. `.smile div div`: absolute positioned、黄色 border 1em（height:0）→ 黒背景を部分的に覆う黄色帯
3. `span`: float:right、transparent 左右 border、黒背景、height:1em
4. `em`: float:inherit(=right)、黄色上border + 黒下border → 口の曲線の上端
5. `strong`: display:block、width:6em → em の幅源、margin-bottom:-1em

## スコープ

- absolute div の黄色 border が正しく描画されること
- span の transparent border がスペースを確保しつつ描画されないこと
- `float: inherit` が親の float:right を正しく継承すること（実装済み）
- nested float の shrink-to-fit 幅が strong の width:6em を反映すること（実装済み）
- 全体が組み合わさって曲線状の口に見えること

## 進捗メモ

- absolute child of relative の yellow border 描画は正しく動作（テスト追加済み）
- smile の absolute div の auto width が 72px（期待: 96px 程度）。span の border-left/right(12+12) が shrink-to-fit 幅に含まれていない
- `intrinsic_width` が explicit width なしのときに自身の padding/border を加算していない。修正を試みたが `shrink_to_fit_width` の戻り値が outer width になることで layout_positioned_child 等に影響があり revert
- `intrinsic_width` を outer width で一貫する設計に変更済み。absolute div width が 72→96 に改善
- 比較テストで smile 領域の pixel を確認: relative div(黒) の上に absolute div(yellow border) が描画されるべきだが、pixel(118, 222) = 黒のまま
- paint 順序上は正しい: `.picture` の auto_positioned phase で relative → absolute の順で paint される
- relative の黒背景が先、absolute の黄色 border が後に描画されるはず
- **未解決**: absolute の yellow border が実際に paint_borders で描画されているのに見えない原因。translate_layout_box_for_test が absolute の border rect を正しく移動していない可能性。または別の要素が上書きしている可能性
