# Acid3 対応 静的ギャップ分析 (Omoikane)

> **注記**: 本書は 2026-07-10 時点の静的分析（ソース突合）である。ベースライン実測値
> （`cargo run --example acid3` のスコア等）は `tests/fixtures/acid3/README.md` を参照。
> 静的分析時点の想定と実測が食い違う場合は README の実測を優先する。

- 対象: Acid3 実物ソース (`http://acid3.acidtests.org/`, HTML 3508行) を取得して解析
- Omoikane 側: `src/js/mod.rs`(ネイティブバインディング), `src/js/dom_bootstrap.js`(JSポリフィル994行), `src/dom/`, `src/css/`, `src/http/`, `src/svg/` を突合
- リポジトリは読み取りのみ (cargo/ビルドは実行していない)
- 構成: 100サブテスト (0〜99)。バケットは各テスト関数の `return N`(N=バケット番号)で判定した。

## Omoikane 実装アーキテクチャの要点(前提理解)

- JSエンジンは **Boa 0.21**。DOM APIは「薄いネイティブ primitive (`__omoikane_*`, 約30個) + `dom_bootstrap.js` でJS側にDOMを構築」という二層構造。
- ネイティブ primitive 全一覧: `get_element_by_id`, `query_selector`, `query_selector_all`, `create_element`, `create_text_node`, `create_comment`, `create_document_fragment`, `append_child`, `insert_before`, `remove_child`, `clone_node`, `parent_node`, `next_sibling`, `previous_sibling`, `child_node_ids`, `node_name`, `node_type`, `get_attribute`, `set_attribute`, `remove_attribute`, `get/set_text_content`, `get/set_inner_html`, `fetch`, `console_log`, `setTimeout/setInterval/clearTimeout/clearInterval`, `document_id`, `location_href`, `navigator_user_agent`。
- **querySelector系はJS層で単純セレクタ (`tag` / `.class` / `#id`) のみ対応**(`matches_simple_selector`, mod.rs:1184)。複合・属性・擬似クラスは非対応。ただし `getElementsByTagName`/`getElementsByClassName`/`document.body|head|documentElement` は動く。
- CSSカスケード用の本格マッチャ `src/css/matcher.rs` は存在するが **querySelectorには接続されていない**。かつ対応擬似クラスは限定的(下記)。
- Rust側DOMモデルは Document / DocumentFragment / Element / Text / Comment / **DocumentType** をサポート。`.data()` はRust側に存在するが **JS `Node` に `.data` プロパティが未露出**。
- HTTPクライアントは **http/https のみ**。`data:` スキーム非対応。

---

## セクション1: ハーネス前提条件チェックリスト(テスト0点回避に必須)

Acid3ドライバ(`update()` の setTimeout チェーン)と `getTestDocument()`/`selectorTest()` が要求する基盤。**これらが欠けると多数のテストが連鎖的に0点になる。**

| # | 前提機能 | ドライバ/ハーネスでの使用箇所 | Omoikane状況 | コスト | 影響範囲 |
|---|----------|------------------------------|-------------|-------|---------|
| P1 | `<body onload="update()">` 起動 = **インラインイベントハンドラ属性の実行** | 全テストの起動トリガ。これが無いとスコアは "JS" のまま | ❌ 未実装 (inline `onXxx` 属性を listener に配線していない。`fire_document_event`はあるが `load` を dispatch しても onload属性は呼ばれない) | M | 全100テスト |
| P2a | `setTimeout(update, 10)` の**関数引数**での再帰チェーン | ドライバのループ機構 (acid3.html:3439,3470 は関数参照 `update` を渡す) | ❌ 未実装 (**第2の致命ブロッカー**) | M | 全テスト |
| P2b | イベントループの実パイプライン駆動 + ランタイム永続化 | `setTimeout` マクロタスクを時間経過で実行し続ける | 🟡 部分的 (API有・未接続) | M | 全テスト |
| P2c | `new Date()` 経過時間計測 | 30fps判定 (失敗ではなく減点) | ✅ 実装済 (Boa Date) | — | — |
| P3 | `document.getElementById` / `getElementsByTagName` | 各所 (`getElementById('score')`, `getElementsByTagName('script')` 等) | ✅ 実装済 (getElementByTagNameはqsa経由でtag対応) | — | 多数 |
| P4 | テキストノードの **`.data` getter/setter** | ドライバ: `span.firstChild.data = score` / `... .data = tests.length`。テスト4,5,12,13,26等でも `node.data` を参照 | ❌ 未実装 (JS `Node` に `data` プロパティ無し。`nodeValue`/`textContent` のみ) | S | ドライバ表示 + テスト4,5,12,13,26 |
| P5 | **`getComputedStyle` の実値** (defaultViewとglobal両方) | test0: `document.defaultView.getComputedStyle(penultimate,'').whiteSpace === 'pre-wrap'`。`selectorTest`: `getComputedStyle(node,'').zIndex` で全セレクタ判定 | ❌ 未実装 (`getComputedStyle` は常に "" を返すスタブ。カスケード結果はJSに露出していない) | XL | test0, 33〜44, 47 |
| P6 | **`document.defaultView`** | test0 / selectorTest が `doc.defaultView.getComputedStyle` を使用 | ❌ 未実装 (defaultView プロパティ無し) | S | test0, 33〜44, 47 |
| P7 | **iframe + `contentDocument`** (別ブラウジングコンテキスト) | `getTestDocument()` = `document.getElementById("selectors").contentDocument`。空HTMLをロードしたiframeの独立documentを操作 | ❌ 未実装 (iframe要素はパースされるがサブドキュメント/contentDocumentの概念が無い) | XL | test1,2,3,6,9,11,12,13,33〜44,46,65〜74 |
| P8 | **`document.write()`** | body末尾で `document.write('<map>...<iframe id="selectors">...')` により selectors iframe や map/form/table を生成 | ❌ 未実装 | L | selectors iframe生成→getTestDocument, test4,52,63等 |
| P9 | 例外機構: `throw {message}` / try-catch / `e.code` / DOMException定数 | `fail()`, 各テストのエラー捕捉 | 🟡 部分的 (Boaのtry/catch/throwは動くが、DOMExceptionオブジェクト & `.code`/定数(`HIERARCHY_REQUEST_ERR`等)を投げる仕組みが無い) | M | test8,11,19,20,22,23,25 |
| P10 | `removeChild`/`appendChild`/`insertBefore`/`removeAttribute`/`previousSibling`/`nextSibling` | 各所 | ✅ 実装済 (reparent含めネイティブ+テスト有り) | — | 多数 |

