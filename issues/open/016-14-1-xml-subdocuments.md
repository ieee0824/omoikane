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
