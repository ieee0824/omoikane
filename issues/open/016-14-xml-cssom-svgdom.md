---
number: 016-14
slug: xml-cssom-svgdom
parent: 016
status: open
---

# XML/XHTML・CSSOM・SVG DOM

## 目的

高コスト・少得点の残り領域（XML/XHTML、CSSOM、SVG DOM）を実装する。優先度は最後。

## 背景（GAP_ANALYSIS.md セクション3 領域O/P/Q、セクション4 フェーズ5）

- Q. XML/XHTML: test 18,70,71,80（XML 整形式性検査、XHTML 名前空間スクリプト、doctype IDL）
- O. CSSOM: test 72（styleSheets / cssRules / insertRule / ownerNode + `<style>` 動的再スタイル）
- P. SVG DOM: test 69,74（SVG を DOM としてパース + getSVGDocument + SVG contentDocument）
- 得点/コスト比が最も低いため最後に回す。75〜79 は既に素通しで加点済みである点に留意。

## スコープ

- XML パーサ（整形式性検査）+ XHTML 名前空間スクリプト実行 + doctype IDL
- CSSOM: `styleSheets` / `cssRules` / `insertRule` / `ownerNode`
- SVG DOM + `getSVGDocument` + SVG contentDocument

## 受け入れ条件

- 各領域の対象テストが個別に検証できる
- Acid3 test 18,69,70,71,72,74,80 の前提を満たす

## 子 issue(2026-07-12 分割)

- [ ] [016-14-1 XML/XHTML サブ文書のパースと DOM 化](016-14-1-xml-subdocuments.md)（test 69/70/71/80/98 系の基盤）
- [x] [016-14-2 CSSOM](../closed/016-14-2-cssom.md)（PR #123, test 72 PASS, 単独 90→91）
- [ ] [016-14-3 SVG DOM 最小実装と getSVGDocument](016-14-3-svg-dom.md)（test 69/74/75/77/79、14-1 が前提）