**サマリ**: 12項目中 **実装済3 (P2c/P3/P10) / 部分2 (P2b/P9) / 未実装7 (P1/P2a/P4/P5/P6/P7/P8)**。

**起動を止めている独立ブロッカーは2つ** (どちらか単独でもスコア=0):
- **P1 (onload属性実行)**: `<body onload="update()">` が評価されず、そもそも `update()` が1回も呼ばれない。ソース全体で `on*` 属性をリスナに配線する処理が皆無、かつ `load` イベントもパイプラインから発火されない (`src/paint/mod.rs:698-717` は `execute_document_scripts` を1回呼ぶのみ)。
- **P2a (setTimeout関数引数)**: 仮に `update()` を手動起動できても、`setTimeout(update, delay)` は関数参照を `to_string()` してソース文字列として保存する実装 (`src/js/mod.rs:680-685`) のため、後で eval されても関数宣言が巻き上がるだけで**呼び出されない** → チェーンが1回で停止。加えて P2b: `execute_document_scripts` 後に `tick()` が呼ばれずランタイムも drop されるため、マクロタスクは消化されない。

その他の最重要ブロッカー: P5+P6 (getComputedStyle実値+defaultView → bucket3全体とtest0/47), P7+P8 (iframe/contentDocument/document.write → `getTestDocument` 依存の約35テスト), P9 (DOMException `.code`/定数 → test8/11/19/20/22/23/25)。

**結論**: 現状 Omoikane では **Acid3 スコアは 0/100**。P1・P2a・P2b を通して「テスト関数が順に呼ばれ、setTimeoutチェーンが回る」状態を作るのが最優先で、それ無しには以降のバケット実装は一切採点に反映されない。

---

## セクション2: テスト 0〜99 ギャップ表

凡例: 状況 = ✅実装済 / 🟡部分的 / ❌未実装 / 🔵Boa依存(素通しの可能性)。コスト = S/M/L/XL。「(P7)」等は該当前提条件ブロッカー。

