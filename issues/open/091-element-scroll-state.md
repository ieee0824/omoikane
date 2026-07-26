---
number: 091
slug: element-scroll-state
github: 245
status: open
priority: high
---

# Element scrollTop・scrollLeft と scroll container state を実装する

## 概要

Element の固定 0 scroll offset を実装し、nested scroll container を DOM / layout / paint へ接続する。
#244（Window scroll state、PR #258）の機構をそのまま拡張する形で実装した。

## 背景

- `scrollWidth` / `scrollHeight` の算出基盤はあった
- `scrollTop` / `scrollLeft` getter は 0、setter は no-op
- overflow clip は存在するが、子孫 paint translation へ scroll offset が未接続
- `overflow: scroll` / `auto` は **clip すらされていなかった**（`hidden` のみ `Overflow::Hidden`）

## Firefox 152 実測（設計の根拠）

Marionette 経由で同一スクリプトを Firefox と omoikane で実行して比較した（約 80 検証点）。

### scroll container

| overflow | Firefox | 実装 |
|---|---|---|
| `hidden` / `scroll` / `auto` | プログラム的に scroll 可能 | `Overflow::Hidden`（clip + scroll container） |
| `visible` / `clip` | scroll 不可 | `Overflow::Visible` |

`hidden` は user scroll できないだけで、`scrollTop` では動く。

### clamp

max = `scrollWidth - clientWidth` / `scrollHeight - clientHeight`。負値は 0、NaN / Infinity は 0。
書き込み時に clamp して保存し、読み出し時にも現在の extent で clamp する（保存値は書き換えない）。

### 状態の寿命（すべて Firefox と完全一致）

| 操作 | 結果 |
|---|---|
| `removeChild` → 再挿入 | 0 にリセット |
| 別 parent へ移動 | 0 |
| 同一 parent 内で `insertBefore` による reorder | 0（detach を経るため） |
| `display: none` → 戻す | 一時的に 0、戻すと**値が復帰** |
| `overflow: visible` → 戻す | 一時的に 0、戻すと**値が復帰** |
| 無関係な style 変更 | 保持 |
| content が縮む | 新しい extent へ clamp |
| `cloneNode` | clone は 0 |
| `innerHTML` で子を差し替え | container 自身の offset は保持 |

「読み出し時に clamp するが保存値は commit しない」設計が、この復帰挙動を自然に満たす。

### 幾何

- 静的子孫は祖先 scroll container の offset を累積（実測値 base [0,20] → outer(15,30) → [-15,-10] → inner(20,40) → [-35,-50] が omoikane で完全一致）
- container 自身の rect、`offsetTop` / `offsetLeft` / `clientTop` は不変
- `position: fixed` は祖先 scroll の影響を受けない。fixed 配下の scroll container は自身の content を scroll する
- 標準モードで `documentElement.scrollTop` ⇔ `scrollY`（setter / メソッドも window を scroll）、`body.scrollTop` は 0

## 実装

| 層 | 内容 |
|---|---|
| `src/dom/mod.rs` | `Element.scroll_offset` と `NodeHandle::{scroll_offset, set_scroll_offset}`。`remove_child` が detach された subtree の offset を 0 に戻す。thread-local カウンタ `any_element_scrolled()` で「どこも scroll していない」経路をゼロコストにする |
| `src/layout/mod.rs` | `overflow_keyword_scrolls` が `scroll` / `auto` も scroll container にする。`LayoutBox::{scrollable_overflow, max_scroll_offset, scroll_offset, is_scroll_container}`（`compute_layout_metrics` の `expand_scroll_bounds` をここへ移動して共有） |
| `src/paint/mod.rs` | `apply_scroll_offsets` が累積 offset を持ち回る 1 パスで tree を移動する。box 自身は祖先分だけ、自身の line box / marker / 子は祖先 + 自身の offset だけ動く。fixed に入ったら累積をリセット |
| `src/js/mod.rs` | `find_layout_box_with_scroll` が transform と累積 scroll を同時に返し、#244 の DOM 祖先を辿る `is_fixed` 判定を置き換える。`HostState::{element_scroll_offset, set_element_scroll}` と native binding 2 本 |
| `src/js/dom_bootstrap.js` | `scrollTop` / `scrollLeft` の getter / setter、`Element.scroll()` / `scrollTo()` / `scrollBy()`、root element の window 委譲、`scroll` イベント dispatch、`onscroll` |

state を HostState ではなく DOM Element に置いたのは、(a) `remove_child` が全削除経路の choke point なので detach reset が正確に書ける、(b) paint がノードから直接読めるのでマップを引き回さなくて済む、(c) JS runtime なしの paint テストから設定できる、の 3 点から。

## テスト

- `src/dom/tests`: 4 本（round trip、detach、move / reorder、カウンタ）
- `src/layout/tests`: 4 本（scroll container 判定、layout geometry 不変、clamp、nested での走査停止）
- `src/paint/tests`: 11 本（translate + clip、横 scroll、paint 側 clamp、`auto` / `scroll` が clip すること、`visible` / `clip` が scroll しないこと、fixed 子孫、fixed 配下の container、nested 累積、window との合成、box と content の分離、script → paint の end-to-end）
- `src/js/tests`: 11 本（clamp、container 種別、メソッド overload、イベント、nested rect、fixed、detach、box 復帰、content 縮小、box なしでの setter、`documentElement`）
- `tests/web_api_surface/manifest.json`: 3 features を `baseline_supported: true` で追加

## 対象外

smooth behavior、scroll snap、sticky、GUI scrollbar、wheel 入力。

follow-up issue に切り出した差分:

| 差分 | issue |
|---|---|
| scroll event の非同期化（rendering opportunity での flush、frame 合体、viewport scroll の target、clamp 由来の発火） | #259 |
| `overflow: clip` が clip されない | #260 |
| `scrollWidth` / `scrollHeight` の end-edge padding（Firefox 314 / omoikane 307） | #261 |
| 包含ブロックが container の外にある絶対配置子孫の clip / scroll escape | #262 |

issue にしていない既知の差分:

- scrollbar gutter — `overflow: auto` / `scroll` で Firefox は 12px を確保するので `clientWidth` / `clientHeight` と clamp 上限が異なる。GUI scrollbar が対象外なので現時点では差分のまま
- 小数の丸め — Firefox は `scrollTop = 10.5` を 10 にするが、#244 の window scroll と揃えて小数を保持する
- `document.scrollingElement` / `Element.scrollIntoView()` 未実装
- quirks モードの `body.scrollTop` 特例（quirks モード自体が未実装）
- iframe sub-document は layout を保持していないため、その中の要素は `scrollTop` が 0

## 完了条件

既存 layout / paint test 非退行、通常の全 suite 成功。

GitHub Issue: #245（親 #173、前提 #244）
