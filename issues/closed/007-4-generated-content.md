---
number: 007-4
slug: generated-content
status: closed
parent: 007
---

# 生成コンテンツ・data URI

CSS生成コンテンツ（::before / ::after）とdata: URIスキームを実装する。
Acid2では生成コンテンツによるテキスト挿入とdata: URIによる画像参照が使われている。

## タスク
- [x] ::before / ::after 擬似要素のボックス生成
- [x] content プロパティの評価（文字列リテラル、url()）
- [x] data: URI のパース（MIME type, base64/plaintext）
- [x] data: URI からの画像デコード（PNG）
- [x] content: "" （空文字）によるレイアウト用ボックス生成
