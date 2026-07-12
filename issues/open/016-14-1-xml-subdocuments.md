---
number: 016-14-1
slug: xml-subdocuments
parent: 016-14
status: open
---

# XML/XHTML サブ文書のパースと DOM 化

## 目的

iframe/object のサブ文書として XML/XHTML コンテンツ(Content-Type: text/xml, application/xml,
image/svg+xml, application/xhtml+xml)を実際に XML としてパースし DOM ツリー化する。
現状は HTML Content-Type 以外は空スケルトンになるため、Acid3 bucket5 の大半が塞がっている。

## 対象テスト(Acid3)

- test 69: svg.xml を DOM として取得(`contentDocument.getElementsByTagName('text')[0]` の除去)
- test 70: XML エンコーディング検査(ISO-8859-1 宣言の文書を UTF-8 で送信 → 整形式性エラー扱い)
- test 71: xhtml.1〜3 の children 検査(XHTML 名前空間、`<script>` 実行を含む)
- test 80: doctype IDL(XML 文書の doctype 検査)
- test 98: XHTML 文書の title 挙動(016-12 からの引き継ぎ)
- test 64 の残存原因もこの領域か実測で確認

## スコープ

- XML パーサ(整形式性検査: タグ対応・属性引用・実体参照の最小セット。違反時は文書を
  エラー扱い = 空 or パースエラー文書)
- Content-Type 判定によるサブ文書ロード経路の分岐(HTML / XML)
- XML 文書の DOM: 大文字小文字保持、名前空間(createElementNS 済み基盤の再利用)、
  doctype ノード
- XHTML 文書内 `<script>` の実行(test 71 が要求する範囲)
- fixture: tests/fixtures/acid3/ の svg.xml / empty.xml / xhtml.1-3 が実際に配信される
  Content-Type を manifest.json で確認して合わせる

## 受け入れ条件

- 上記対象テストのうち XML パース起因のものが前進する(実測で個別記録)
- 既存テスト・Acid3 スコア(90/100)の維持以上

## 実装結果（2026-07-12）

- `src/xml` に strict な最小 XML parser を追加。タグ対応、属性引用、定義済み実体／数値文字参照、CDATA、comment、PI、doctype、UTF-8 と encoding 宣言を検証し、違反時は部分 DOM を破棄する。
- `text/xml` / `application/xml` / `image/svg+xml` / `application/xhtml+xml` を XML としてロード。qualified name の大小文字、default/prefixed namespace、XML 属性名、doctype IDL を DOM/JS に公開した。
- 正しい XHTML namespace の整形式文書だけ parser-inserted classic script を実行。誤 namespace の `xhtml.3` と不整形式な `xhtml.2` は実行しない。
- XHTML の動的 `Document.title` と namespaced form の `Document.forms` を補完した。
- XML parser 単体 5 tests と iframe 統合 3 tests を追加。`cargo build -j1`、`cargo test --lib -j1`（1036 passed / 26 ignored）、`cargo test --tests -j1`（同 unit 群 + Acid3 harness 4 passed）を完走。

### Acid3 実測

| test | main | 実装後 | 結果 |
| --- | --- | --- | --- |
| 64 | FAIL（object.data が `test.html`） | FAIL（同一） | URL IDL 反射で別領域 |
| 69 | FAIL（`t was null`） | PASS | SVG XML DOM 化で解消 |
| 70 | PASS | PASS | 不正 UTF-8 を XML parser の fatal error として固定 |
| 71 | FAIL（Document childNodes 3、期待 2） | FAIL（同一） | `document.write` 後の HTML tree construction で別領域 |
| 80 | FAIL（linktest onload timeout） | FAIL（同一） | test 48 の接続済み iframe `src` 再 navigation / 動的 onload が未配線。XHTML script 自体は統合 test で検証済み |
| 98 | FAIL（title 更新後も空） | PASS | XHTML title / forms を解消 |

FAITHFUL / DIRECT とも **90/100 → 92/100**、index 100、script/drive error 0。FAITHFUL は非ハング。
