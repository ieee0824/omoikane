---
number: 044-2
slug: layout-metrics-bindings
status: open
parent: 044
---

# レイアウトメトリクス（Rust連携）

## 概要

JS からレイアウト結果を取得する API を Rust ネイティブバインディングとして実装する。

## 追加 API

| API | 用途 |
|-----|------|
| `getBoundingClientRect()` | 要素の位置・サイズ取得 |
| `offsetWidth` / `offsetHeight` | レイアウト幅・高さ |
| `offsetTop` / `offsetLeft` | 親からのオフセット |
| `clientWidth` / `clientHeight` | padding 含むサイズ |
| `scrollWidth` / `scrollHeight` / `scrollTop` / `scrollLeft` | スクロール情報 |
| `getComputedStyle()` 実値返却 | 計算済みスタイル取得 |

## 技術的課題

- JS 実行時点ではレイアウトが未完了の可能性がある
- 同期的にレイアウト結果を返す必要がある（forced reflow）
- HostState にレイアウトツリーへの参照を持たせる設計が必要

## 進捗 (2026-07-10)

**016-8 と一体で実装（ブランチ: `issue/016-8-computed-style-exposure`、main から分岐）。**

### 実装内容
- ネイティブ primitive `__omoikane_layout_metrics(nodeId)` を追加（`src/js/mod.rs`）。
  対象要素の `LayoutBox` を探索し、以下を JSON で返す:
  - `getBoundingClientRect` の8値（x/y/width/height/top/left/right/bottom）= border box。
  - `offsetWidth`/`offsetHeight` = border box、`offsetTop`/`offsetLeft` = border box の
    初期包含ブロック（viewport 原点）相対位置。
  - `clientWidth`/`clientHeight` = padding box、`clientTop`/`clientLeft` = border 幅。
  - `scrollWidth`/`scrollHeight` = padding box をはみ出す子孫 border box まで拡張。
  - `scrollTop`/`scrollLeft` = 0（スクロールオフセット未モデル化）。
- `HostState` に `StyleResolver` / レイアウトツリー / viewport / dirty フラグを保持し、
  DOM が dirty のとき次回クエリで `layout_tree()` を同期実行（forced reflow）。
- `dom_bootstrap.js` の 0 固定スタブ（`offsetWidth` 等 / `getBoundingClientRect`）を
  ネイティブ呼び出しに置換。`JsRuntime::set_viewport()` を追加し `render_document` から配線。
- `getComputedStyle` 実値返却は 016-8 側で実装（同根のため一体）。

### テスト（具体的な期待値つき）
`src/js/mod.rs` の tests に追加:
- `get_bounding_client_rect_returns_block_geometry`: `width:100px; height:50px` → rect (0,0,100,50)。
- `client_metrics_account_for_padding_and_border`: padding 10px + border 5px →
  clientWidth=120, offsetWidth=130, clientTop=5 等。
- `layout_metrics_force_reflow_after_class_change`: class 追加後の幅変化を再レイアウトで反映（100→250）。
- `scroll_size_encloses_overflowing_child`: clientHeight=50 だが scrollHeight=200。

### スコア変化
- Acid3: 43/100 → **45/100**（016-8 と合わせて test 0 / test 45 が PASS）。

### 残課題
- `offsetParent` は常に `null`（DOM 上）。offset* は初期包含ブロック相対で近似。
- スクロールオフセット（`scrollTop`/`scrollLeft` の実値）は未モデル化。
