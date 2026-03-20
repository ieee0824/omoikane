---
number: 007-1
slug: paint-system
status: closed
parent: 007
---

# ペイントシステム

レイアウトツリーを走査し、ピクセルバッファ（ビットマップ）に描画する仕組みを構築する。

## タスク
- [x] ピクセルバッファ（RGBA）の構造体と基本描画プリミティブ（矩形塗りつぶし）
- [x] 背景色の描画（background-color）
- [x] ボーダーの描画（border-width, border-color, border-style: solid）
- [x] テキスト描画（簡易フォントラスタライザ）
- [x] PNG画像の読み込みと描画（alpha合成）
- [x] z-index / スタッキングコンテキストに基づく描画順序
- [x] overflow: hidden によるクリッピング
- [x] visibility: hidden の処理
- [x] PNG出力