| # | バケット | 検証内容 | 必要機能 | Omoikane状況 | コスト |
|---|---------|---------|---------|-------------|-------|
| 0 | 特殊 | 最終子削除時のスタイル再計算, script削除, `:last-child`, `pre-wrap` | getComputedStyle実値, defaultView, script除去 | ❌ (P5,P6) | XL |
| 1 | 1 DOM Traversal | NodeFilterと例外転送 (NodeIterator/TreeWalker) | createNodeIterator, createTreeWalker, NodeFilter例外転送 | ❌ (P7 + Iterator/Walker未実装) | L |
| 2 | 1 | 反復中のノード削除 (NodeIterator参照ノード追従) | createNodeIterator + live追従 | ❌ (P7 + Iterator) | L |
| 3 | 1 | 無限イテレータ (フィルタ内での木構造変更) | createNodeIterator | ❌ (P7 + Iterator) | L |
| 4 | 1 | 空白テキストノードの除外 (NodeIterator, whatToShow) | createNodeIterator + whatToShowビットマスク + `.data` | ❌ (Iterator, P4; メイン文書上なのでP7不要) | L |
| 5 | 1 | 空白テキストノードの除外 (TreeWalker) | createTreeWalker + `.data` | ❌ (Walker, P4) | L |
| 6 | 1 | 木の外を歩く (TreeWalker + 再グラフト) | createTreeWalker | ❌ (P7 + Walker) | L |
| 7 | 1 DOM Range | 基本Range (collapse, cloneContents, insertNode等) | document.createRange + Range全般 | ❌ (Range未実装) | L |
| 8 | 1 | 境界点の移動, setEnd/setStartBefore/selectNode | createRange + implementation.createDocument + `e.code`/INVALID_NODE_TYPE_ERR | ❌ (Range, F, P9) | L |
| 9 | 1 | extractContents() (Document内, DocumentFragment生成) | createRange.extractContents + `.toString()` | ❌ (P7, Range) | L |
| 10 | 1 | Rangeと属性ノード | (2011でコメントアウト、`return 1` の素通し) | ✅ 素通し | — |
| 11 | 1 | Rangeとコメント (surroundContents, HIERARCHY_REQUEST_ERR) | createRange.surroundContents + `e.code` | ❌ (P7, Range, P9) | L |
| 12 | 1 | Range変異: テキストノードへの挿入 (splitText) | createRange.insertNode + splitText + `.data` | ❌ (P7, Range, P4) | L |
| 13 | 1 | Range変異: 削除時の境界点補正 | createRange + live境界更新 | ❌ (P7, Range) | L |
| 14 | 1 HTTP | `Content-Type: image/png` をHTMLとして解釈しない | iframe(empty.png)のcontentDocument + MIME判定 | ❌ (P7 + MIME) | L |
| 15 | 1 | `Content-Type: text/plain` をHTMLとして解釈しない | iframe(empty.txt)のcontentDocument | ❌ (P7 + MIME) | L |
| 16 | 1 | `<object>` 入れ子とHTTPステータス処理 | object要素 + data属性 + 例外を投げない | 🟡 (要素作成/appendは動くが object のリソース取得/フォールバック無し。例外は投げないので条件付きPASS可) | M |
| 17 | 2 DOM2 Core | hasAttribute (欠落/含意/実在) | hasAttribute | ✅ 実装済 (bootstrap hasAttribute) | S |
| 18 | 2 | nodeType (document=9, element=1, text=3, **DOCTYPE=10**) | node_type + doctypeがfirstChildに存在 | 🟡 (nodeType値は正しい。ただしパース後 `document.firstChild` がDOCTYPEノードか要確認) | M |
| 19 | 2 | 定数値 (DOCUMENT_FRAGMENT_NODE=11 等) + HIERARCHY_REQUEST_ERR | Node/例外の定数プロパティ + appendChildの循環検出 | ❌ (Nodeに定数群無し, P9, 循環検出) | M |
| 20 | 2 | 各所のNULバイト (`getElementById`, `createElement`でINVALID_CHARACTER_ERR) | NUL含む名前で `e.code==5` を投げる | ❌ (createElementの名前検証+例外無し, P9) | M |
| 21 | 2 | 基本ネームスペース (createElementNS) | createElementNS + prefix/localName/namespaceURI | ❌ (createElementNS未実装) | M |
| 22 | 2 | 不正タグ名でcreateElement → INVALID_CHARACTER_ERR(5) | createElementの名前検証 + DOMException | ❌ (検証/例外無し, P9) | M |
| 23 | 2 | 不正タグ名でcreateElementNS → code 5/14(NAMESPACE_ERR) | createElementNS + 名前/NS検証 | ❌ (P9, NS) | M |
| 24 | 2 | イベントハンドラ属性の値取得 (`body.getAttribute('onload')`) | getAttribute('onload') の生値 | ✅ 実装済 (getAttribute) ※body onload属性がパース保持されていれば | S |
| 25 | 2 | createDocumentTypeのNS検査 + DOMExceptionオブジェクト | implementation.createDocumentType + NAMESPACE_ERR + 定数 | ❌ (implementation, P9) | M |
| 26 | 2 | 参照保持での文書木の生存 (GCストレス + 反復DOM操作) | implementation.createDocument, insertBefore/removeChild反復, `.data` | ❌ (F, P4; Rc生存自体はOK) | M |
| 27 | 2 | test26の継続 (テスト跨ぎでノード生存) | (kungFuDeathGrip保持) | 🟡 (グローバル変数保持は動く。test26成立が前提) | S |
| 28 | 2 | getElementById (name=""と混同しない, " " ID) | getElementById厳密一致 | ✅ 実装済 (id属性一致で判定) | S |
| 29 | 2 | クローン時の空白保持 (table.cloneNode) + tBodies/rows/cells | cloneNode(deep) + HTMLTableElement API | 🟡 (cloneNodeは有り。tBodies/rows/cells等のtable DOM API無し) | M |
| 30 | 2 Events | dispatchEvent + createEvent('UIEvents') + initUIEvent + detail | createEvent(UIEvents), initUIEvent, event.detail, add/removeEventListener | 🟡 (dispatch/add/removeは実装。createEventは `initEvent` のみで **initUIEvent/detail 無し** → 失敗) | M |
| 31 | 2 | stopPropagation + capture (二重リスナ, reset input) | addEventListener capture, click(), stopPropagation, eventPhase | 🟡 **ほぼ動作可** (bootstrapのdispatchはcapture/bubble/phase/target/stopPropagation対応。input.click()はMouseEvent発火。要検証) | S |
| 32 | 2 | Documentノードを通るバブリング | bubble経路にdocument含む | 🟡 **ほぼ動作可** (parentNode経路にdocumentが入る。要検証) | S |
| 33 | 3 Selectors | クラス/属性セレクタ (大小区別, `[class=]`, `[title=]`, `[align="..."]`) | selectorTest基盤 + カスケードでのクラス/属性マッチ | ❌ (P5,P6,P7,P8。属性マッチ自体はmatcher.rsに有り) | XL |
| 34 | 3 | `:lang()` と `[\|=]` | カスケードで `:lang`, DashMatch | ❌ (P5-P8 + `:lang`未実装。`[\|=]`はmatcher有り) | L |
| 35 | 3 | `:first-child` (動的) | `:first-child` | ❌ (P5-P8。`:first-child`はmatcher有り) | L |
| 36 | 3 | `:last-child` (動的) | `:last-child` | ❌ (P5-P8。`:last-child`はmatcher有り) | L |
| 37 | 3 | `:only-child` (動的) | `:only-child` | ❌ (P5-P8 + `:only-child`未実装) | L |
| 38 | 3 | `:empty` | `:empty` | ❌ (P5-P8 + `:empty`未実装) | L |
| 39 | 3 | `:nth-child`, `:nth-last-child` (an+b式) | nth an+b パーサ + `:nth-last-child` | ❌ (P5-P8 + nthは整数/odd/evenのみ, an+b・nth-last未実装) | L |
| 40 | 3 | `:first/last/only/nth/nth-last-of-type` | of-type系擬似クラス全般 + an+b | ❌ (P5-P8 + of-type系全て未実装) | L |
| 41 | 3 | `:root`, `:not()` | `:root`, `:not()` | ❌ (P5-P8。`:root`/`:not`はmatcher有り) | L |
| 42 | 3 | `+ ~ >` と子孫結合子の動的評価 | 結合子 + 動的再マッチ | ❌ (P5-P8。結合子はmatcher有り) | L |
| 43 | 3 | `:enabled/:disabled/:checked` (input状態連動) | 状態擬似クラス + input checked/disabled のライブ状態 | ❌ (P5-P8 + `:enabled/:disabled/:checked`未実装 + input状態モデル) | L |
| 44 | 3 | `*` 直前のスペース無しセレクタの誤パース防止 | セレクタパーサ堅牢性 (`html*.test`) | ❌ (P5-P8。パーサ挙動の検証要) | L |
| 45 | 3 Style | `cssFloat` と style属性 | `element.style.cssFloat` の読み書き (float↔cssFloatマッピング) | 🟡 (style Proxyは有るが cssFloat 特殊名の対応無し。メイン文書上でP7不要) | M |
| 46 | 3 | メディアクエリ (`@media all and (...)`, not/only/カンマ) | getTestDocument + MQ評価 + getComputedStyle | ❌ (P5-P8 + media.rsのMQ評価をJSに接続) | L |
| 47 | 3 | `cursor` CSS3値 (32種) の getComputedStyle | getComputedStyleでcursor値保持 | ❌ (P5,P6,P7,P8) | XL |
| 48 | 3 | `:link` / `:visited` (訪問状態のプライバシー) | iframe再ロード + linkマッチ + onload属性 | ❌ (P1,P7,P8) | L |
| 49 | 4 Tables | table アクセサ (createCaption/tHead/tFoot, delete*, rows/tBodies) | HTMLTableElement API一式 | ❌ (table系DOM API未実装) | L |
| 50 | 4 | table構築と再構成 (insertRow, rowIndex, insertBefore/replaceChild) | HTMLTableElement/Section/Row API + rowIndex | ❌ (table系DOM API未実装) | L |
| 51 | 4 | 行の順序と生成 (thead/tfoot/tbody自動振り分け) | table API + rows集約順序 | ❌ (table系DOM API未実装) | L |
| 52 | 4 Forms | `<form>` と `.elements` | HTMLFormElement.elements (live, 名前/indexアクセス), form.length | ❌ (form/elements API未実装, P8でform生成) | L |
| 53 | 4 | `<input>` の動的変更 (name/type/value プロパティ反映) | HTMLInputElement (name/type/value反映, elements名前アクセス) | 🟡 (value/type getterは有るが name反映/form.elements連動無し) | L |
| 54 | 4 | パース済`<input>`の変更 + submit + onsubmit | input.type=submit, form submit既定動作, onsubmit, maxLength文字列型 | ❌ (form submit既定動作/onsubmit配線無し, P8) | L |
| 55 | 4 | 移動したcheckboxが状態保持 | input.checked のライブ状態(属性でなくIDL値) | 🟡 (checkedは `checked` **属性** に写像 → プロパティ状態と属性を混同。移動で保持されるかは要検証) | M |
| 56 | 4 | クローンしたradioボタンの状態保持 + name group | radio group排他 + checked状態 + cloneNode | ❌ (radio group排他ロジック無し, checked属性混同) | L |
| 57 | 4 | `HTMLSelectElement.add()` | select.add(), select.options | ❌ (select DOM API無し) | M |
| 58 | 4 | `HTMLOptionElement.defaultSelected` → selectedIndex | option.defaultSelected, select.selectedIndex | ❌ (option/select API無し) | M |
| 59 | 4 | `<button>` の type/value属性 | button.type既定(submit), value | 🟡 (type getterは属性直読。button既定"submit"や value≠textContent分離無し) | M |
| 60 | 4 Misc HTML | className vs "class" vs 属性ノード | className↔class属性連動 | ✅ **ほぼ実装済** (classNameは class属性に写像。メイン文書上P7不要。要検証) | S |
| 61 | 4 | className/class の空白保持 | className の空白そのまま保持 | ✅ **ほぼ実装済** (set_attributeで生値保持。要検証) | S |
| 62 | 4 | DOM属性とcontent属性が等価でない (`className` という属性名) | className プロパティ ≠ `getAttribute('className')` | 🟡 (classNameは 'class' 属性に写像。`getAttribute('className')`(大文字)は別物になるはず。buckets判定要検証) | M |
| 63 | 4 | `<area>` の属性 (href/shape/coords/alt) | area要素属性 + `document.write` の map/area | ❌ (P8で生成される要素。getAttribute自体はOK) | M |
| 64 | 4 | 属性のURI解決 (`object.data` 絶対URI化) + 非存在属性 | object.data のURI絶対化, 属性が prototype に漏れない | 🟡 (getAttributeはOK。`.data`のURI解決/絶対化は無し。`in`演算子での属性非漏洩は要検証) | M |
| 65 | 5 Competition | svg/html/xml ファイルをiframe/objectで動的ロード(後続準備) | iframe/object動的src + onloadイベント + contentDocument | ❌ (P1,P7) | XL |
| 66 | 5 | text/comment/documentノードの localName=null | node.localName | ❌ (JS Nodeに localName 無し) | S |
| 67 | 5 | (2013でコメントアウト、`return 5` 素通し) | — | ✅ 素通し | — |
| 68 | 5 | UTF-16サロゲート対 (input.value) | String.fromCharCode + input.value 保持 | 🔵 (Boaの文字列 + value getter。4通りいずれか許容なので通る可能性大) | S |
| 69 | 5 | サポートファイル読込確認 (svg iframeのtext要素操作) | contentDocument (SVG) + getElementsByTagName + removeChild | ❌ (P7 + SVG DOM) | XL |
| 70 | 5 | XMLエンコーディング (ISO-8859-1をUTF-8で送る整形式性エラー) | XMLパーサ + 整形式性エラー検出 + contentDocument | ❌ (P7 + XMLパーサ) | XL |
| 71 | 5 | HTML parsing (doctype publicId/systemId, head/body挿入) + `document.write/open/close` | doc.open/write/close (contentDocument上) + doctype IDL + HTMLツリー構築 | ❌ (P7,P8 + doctype IDL) | XL |
| 72 | 5 | `<style>`テキストノードの動的変更 + CSSOM (styleSheets/cssRules/insertRule) | CSSOM (styleSheets, cssRules, insertRule, ownerNode) + ライブ再スタイル | ❌ (CSSOM未実装, P7) | XL |
| 73 | 5 | 入れ子イベント (click/dispatchEvent再入 + HTMLEvents) | createEvent(HTMLEvents)+initEvent, button.click/dispatch再帰 | ❌ (P7上のbutton。createEvent自体は初期化のみ) | L |
| 74 | 5 | `getSVGDocument()` (iframe/object) | getSVGDocument + SVG contentDocument | ❌ (P7 + SVG DOM + getSVGDocument) | XL |
| 75 | 5 | (2011でコメントアウト、`return 5` 素通し) SMIL | — | ✅ 素通し | — |
| 76 | 5 | (同上) SMIL part2 | — | ✅ 素通し | — |
| 77 | 5 | (同上) 外部SVGフォント | — | ✅ 素通し | — |
| 78 | 5 | (同上) SVG textPath/getRotationOfChar | — | ✅ 素通し | — |
| 79 | 5 | (同上) svg:font | — | ✅ 素通し | — |
| 80 | 5 | iframe/object除去 + XHTMLスクリプト実行確認 + `:visited`プライバシー | notifications(cross-file), XHTML名前空間スクリプト実行 | ❌ (P7 + XHTMLパース + XLinkスクリプト) | XL |
| 81 | 6 ECMAScript | 末尾省略配列の length (`[,]`=1) | ES配列リテラル | 🔵 (Boaで通る想定) | — |
| 82 | 6 | 中間省略配列の length と `in` | ES配列 sparse | 🔵 (Boaで通る想定) | — |
| 83 | 6 | 配列メソッド (unshift戻り値, join(undefined)) | Array.prototype | 🔵 (Boaで通る想定) | — |
| 84 | 6 | 数値→文字列 (toFixed/toExponential/toPrecision, -0) | Number.prototype | 🔵 (Boaで通る想定) | — |
| 85 | 6 | 文字列 (substr負数) | String.prototype.substr | 🔵 (Boaで通る想定) | — |
| 86 | 6 | Date (引数なしメソッド → NaN) | Date | 🔵 (Boaで通る想定) | — |
| 87 | 6 | Date (2桁年の1900オフセット) | Date/Date.UTC | 🔵 (Boaで通る想定) | — |
| 88 | 6 | 識別子中のUnicodeエスケープ不可 (parse error) | eval + パーサのSyntaxError | 🔵 **要検証** (Boaパーサが `+` を識別子文字として拒否するか) | S |
| 89 | 6 | 正規表現 (空クラス `/[]/`, 孤立ブラケット) | Boa正規表現(regressクレート) | 🔵 **要検証** (regexエッジケース) | S |
| 90 | 6 | 正規表現 (NUL, 前方参照backref, 否定先読み) | Boa正規表現 | 🔵 **要検証** (前方参照backref・否定先読み) | S |
| 91 | 6 | プロパティ既定enumerable + for-in順序 | ES列挙 | 🔵 (Boaで通る想定) | — |
| 92 | 6 | Functionオブジェクト内部プロパティ (constructor DontEnum等) | ES Function意味論 | 🔵 (Boaで通る想定) | — |
| 93 | 6 | 名前付きFunctionExpressionのスコープ (名前がReadOnly束縛) | ES FunctionExpression束縛 | 🔵 **要検証** (名前の不変束縛) | S |
| 94 | 6 | 例外catchブロックのスコープ | ES catch束縛 | 🔵 (Boaで通る想定) | — |
| 95 | 6 | 式の型 (`a.length = "..."` の戻り値が string) | ES代入式意味論 | 🔵 (Boaで通る想定) | — |
| 96 | 6 | encodeURI/encodeURIComponent(NUL) → `%00` | グローバル関数 | 🔵 (Boaで通る想定) | — |
| 97 | 7(特殊) | `data:` URI解析 (`<script src=data:...>` の d1〜d5) | **data: スキーム取得** (escaped/base64/base64+空白/バックスラッシュ) | ❌ (HTTPクライアントがdata:非対応 → d1〜d5 全て "fail") | M |
| 98 | 7(特殊) | XHTMLとDOM (implementation.createDocument/DocumentType, createElementNS, doc.title動的, doc.forms) | implementation.createDocument+createDocumentType, createElementNS, document.title, document.forms | ❌ (F, G + title/forms IDL) | L |
| 99 | 7(特殊) | 最凶バグ (a.href setterがtextContentを壊さない) | `<a>`.href setter (属性のみ変更, 子ノード不変) | 🟡 (setAttributeベースなら子は不変。`.href`プロパティ setterの有無要確認。href getter/setterは未定義の可能性) | S |

