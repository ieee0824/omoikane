---
number: 007-4
slug: generated-content
status: open
parent: 007
---

# 生成コンテンツ・data URI

CSS生成コンテンツ（::before / ::after）とdata: URIスキームを実装する。
Acid2では生成コンテンツによるテキスト挿入とdata: URIによる画像参照が使われている。

## タスク
- [ ] ::before / ::after 擬似要素のボックス生成
- [ ] content プロパティの評価（文字列リテラル、url()）
- [ ] data: URI のパース（MIME type, base64/plaintext）
- [ ] data: URI からの画像デコード（PNG）
- [ ] content: "" （空文字）によるレイアウト用ボックス生成
