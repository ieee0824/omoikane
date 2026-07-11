---
number: 016
slug: acid3-conformance
parent:
status: open
---

# Acid3 対応

## 概要

Acid2 相当の静的レイアウト・描画互換性に続いて、
より広いブラウザ互換性を確認するために Acid3 を通す。

Acid3 は DOM / CSS / HTML parser / scripting / networking まで含む複合テストのため、
単一機能の修正ではなく段階的な分解と検証が前提になる。

## 背景

- 現状は Acid2 系の描画互換性までは一定の到達がある
- ただし実ブラウザ相当の互換性を測るには、動的挙動や API 面の不足がまだ大きい
- Acid3 を目標に置くことで、HTML/CSS 描画エンジン単体からブラウザ互換基盤全体へ進める

## 想定スコープ

- HTML parser / DOM の仕様差分の洗い出し
- CSS parser / selector / cascade の不足補完
- JavaScript 実行と DOM bindings の互換性向上
- タイマー・イベント・例外処理などブラウザ API の最小整備
- HTTP / data URI / encoding / resource loading の互換性向上
- Acid3 実行結果を継続確認できるテスト・観測基盤の整備

## 進め方

1. まず Acid3 を実行できる最小 harness を用意する
2. failure をカテゴリ別に分解して子issue化する
3. 影響範囲の狭い基盤差分から順に潰す
4. 最終的にスコアだけでなく、安定して再現できる CI/ローカル検証手順を固める

## 受け入れ条件

- Acid3 の実行とスコア取得がローカルで再現できる
- 主要 failure が子issueに分解され、追跡可能になっている
- 最終的に Acid3 が通過、または少なくとも未達成項目が明確に整理されている

## 備考

- Acid3 は広範囲の互換性を要求するため、短期完了前提ではなく段階実装とする
- 必要に応じて parser / DOM / CSS / JS / networking / harness の各観点で子issueへ分割する

## スコア推移（`cargo run --example acid3`）

| 時点 | スコア | 主な変化 |
| --- | --- | --- |
| 初期 harness 導入時 | 0/100 | ドライバ未接続 |
| 016-2〜016-3 | 26/100 | トークナイザ + タイマーコールバック |
| 016-4〜016-5 | 28/100 | load イベント + data: URI スクリプト |
| 016-12/016-13 部分 + 016-6 | 43/100 | DOMException 基盤・名前検証・createElementNS・createEvent 補完・table/form/input/button/label/meta/select 反射・Boa annex-b |
| 016-7 + 016-9 + 016-8/044-2（PR #103/#104/#105） | **58/100** | document.write/open/close・iframe/contentDocument サブブラウジングコンテキスト・getComputedStyle 実値化・レイアウトメトリクス（offset*/client*/scroll*/getBoundingClientRect）・forced reflow |
| 016-15 実装（resolver 文書単位化） | 61/100 | getComputedStyle をノードの owner document 基準に解決（文書単位 StyleResolver キャッシュ）+ サブ文書 defaultView を contentWindow に接続。selectorTest bucket3 の test 36/41/42 が新規 PASS |
| 016-15 実装（title 反射 + :first-child 修正） | **63/100** | HTMLElement.title の IDL 反射を追加し、:first-child/:last-child/:nth-child がルート要素（親が Document）にマッチしていたマッチャのバグを修正。test 33/35 が新規 PASS（FAITHFUL/DIRECT 両モード 63/100、実測） |
| 016-10 セレクタ拡充 + querySelector matcher 統合 | **70/100** | strict selector-list parser、querySelector 系の CSS matcher 統合、`:lang` / child・of-type・`:empty` / UI 状態擬似クラス、フォーム checkedness を実装。test 34/37/38/39/40/43 が新規 PASS。getElementById の selector 依存も解消して test 28 が PASS（FAITHFUL/DIRECT 両モード 70/100、実測） |
| 016-11 Traversal / Range | **79/100** | NodeIterator / TreeWalker / Range と live 変異補正を純 JS で実装。test 1-3, 6-7, 9, 11-13 が新規 PASS（FAITHFUL/DIRECT 両モード 79/100、実測）。test 4-5 は HTML tree/API 前提差、test 8 は `implementation.createDocument` 未実装のため残存。 |

### 70/100 到達時（016-10 実装）に新規 PASS したテスト

- test 34: `:lang()` の継承・言語 prefix と `[class|=...]`
- test 37: `:only-child` と DOM 変異後の再計算
- test 38: `:empty`（Comment と空 Text を無視し、内容のある Text/Element を判定）
- test 39: 一般 `an+b` と負係数を含む `:nth-child()` / reverse position
- test 40: first/last/only/nth/nth-last の of-type 系
- test 43: `:enabled` / `:disabled` / `:checked`、checkbox/radio の live checkedness・dirty checkedness・radio group 排他
- test 28: `getElementById` を querySelector 経由から ID 文字列完全一致走査へ変更したことで、selector として特殊な ID も正しく検索

実測では test 37/39/40（および実行によって 33/42）が 30fps 閾値超過の警告を出す場合があるが、assertion 自体は PASS。FAITHFUL が DIRECT より低くなる差やハングはない。

### 63/100 到達時（016-15 実装）に新規 PASS したテスト