---

## セクション3: 機能領域別の集計(領域を実装すると解放されるテスト)

コスト昇順・費用対効果重視で整理。「解放」は他ブロッカーも解消した前提の理論上限。

| 領域 | 実装内容 | 解放されるテスト | テスト数 | コスト |
|------|---------|----------------|---------|-------|
| **A. ハーネス起動** | インラインイベントハンドラ属性(`onload`)実行 = P1 | ドライバ起動そのもの(全テストの前提) | 0→(全100の門) | M |
| **B. ドライバ小物** | `.data`(P4) + `defaultView`(P6) + `Node.localName`/定数 | 66直接; 4,5,12,13,26,27の前提解除 | 1+α | S |
| **C. ECMAScript (Boa素通し)** | 実質新規実装不要(要検証4件のみ) | 81〜96 | **16** | S(検証のみ) |
| **D. getComputedStyle + カスケード露出** | カスケード計算結果をJS `getComputedStyle` に接続(P5) | 0, 47(cursor); 33〜44/46 の必須前提 | 2直接 +12前提 | XL |
| **E. iframe/contentDocument/document.write** | サブブラウジングコンテキスト(P7,P8) | getTestDocument系: 1,2,3,6,9,11,12,13,14,15,16,33-44,46,48,65,69,70,71,74,80 の前提 | 前提解除 約35 | XL |
| **F. NodeIterator / TreeWalker** | createNodeIterator/createTreeWalker + NodeFilter + whatToShow + 例外転送 + live追従 | 1,2,3,4,5,6 | **6** | L |
| **G. DOM Range** | createRange + 境界点/collapse/extract/clone/insert/surround/toString + live更新 | 7,8,9,11,12,13 (10は素通し) | **6** | L |
| **H. CSS セレクタ拡充** | of-type系, `:only-child`, `:empty`, nth an+b, `:nth-last-*`, `:lang`, `:enabled/:disabled/:checked` を matcher.rs に追加 | 34,37,38,39,40,43 のサブ条件 (33,35,36,41,42はmatcher既存で足りる) | 6サブ改善 | M |
| **I. DOM2 Core / 名前空間 / DOMException** | createElementNS, implementation.createDocument/DocumentType, DOMException(`.code`+定数), 名前検証, prefix/localName/namespaceURI | 8,19,20,21,22,23,25,26,98 | **9** | L |
| **J. DOM2 Events 補完** | createEvent(UIEvents/HTMLEvents)+initUIEvent+detail; 既存dispatchの検証 | 30,31,32,73 | **4** (31,32は既にほぼ動作) | M |
| **K. HTMLTableElement API** | table/thead/tbody/tfoot/caption/row/cell の DOM API + rowIndex + 自動振り分け | 29,49,50,51 | **4** | L |
| **L. HTMLFormElement/Input/Select/Button** | form.elements(live), input状態(checked/value IDL分離), radio group, select/option, button既定type | 52,53,54,55,56,57,58,59 | **8** | L |
| **M. HTML属性/反射の細部** | hasAttribute(済), getAttribute(済), `.data`のURI解決, `in`非漏洩, area属性 | 17,24,28,60,61,62,63,64,99 (一部は既にほぼPASS) | **9** (半数は軽微) | M |
| **N. data: URI** | HTTPクライアントに `data:` スキーム対応(escaped/base64/空白) | 97 (+ 画像/scriptのdata:) | **1** | M |
| **O. CSSOM** | styleSheets/cssRules/insertRule/ownerNode + `<style>`動的再スタイル | 72 | 1 | XL |
| **P. SVG DOM + getSVGDocument** | SVGをDOMとしてパース + getSVGDocument + SVG contentDocument | 69,74 (75-79は既に素通し) | 2 | XL |
| **Q. XML パーサ + XHTML** | XML整形式性検査, XHTML名前空間スクリプト, doctype IDL | 18,70,71,80 | 4 | XL |

