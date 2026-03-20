---
number: 015-3
slug: ffi-screenshot-viewport-control
parent: 015-anonymized-real-world-rendering-gap
status: open
---

# screenshot API の viewport 指定対応

## 概要

現在の `omoikane_screenshot_png` は固定サイズ描画のため、
ページ依存で見え方が崩れる。FFIからviewportを指定できるAPIを追加する。

## スコープ

- C APIに viewport 指定付き screenshot 関数を追加
- 既存API互換の維持（既存関数は残す）
- 不正なサイズ入力時のエラーハンドリング
- CヘッダとFFIテスト更新

## 受け入れ条件

- 呼び出し側から幅/高さを指定してPNG取得できる
- 既存 `omoikane_screenshot_png` 利用コードが壊れない
- FFIテストでPNG寸法が指定値になることを検証できる