- test 36（:last-child）, 41（:root / :not(:root)）, 42（`+` `~` `>` ` ` の動的組み合わせ）: selectorTest が iframe contentDocument の `<style>` を `doc.defaultView.getComputedStyle` で解決できるようになり解放
- test 33（`[title=...]` 属性セレクタ）: `p.title = ...` の `HTMLElement.title` IDL セッターが `title` 属性へ反射するよう実装し解放
- test 35（:first-child）: ルート要素（親が `Document` ノード）が `:first-child` を主張していたマッチャのバグを修正（位置ベースの構造疑似クラスは親が `Element` の場合のみ成立）し解放

### 43/100 到達時（016-12/016-13 部分 + 016-6）に新規 PASS したテスト

- 016-12 分: test 19（appendChild HIERARCHY_REQUEST_ERR の JS 例外化）, 20（NUL バイト → INVALID_CHARACTER_ERR）, 21（createElementNS 名前空間プロパティ）, 22（createElement 名前検証）, 23（createElementNS 名前検証と例外コード）, 25（DOMException 定数群 + createDocumentType）, 30（createEvent('UIEvents')/initUIEvent/detail）
- 016-13 分: test 49（table.caption/tHead/tFoot/tBodies/rows/create*/delete*）, 53（input.name 反射 + value dirty value + form.elements 名前/index アクセス）, 57（select.add/options）, 58（option.defaultSelected/select.selectedIndex）, 59（button 既定 type=submit）, 62（label.htmlFor / meta.httpEquiv 反射）
- 016-6 分: test 68（input value の孤立サロゲート保持。JS メモリ内 dirty value 化で FFI 越えを回避）, 85（Boa `annex-b` フィーチャ有効化で `String.prototype.substr` 提供）

### 旧・主要ブロッカー（解消済み: 2026-07-11、43→58）

- ~~**016-7 document.write**~~ → PR #103 で実装（43→52）。
- ~~**016-9 iframe / contentDocument**~~ → PR #104 で実装（52→56）。
- ~~**016-8 getComputedStyle 実値化 + 044-2**~~ → PR #105 で実装・統合（統合実測 58）。

### 残りの主要ブロッカー（70/100 時点の失敗傾向）

- ~~**016-10 querySelector matcher 接続 + セレクタ拡充**~~ → 実装済み（63→70）。test 34/37/38/39/40/43 と、getElementById 経路修正により test 28 を解放。
- **050 CSS media query 評価**: test 46（`@media`、iframe viewport、`text-transform` computed style）。
- **051 CSS プロパティ値検証**: test 47（無効な `cursor` 宣言の破棄、初期値 `auto`、serialization）。issue 031 と相互参照。
- **016-11 NodeIterator / TreeWalker / Range**: test 01-09, 11-13 等の「not a callable function」群。
- **016-14 XML/XHTML・CSSOM・SVG DOM**: test 69-72, 74-80 の SVG/XML サブドキュメント系。
- **016-16 iframe load イベント**: test 65/69 の kungFuDeathGrip。
- 残: test 26-27, 29, 50-51, 54-56, 64, 98 等（016-12/016-13 の残り + イベント dispatch）。

## 子issue

Acid3 ギャップ分析（`tests/fixtures/acid3/GAP_ANALYSIS.md`）に基づく分解。
実装順序の推奨は GAP_ANALYSIS.md セクション4 を参照。

- [x] [016-1 Acid3 ローカル実行ハーネス](../closed/016-1-acid3-harness.md)
- [x] [016-2 script-data / RAWTEXT / RCDATA トークナイザ](../closed/016-2-script-data-tokenizer.md)
- [x] [016-3 setTimeout 関数コールバック保持 + イベントループ統合](../closed/016-3-timer-callbacks-event-loop.md)
- [x] [016-4 load イベント発火 + on* インラインハンドラ配線](../closed/016-4-load-event-inline-handlers.md)
- [x] [016-5 data: URI スクリプト対応](../closed/016-5-data-uri-scripts.md)
- [ ] [016-6 bucket6 (ECMAScript) 実測と微修正](016-6-bucket6-ecmascript.md)
- [x] [016-7 document.write 実装](../closed/016-7-document-write.md)
- [x] [016-8 getComputedStyle 実値化（044-2 と同根）](../closed/016-8-computed-style-exposure.md)
- [x] [016-9 iframe / contentDocument サブブラウジングコンテキスト](../closed/016-9-iframe-content-document.md)
- [x] [016-10 querySelector matcher 接続 + セレクタ拡充](../closed/016-10-css-selector-extensions.md)（PR #110, 63→70）
- [ ] [016-11 NodeIterator / TreeWalker / Range](016-11-traversal-and-range.md)
- [ ] [016-12 DOM2 Core / 名前空間 / DOMException](016-12-dom2-core-namespaces.md)
- [ ] [016-13 HTMLTableElement / Form / Input / Select / Button API](016-13-table-form-apis.md)
- [ ] [016-14 XML/XHTML・CSSOM・SVG DOM](016-14-xml-cssom-svgdom.md)
- [x] [016-15 getComputedStyle のサブ文書対応（selectorTest 解放の前提）](../closed/016-15-computed-style-subdocument.md)（PR #109, 58→63）
- [ ] [016-16 iframe の load イベント発火](016-16-iframe-onload-event.md)
- [ ] [050 CSS media query の構文解析・評価と computed style 反映](050-css-media-query-evaluation.md)（test 46）
- [ ] [051 CSS プロパティ値検証と computed style serialization](051-css-property-value-validation.md)（test 47、031 と相互参照）
