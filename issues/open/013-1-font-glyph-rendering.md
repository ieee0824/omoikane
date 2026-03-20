---
number: 013-1
slug: font-glyph-rendering
parent: 013-real-world-rendering
status: open
---

# フォントグリフレンダリング

## 概要

テキストが固定幅の黒四角として描画されている。フォントファイルを読み込み、
各文字のグリフ（輪郭）をラスタライズして描画する。

## スコープ

- TrueType/OpenType フォントファイル（.ttf/.otf）の読み込み
- cmap テーブルから文字→グリフID のマッピング
- glyf テーブルからグリフのアウトライン取得
- アウトラインのラスタライズ（ベジェ曲線→ビットマップ）
- paint 時にグリフビットマップを描画
- システムフォントのフォールバック検索（sans-serif/serif/monospace）
- font-size に応じたスケーリング

## スコープ外（初期）

- フォントの合成（bold/italic の自動生成）
- カーニング・リガチャ
- CJK フォントの vertical layout
- Web フォント（@font-face）
- サブピクセルレンダリング

## 技術方針

- pure Rust クレートの利用を検討（ttf-parser, ab_glyph 等）
- コア部分は自前実装の方針だが、フォントパーサーはインフラ層として外部クレート許可の範囲
