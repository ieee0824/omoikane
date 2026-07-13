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
| 016-12 `implementation.createDocument` | 82/100 | 独立 Document の native 生成・文書単位 StyleResolver 登録、QName/namespace 検証、doctype のルート前挿入を実装。test 8/26/27 が新規 PASS（FAITHFUL/DIRECT 両モード 82/100、実測）。test 98 は createDocument を通過し、016-14 範囲の XML/XHTML document title 差で残存。 |
| 050 `@media` 評価と iframe 固有 viewport | **83/100** | media query の構文解析・評価（all/not/only、リスト OR、color/monochrome、min/max-width/height の px・em）と owning iframe のレイアウト content box に基づく文書固有 viewport、text-transform 初期値の露出を実装（PR #114）。test 46 が新規 PASS（FAITHFUL/DIRECT 両モード 83/100、実測）。 |
| 016-16 iframe/object load イベント | **83/100** | 接続時のサブ文書ロードとマクロタスクでの `load` dispatch、3種のハンドラ経路、`object[data]`、再接続時の再ロードを実装。test 65 の7ハンドラが完了し、test 69 は retry を抜けて後続の `t was null`（016-14）まで前進。FAITHFUL/DIRECT とも index 100、83/100（実測）。 |
| 016-13 table row API + フォーム送信/on* ハンドラ | 86/100 | HTMLTableElement.insertRow/deleteRow、HTMLTableSectionElement(rows/insertRow/deleteRow)、HTMLTableRowElement(cells/rowIndex/sectionRowIndex/insertCell/deleteCell)、IndexSizeError 境界検証、rows ゲッタのツリー順修正を実装（test 50/51 が PASS、83→85）。加えて submit/reset ボタン click の活性化挙動（所属 form への cancelable な submit/reset 同期発火）と on* イベントハンドラ IDL 属性を実装（test 54 が PASS、85→86）。FAITHFUL/DIRECT 両モード 86/100（実測）。test 29 は HTML パーサが `<tr>` を tbody 挿入せず table 直下に置くため（tree construction 側、本 issue スコープ外）残存。 |
| 051 cursor 値検証（無効宣言の破棄） | **87/100** | CSS 宣言のプロパティ別値検証フックを導入し、`cursor` の keyword/url() 文法検証・無効宣言のカスケード前破棄・初期値 `auto` フォールバック・inline style への同一検証適用を実装（PR #117）。test 47 が新規 PASS（PR #116 と合流後の main 実測で FAITHFUL/DIRECT 両モード 87/100）。 |
| 054 table tree construction（暗黙 tbody 生成） | **88/100** | HTML パーサに "in table" / "in table body" 挿入モードを実装。`<table>` 直下の `<tr>`（および `<td>`/`<th>`）に対し暗黙 `<tbody>`（必要なら `<tr>` も）を生成し、明示 `<thead>`/`<tbody>`/`<tfoot>` は二重ラップしない。section 閉じタグ後の table 内空白テキストの配置も仕様に寄せた。test 29（cloneNode + tBodies）が新規 PASS（FAITHFUL/DIRECT 両モード 88/100、実測）。test 5 は暗黙 tbody 前提（expectation 11）を通過したが、`document.forms` / `document.links` 未実装のため後続で残存。test 4 も同 API（`document.forms[0]`）が原因で残存（tbody 前まで到達せず）。foster parenting は本 issue でも対象外。 |
| 055 document.forms / document.links の HTMLCollection | **90/100** | Document の live HTMLCollection（`forms` / `links` / `images` / `anchors`）を実装。tree 順・index/`length`・`item`/`namedItem`・`name`/`id` 名前アクセスに対応し、保持した参照でも DOM 変更を反映する live 性を持つ（`collect()` を毎アクセス再実行）。`links` は `href` 属性を持つ `<a>`/`<area>` のみ、`anchors` は `name` を持つ `<a>` のみを対象とし、各コレクションはその Document 自身の tree にスコープされる（iframe contentDocument の form/link がメイン文書に混ざらない）。test 4 / test 5 が新規 PASS（FAITHFUL/DIRECT 両モード 90/100、実測）。加えて FAITHFUL harness の stall 判定を「保留タイマーが残る間は打ち切らない」よう修正（`has_pending_timers()` を条件に追加）。`document.links` が解決したことで新規に発火する test 80 の retry ループ（linktest の iframe onload 待ち）で FAITHFUL が index 80 で停止していた退行を解消し、両モードとも index 100 まで到達。 |
| 016-14-1 XML/XHTML サブ文書 | **92/100** | strict XML parser、XML MIME 分岐、名前空間／大小文字保持／doctype IDL、XHTML script 実行、XHTML `Document.title` / `forms` を実装。test 69 / 98 が新規 PASS（FAITHFUL/DIRECT 両モード 92/100、index 100）。test 70 は従来も空 skeleton により PASS だったが、不正 UTF-8 と非 UTF-8 encoding 宣言を parser 自身の fatal error にした。test 80 は接続済み iframe の `src` 再 navigation / 動的 onload 未配線で XML script の検査まで到達せず残存。 |
| 016-14-2 CSSOM / 016-14-3 SVG DOM | **97/100** | CSSOM（styleSheets/cssRules/insertRule/ownerNode、PR #123, test 72）と SVG DOM（SVGElement 基底 + SVGSVGElement/SVGRectElement/SVGTextContentElement、getSVGDocument、PR #125, test 74/75/77/79）を実装。FAITHFUL/DIRECT 両モード 97/100、index 100。残存は test 64（object の URL IDL 反射）/ 71（document.write 後の tree construction）/ 80（iframe 再 navigation・ネットワーク）。 |
| 064 object.data の URL 反射 | **98/100** | native binding `__omoikane_resolve_url` を追加し `HTMLObjectElement.data` を URL 反射化（相対→絶対解決、fragment 保持、空参照は base URL、解決失敗は属性値フォールバック）。test 64 が新規 PASS（PR #136、FAITHFUL/DIRECT 両モード 98/100、実測）。残存は test 71（065）/ 80（066）。 |

