---
number: 065
slug: document-write-full-document-parsing
parent: 016
status: open
---

# document.write の完全文書パースと doctype IDL（Acid3 test 71）

## 概要

空文書への `doc.open(); doc.write(完全なHTML文書); doc.close()` で、doctype ノードと html/head/body の暗黙構造を仕様どおり構築する。あわせて DOCTYPE の publicId/systemId をトークナイザで解析する。

## 失敗内容

```
Test 71 failed: expected '2' but got '3' - wrong number of children in #document (first test)
```

iframe contentDocument への
`doc.write('<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.0 Transitional//EN"><title></title><span></span><script type="text/javascript"></script>')`
の後、期待は `doc.childNodes = [doctype, html]`（2個）、`html = [head, body]`、`head = [title]`、`body = [span, script]`。
さらに `doctype.name`（"HTML"）、`publicId`、`systemId`（無指定時は null か ""）、`internalSubset`（null）の IDL 検証と、
2回目の write で `PUBLIC "..." "..."` 形式の systemId 取得、`<span><script></script></span>` のネスト構造検証がある。

## 原因分析（調査済み）

1. **write が常にフラグメント扱い**: `document_write_native`（`src/js/mod.rs:3209-3348`）は書き込みテキストを常に `<body>{text}</body>` で包んでパースし（`src/js/mod.rs:3228`）、body の子だけ取り出して挿入する。このため
   - DOCTYPE トークンは "in body" モードで破棄される（`src/html/tree_builder.rs:233`）
   - `<title>` は head に振り分けられず body 直下の汎用要素になる
   - 空の iframe 文書では fallback parent が document 自身になり（`src/js/mod.rs:3249-3253, 3273-3280`）、`[title, span, script]` の3ノードが document 直下に落ちる → これが「3」の正体
2. **トークナイザが publicId/systemId を解析しない**: `DoctypeToken` は name + force_quirks のみ（`src/html/tokenizer.rs:35-52`）。doctype 用の状態は 3 つだけで（`src/html/tokenizer.rs:114-116`）、AfterDoctypeName 以降の状態群（PUBLIC/SYSTEM キーワード、引用符付き識別子）が全て欠落。現状 `<!DOCTYPE HTML PUBLIC "...">` は name が壊れた文字列になる
3. **tree builder が doctype に空の public/system を渡す**（`src/html/tree_builder.rs:107-112`）
4. **DOM 層と JS IDL は対応済み・修正不要**: `DocumentType`（`src/dom/mod.rs:175-210`）、`publicId`/`systemId` getter（`src/js/dom_bootstrap.js:607-613`、native は None 時 `""` を返し test の `!= null && != ""` 判定を通過）、`internalSubset` は常に null（`src/js/dom_bootstrap.js:615-617`）
5. **"after head" モードは不要**: `in head` の other 分岐が pop head → InBody → ensure_body で代替しており、test 71 のケースは既存モードで正しく振り分けられる（トレース確認済み）

## 実装方針

1. **write の分岐**（`src/js/mod.rs` `document_write_native`）: 対象文書が documentElement を持たない（`doc.open()` 直後で空）場合は `<body>` ラップをやめ、`TreeBuilder::parse(&text)` で**完全文書としてパース**し、`parsed.document().child_nodes()`（= [doctype, html]）を対象 document に append する。documentElement が既にある場合（mid-parse write 等）は**現行のフラグメント挿入を維持**。inline classic script の収集・返却（`src/js/mod.rs:3335-3346`）は full-parse 分岐でも同様に行う
2. **トークナイザに DOCTYPE public/system 解析を追加**（`src/html/tokenizer.rs`）: `DoctypeToken` に `public_id: Option<String>` / `system_id: Option<String>` を追加。HTML 仕様 §13.2.5.53–13.2.5.68 相当の状態群（AfterDoctypeName / PublicKeyword / 引用符付き識別子 / Between / SystemKeyword / BogusDoctype）を追加。**識別子は小文字化しない**。EOF ハンドリングにも新状態を追加
3. **tree builder の doctype 生成**（`src/html/tree_builder.rs:107-112`）: `NodeHandle::document_type(name, public_id, system_id)` に実値を渡す

## テスト計画

- **tokenizer**（`src/html/tokenizer.rs`、既存 `tokenizes_doctype` :1408 の近く）: PUBLIC のみ / PUBLIC+SYSTEM / systemId 無しで `system_id == None`
- **tree_builder**（`src/html/tree_builder.rs` :917 以降）: doctype 付き完全文書の full parse で document 子が `[doctype, html]`、doctype の name/public_id/system_id、head=[title]/body=[span,script]
- **document.write**（`src/js/mod.rs` テスト群）: iframe contentDocument への open/write/close で test 71 の first/second テストを単体再現（childNodes.length==2、firstChild.name/publicId/systemId、head/body 構造）

## 回帰リスク

- **`document_open_write_close_leaves_only_written_content`（src/js/mod.rs:9827-9859）は期待値の更新が必要**: 現在 document 直下が `["div"]` であることをアサートしているが、full-parse 化で `["html"]`（div は body 内）になる。実ブラウザ挙動は後者なので、このテストの期待値を暗黙 html/head/body 構造の検証に書き換える（テストの弱化ではなく仕様準拠への修正であることをコミットメッセージに明記）
- `iframe_sub_document_open_write_close_replaces_content`（:9468）は getElementById 等のみ検証で回帰しない
- メイン文書の mid-parse write テスト群（:7847-7974）は documentElement 存在下 → フラグメント経路維持で回帰なし。**分岐条件を「documentElement 無し」に限定することが必須**
- Acid3 test 72 の前提（test 71 の finally が write する style/img 文書）: full-parse で style は head、img は body 内になるが、test 72 は `doc.styleSheets[0]` / `doc.images[0]` のみ参照するため維持される見込み。**head 内 `<style>` の RAWTEXT 処理と stylesheet 収集が機能することを要確認**
- 054 の table tree construction には触れない（影響なし）
- issue 046（document.write 仕様残差）: 本修正で「空文書時は完全文書パース」の分岐が加わる。046 の項目 4（タグ跨ぎ分割 write）に新挙動が絡むため、046 に本分岐の存在を追記する

## 受け入れ条件

- Acid3 test 71 が FAITHFUL/DIRECT 両モードで PASS、test 72 が PASS を維持
- 上記単体テストの追加と既存テスト全通過（期待値更新は `document_open_write_close_leaves_only_written_content` のみ）