---

## セクション4: 実装順序の推奨(依存関係 × 費用対効果)

### フェーズ0: ハーネスを起動させる(これが無いと0点)
1. **A: onload属性実行(P1)** — M。全テストの門。`load` 発火 + `on*` 属性→リスナ配線。
2. **A': setTimeoutの関数引数対応(P2a) + イベントループ駆動・ランタイム永続化(P2b)** — M。`setTimeout(fn, ms)` が**関数コールバックを保持・再実行**できるようにし(現状は文字列化のみ)、レンダリングパイプラインが `tick()`/`run_until_idle()` を繰り返し呼んでマクロタスクを消化する構造にする。**A と A' は両方揃って初めてドライバのループが回る**(片方だけでは1テストで停止 or 起動せず)。
3. **B: `.data`(P4) + `defaultView`(P6) + `Node.localName`/`Node`定数群** — S。ドライバ表示と多数テストの地味な前提。
- → この時点で「起動して100テストが順に走る」。

### フェーズ1: 最小コストで即得点(費用対効果 最大)
3. **C: ECMAScript(81〜96)の検証** — Sの検証のみ。**16テストが理論上ほぼ無改修で解放**。要検証は88(識別子Unicodeエスケープ), 89/90(regexエッジ), 93(名前付き関数式束縛)の4件だけ。Boaが素通しなら bucket6 が丸ごと点灯。**最優先の"タダ取り"領域**。
4. **M(軽量分)+ 17,24,28,60,61,62**: hasAttribute/getAttribute/className はほぼ実装済。メイン文書上で動くものの検証・微修正で 17,24,28,60,61 あたりを確保 — S。
5. **J: DOM2 Events(30,31,32)** — 31,32は現状のdispatch実装でほぼ通る見込み。30はcreateEventにinitUIEvent+detailを足すだけ(S〜M)。**bucket2の3点**をメイン文書上(P7不要)で確保可能。

