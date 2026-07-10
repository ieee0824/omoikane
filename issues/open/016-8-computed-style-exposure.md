---
number: 016-8
slug: computed-style-exposure
parent: 016
status: open
---

# getComputedStyle の実値化（カスケード結果のJS露出）

## 目的

CSS カスケードの計算結果を JS の `getComputedStyle` に接続し、実値を返せるようにする。

## 背景（GAP_ANALYSIS.md セクション1 P5/P6、セクション3 領域D）

- 現状 `getComputedStyle` は常に `""` を返すスタブで、カスケード結果が JS に露出していない。
- `src/css/matcher.rs` / カスケード計算は存在するが JS 側に繋がっていない。
- Acid3 の `selectorTest` は `getComputedStyle(node,'').zIndex` で全セレクタ判定を行い、
  test0 は `whiteSpace === 'pre-wrap'` を確認するため、bucket3 全体と test 0/47 の必須前提。

## 相互参照

- **044-2（`issues/open/044-2-layout-metrics-bindings.md`）と同根**。
  044-2 は `getComputedStyle()` 実値返却 + レイアウトメトリクス API を扱う。
  本issueと 044-2 は同じ「カスケード/レイアウト結果の JS 露出」基盤であり、
  実装は一体で進めること（重複実装を避ける）。

## スコープ

- カスケード計算済みプロパティを `getComputedStyle` の戻り値オブジェクトに露出
- `document.defaultView.getComputedStyle` と global 両対応

## 受け入れ条件

- `getComputedStyle(el,'').whiteSpace` 等がカスケード結果の実値を返す
- Acid3 test 0 の `pre-wrap` 判定と selectorTest の zIndex 判定が機能する

## 進捗 (2026-07-10)

**044-2 と一体で実装（ブランチ: `issue/016-8-computed-style-exposure`、main から分岐）。**

### 実装内容
- ネイティブ primitive `__omoikane_computed_style(nodeId)` を追加（`src/js/mod.rs`）。
  カスケード計算済みプロパティを kebab-case キーの JSON マップで JS に返す。
  - `HostState` に `StyleResolver` / レイアウトツリー / viewport / dirty フラグを保持。
  - `getComputedStyle` 呼び出し時にインライン `<style>` を収集して `StyleResolver` を構築し、
    `computed_style()` の結果を実値化して返す（外部CSSは同期性維持のため取得しない）。
- `dom_bootstrap.js` の空 Proxy スタブを実装に置換。
  - camelCase アクセス（`style.whiteSpace`）と `getPropertyValue('white-space')` の両対応。
  - `cssFloat`/`styleFloat` → `float` エイリアス。
  - インライン `style` 属性の宣言をカスケード結果に上書きマージ（インライン優先）。
  - `getComputedStyle` / `window.getComputedStyle` / `document.defaultView.getComputedStyle` は
    全て同一関数（`defaultView` は `globalThis` を返すため自動的に一致）。
- カスケードで列挙型プロパティの不正キーワードを破棄するよう修正（`src/css/style.rs`）。
  `white-space: pre-wrap; white-space: x-bogus;` が `pre-wrap` を維持する（test 0 前提）。
- DOM 変更（append/insert/remove child, set/remove attribute, textContent, innerHTML）で
  dirty フラグを立て、次回クエリで強制再レイアウト（forced reflow）。

### スコア変化
- Acid3: **43/100 → 45/100**（`cargo run --example acid3` 実測）。
  - **test 0**（`getComputedStyle(penultimate,'').whiteSpace === 'pre-wrap'` + `:last-child` 再計算 + `defaultView`）が PASS。
  - **test 45**（`document.body.style.cssFloat`）が PASS。

### 残課題
- selectorTest（bucket3: 33〜44, 46, 47）は `getTestDocument()` = iframe/contentDocument（P7, Track E）
  が未実装のため依然 fail。`getComputedStyle(node,'').zIndex` 自体は実値（`0`/`N`）を返す状態になっており、
  Track E 統合後に解放される見込み（受け入れ条件どおり）。
- 外部リンク CSS は `getComputedStyle` のカスケードに未反映（インライン `<style>` のみ収集）。
