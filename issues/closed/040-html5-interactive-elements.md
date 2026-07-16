---
number: 040
slug: html5-interactive-elements
status: closed
---

# HTML5 インタラクティブ要素対応

## 概要

`<details>`, `<summary>`, `<dialog>` 等のインタラクティブ要素の基本レンダリングを実装する。

## 対象要素

| 要素 | 最小実装 |
|------|---------|
| `<details>` | open 属性がない場合は summary のみ表示、open 時は全コンテンツ表示 |
| `<summary>` | display: list-item（disclosure triangle） |
| `<dialog>` | open 属性がない場合は display: none |
| `<time>` | inline 要素として扱う（テキスト表示のみ） |
| `<progress>` | プレースホルダー矩形 |
| `<meter>` | プレースホルダー矩形 |

## 優先度

中

## 対応内容

- `details`/`summary` の閉開状態と list-item UA 表示を実装
- `dialog` の open 属性による表示切り替えと `time` の inline 表示を実装
- `progress`/`meter` を既存フォームコントロール描画によるプレースホルダー矩形として実装
- 対象タグを対応済みHTMLタグ一覧へ追加し、スタイル・レイアウト回帰テストを追加
