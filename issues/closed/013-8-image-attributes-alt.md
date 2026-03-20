---
number: 013-8
slug: image-attributes-alt
parent: 013-real-world-rendering
status: closed
---

# 画像の相対URL・サイズ属性・alt表示

## 概要

外部画像の相対URL解決、HTML属性によるサイズ指定、フェッチ失敗時のalt表示を実装する。

## スコープ

### 相対URL解決
- `<img src="images/logo.png">` の相対URLをbase URLから解決
- base要素対応後はそちらのbase URLを使用

### サイズ属性
- `<img width="100" height="50">` のHTML属性を intrinsic size として使用
- CSS width/height が指定されていない場合に適用
- アスペクト比の維持（片方のみ指定時）

### altテキスト表示
- 画像フェッチ失敗時に `alt` 属性のテキストを表示
- altテキストはテキストフラグメントとしてレイアウト
- 破損画像アイコン（オプション）

## 技術方針

- element_inline_image にbase URL引数を追加
- 画像ノードから width/height 属性を取得
- フェッチ失敗時は InlineFragmentContent::Text にフォールバック