### フェーズ2: 二大ブロッカーの解消(ここが本丸・高コスト)
6. **D: getComputedStyle + カスケード露出(P5)** — XL。test0/47と、bucket3全体(selectorTest)の必須前提。
7. **E: iframe/contentDocument/document.write(P7,P8)** — XL。getTestDocument依存の約35テストの門。**DとEは bucket3/5 を解放する二枚看板**で、両方揃って初めて効く(selectorTestはgetTestDocument内でgetComputedStyleを使う)。

### フェーズ3: Dが済んだ後に効くセレクタ/スタイル系
8. **H: CSSセレクタ拡充(matcher.rs)** — M。D+E完了後、bucket3(33〜44,46,47,48)を段階解放。既存matcherで足りる 33,35,36,41,42 が先に点灯、of-type/only-child/empty/nth an+b/lang/UI状態 を足して 34,37〜40,43 を追う。
9. **G: Range(7〜13)** と **F: NodeIterator/TreeWalker(1〜6)** — L×2。bucket1のDOM Traversal/Rangeで最大12点。E(contentDocument)前提のものが多い。純粋なDOM木操作なので、E完了後は独立に進められる。

### フェーズ4: HTML DOM 意味論
10. **I: DOM2 Core/名前空間/DOMException** — L。8,19〜23,25,26,98(9点)。P9(DOMException)は多くのテストが `e.code` を見るので早めに基盤化すると波及効果大。
11. **K: Table API(29,49,50,51)** と **L: Form/Input/Select/Button(52〜59)** — L×2。bucket4で最大12点。
12. **N: data: URI(97)** — M。単発1点だが独立・低リスク。scriptのdata:取得(`fetch_script_source`)も同時に直せる。

