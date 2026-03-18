---
number: 007-1
slug: paint-system
status: open
parent: 007
---

# ペイントシステム

レイアウトツリーを走査し、ピクセルバッファ（ビットマップ）に描画する仕組みを構築する。

## タスク
- [ ] ピクセルバッファ（RGBA）の構造体と基本描画プリミティブ（矩形塗りつぶし、ライン）
- [ ] 背景色の描画（background-color）
- [ ] ボーダーの描画（border-width, border-color, border-style: solid）
- [ ] テキスト描画（簡易フォントラスタライザ or 組み込みビットマップフォント）
- [ ] PNG画像の読み込みと描画（alpha合成）
- [ ] z-index / スタッキングコンテキストに基づく描画順序
- [ ] overflow: hidden によるクリッピング
- [ ] visibility: hidden の処理
- [ ] PNG出力（スクリーンショット機能）
