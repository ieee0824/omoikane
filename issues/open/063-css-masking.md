---
number: 063
slug: css-masking
status: open
---

# CSS マスキング（mask / -webkit-mask / mask-image）対応

## 概要

`mask` 系プロパティが未対応のため、SVG マスクで形状を切り抜く装飾が
矩形のまま全面描画される。kasaneteto.jp 実測では `mask` 10 箇所・
`-webkit-mask` 10 箇所・`mask-image` 7 箇所が未対応ログに出ており、
ヒーローの赤い装飾（`.p-home-kv__visual-bg`: `inset: 0; background: var(--c-red);
-webkit-mask: url("../images/index/kv/mask_kv_character_sp.svg") ...`）が
巨大な赤矩形として描画される。

## スコープ（段階実装）

1. `mask` / `-webkit-mask` / `mask-image` / `-webkit-mask-image` を
   is_supported_property に登録し、ショートハンドを mask-image / mask-position /
   mask-size / mask-repeat に展開（-webkit- は標準へマッピング）
2. `mask-image: url(...)` の画像（SVG 含む）を取得・ラスタライズし、
   アルファ（luminance ではなく alpha 既定）を要素描画に乗算
3. mask-size / mask-position / mask-repeat は background-* と同じ解決ロジックを流用
4. 取得失敗・未対応形状は「マスク無し」ではなく**要素を描画しない**方向も検討
   （マスク前提の装飾は素の矩形より非表示の方が実サイトの見た目に近い。
   仕様的には invalid reference = マスク無しなので、どちらに寄せるかは実装時に判断）

## 受け入れ条件

- mask-image の alpha 乗算の回帰テスト（ピクセル or レイアウト単位の具体値アサート）
- kasaneteto.jp ヒーローの赤矩形が装飾形状（または非表示）になる
- 既存テスト・Acid3 スコア（97/100）の維持

## 関連

- 062 clip-path: inset()（同サイトの全面赤ブロック）
- SVG ラスタライズ基盤（既存の画像デコード経路の確認が必要）