### フェーズ5: 高難度・少得点(後回し)
13. **Q: XML/XHTML(18,70,71,80)**, **O: CSSOM(72)**, **P: SVG DOM(69,74)** — XL。bucket5の難所。得点/コスト比が最も低いので最後。75〜79は既に素通しで点になっている点に留意。

### 依存関係の要約
- **A + A' → 全て**(起動 + ループ駆動。両方必須)
- **C は A + A' のみに依存**(最速16点)
- **D と E は互いに独立だが、bucket3(selectorTest)は D×E 両方が必要**
- **E は bucket1の約半分・bucket5の大半の前提**
- **H は D+E の後**
- **P9(DOMException)は I 内だが、8/11/19/20/22/23/25 に横断的**なので基盤として先行実装が望ましい

---

## セクション5: Boa 0.21 自体の制約でブロックされ得るテスト

DOM層とは別に、JSエンジン(Boa 0.21)の実装差で落ちる可能性がある候補(いずれも**要実機検証**、ハードブロッカーとは断定できない):

- **test 88**: 識別子内 `+` を SyntaxError にできるか(Boaパーサの識別子解釈)。
- **test 89**: 正規表現の空クラス `/[]/`・孤立ブラケット `/TA[])]/` のパース(Boaは `regress` クレート使用。JS準拠なら通る想定)。
- **test 90**: NULバイト・**前方参照backref** `/(\3)(\1)(a)/`・否定先読み `(?!...)`(前方参照backrefは実装差が出やすい)。
- **test 93**: **名前付きFunctionExpressionの名前が内部でReadOnly束縛**になるか(`functest = ...` が無効化される仕様)。Boaが仕様準拠なら通る。

bucket6(81〜96)は基本的にBoaで通る前提だが、上記4件はBoaのバージョン依存で失敗し得るため、実装計画では「まず16テストを実機で流してBoaの素通し率を測る」ことを推奨。仮に88/89/90/93が落ちても、それはBoa側の課題(自前JSエンジンでない限りアップストリーム依存)であり、Omoikaneの設計変更では解決しにくい唯一の領域である。

なお、ハーネスの `document.write(...<script>...)` で流し込まれるスクリプトの同期実行、`setTimeout` の30fps判定(33ms超で"less than perfect"ログ=**減点だが失敗ではない**)など、タイミング要件はBoa/イベントループの実行速度に依存する。100/100の「完璧」判定には速度も要るが、まず「スコア加算」だけならタイミングは失敗要因にならない。