### 82/100 到達時（016-12 `createDocument` 実装）に新規 PASS したテスト

- test 8: 独立生成文書上の Range 境界操作（`setStart` / `setEndBefore` 等）
- test 26: 参照保持・GC ストレス下の独立生成文書 DOM 操作（FAITHFUL 約29.0秒、DIRECT 約27.7秒の速度警告あり）
- test 27: `createDocument` 不在に起因していた後続の null/undefined failure を解消
- test 98: `createDocument` と doctype 挿入は通過するが、生成 XHTML 文書の `title` 初期値が `null`（期待値 `""`）のため残存（016-14）

`cannot convert 'null' or 'undefined' to object` 系では test 27 のみ解消。test 29/64/72/79/80 は残存し、test 71 は別 failure（Document の子ノード数差）。

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
- ~~**016-11 NodeIterator / TreeWalker / Range**~~ → 実装済み（70→79）。test 8 も 016-12 の `createDocument` 実装で解放。
- **016-14 XML/XHTML・CSSOM・SVG DOM**: test 69-72, 74-80 の SVG/XML サブドキュメント系。
- ~~**016-16 iframe load イベント**~~ → 実装済み。test 65 の iframe×6 + object×1 の load が完了し、test 69 の kungFuDeathGrip/title/retry 部分を解消。残る `t was null` は 016-14 の XML/SVG パース範囲。
- ~~**016-13 table row API + フォーム送信**~~ → 実装済み（83→86）。test 50/51（insertRow/rowIndex/section）と test 54（submit ボタン click → form submit イベント）を解放。test 55/56（checkbox 移動・radio clone）は 016-10 のライブ選択状態モデルで既に PASS 済みと実測確認。
- ~~test 29~~ → 054 で解消（parser の暗黙 tbody 生成、87→88）。
- ~~**test 4 / test 5**~~ → 055 で解消（Document の live HTMLCollection `forms`/`links`/`images`/`anchors` を実装、88→90）。test 4 は `document.forms[0]`／`document.forms.form.elements[0]`、test 5 は `document.links[1].firstChild` が解決するようになり両モードで PASS。
- 残: test 64（object URI 解決）, 69/71/72/74/75/77/79/80/98（016-14 の SVG/XML/CSSOM + object/iframe URI + linktest networking）。test 80 は `document.links` 解決後 retry を経て `timeout -- could be a networking issue`（linktest の onload 未発火）へ前進したが、依然として残存。
- 97/100 時点の残り3テストは原因調査済みで 064/065/066 に分解（2026-07-13）:
  - test 64 → [064 object.data URL 反射](064-object-data-url-reflection.md)
  - test 71 → [065 document.write 完全文書パース + doctype IDL](065-document-write-full-document-parsing.md)
  - test 80 → [066 iframe src 再ナビゲーション + 動的 on* 配線](066-iframe-renavigation-dynamic-onload.md)

