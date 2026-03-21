---
number: 015-3
slug: ffi-screenshot-viewport-control
parent: 015-anonymized-real-world-rendering-gap
status: closed
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

## 実施結果

- `omoikane_screenshot_png_with_viewport(browser, width, height)` を追加
- 既存 `omoikane_screenshot_png` は互換維持しつつ、内部でデフォルト viewport 指定の新経路を利用
- width/height が 0 以下の場合は `omoikane_last_error` に理由を設定して `null` を返すようにした
- `cbindgen.toml` / `include/omoikane.h` を更新して新APIを公開

## 検証

- `cargo test ffi::` 通過
- 新規テスト:
  - 明示 viewport (`1024x512`) で PNG IHDR 寸法が一致
  - 不正 viewport (`0x720`) でエラーを返す