## 子issue

Acid3 ギャップ分析（`tests/fixtures/acid3/GAP_ANALYSIS.md`）に基づく分解。
実装順序の推奨は GAP_ANALYSIS.md セクション4 を参照。

- [x] [016-1 Acid3 ローカル実行ハーネス](../closed/016-1-acid3-harness.md)
- [x] [016-2 script-data / RAWTEXT / RCDATA トークナイザ](../closed/016-2-script-data-tokenizer.md)
- [x] [016-3 setTimeout 関数コールバック保持 + イベントループ統合](../closed/016-3-timer-callbacks-event-loop.md)
- [x] [016-4 load イベント発火 + on* インラインハンドラ配線](../closed/016-4-load-event-inline-handlers.md)
- [x] [016-5 data: URI スクリプト対応](../closed/016-5-data-uri-scripts.md)
- [x] [016-6 bucket6 (ECMAScript) 実測と微修正](../closed/016-6-bucket6-ecmascript.md)
- [x] [016-7 document.write 実装](../closed/016-7-document-write.md)
- [x] [016-8 getComputedStyle 実値化（044-2 と同根）](../closed/016-8-computed-style-exposure.md)
- [x] [016-9 iframe / contentDocument サブブラウジングコンテキスト](../closed/016-9-iframe-content-document.md)
- [x] [016-10 querySelector matcher 接続 + セレクタ拡充](../closed/016-10-css-selector-extensions.md)（PR #110, 63→70）
- [x] [016-11 NodeIterator / TreeWalker / Range](../closed/016-11-traversal-and-range.md)（PR #111, 70→79）
- [x] [016-12 DOM2 Core / 名前空間 / DOMException](../closed/016-12-dom2-core-namespaces.md)（PR #113 ほか, test 98 の残差は 016-14 へ）
- [x] [016-13 HTMLTableElement / Form / Input / Select / Button API](../closed/016-13-table-form-apis.md)
- [x] [016-14 XML/XHTML・CSSOM・SVG DOM](../closed/016-14-xml-cssom-svgdom.md)
- [x] [016-15 getComputedStyle のサブ文書対応（selectorTest 解放の前提）](../closed/016-15-computed-style-subdocument.md)（PR #109, 58→63）
- [x] [016-16 iframe の load イベント発火](../closed/016-16-iframe-onload-event.md)（PR #115, 83/100 維持、test 69 retry 解消）
- [x] [050 CSS media query の構文解析・評価と computed style 反映](../closed/050-css-media-query-evaluation.md)（PR #114, 82→83）
- [x] [051 CSS プロパティ値検証と computed style serialization](../closed/051-css-property-value-validation.md)（PR #117, test 47 PASS, 合流後 87/100）
- [x] [054 table tree construction（暗黙 tbody 生成）](../closed/054-parser-table-tree-construction.md)（PR #119, test 29 PASS, 87→88）
- [x] [055 document.forms / document.links の HTMLCollection](../closed/055-document-forms-links-collections.md)（PR #120, test 4/5 PASS, 88→90）
- [x] [064 HTMLObjectElement.data の URL 反射](../closed/064-object-data-url-reflection.md)（PR #136, test 64 PASS, 97→98）
- [ ] [065 document.write の完全文書パースと doctype IDL](065-document-write-full-document-parsing.md)（test 71）
- [ ] [066 接続済み iframe の src 再ナビゲーションと動的 on* 属性配線](066-iframe-renavigation-dynamic-onload.md)（test 80）
