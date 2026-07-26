---
number: 090
slug: document-focus-state
github: 243
status: closed
priority: high
---

# Document focus state と focus event core を実装する

## 概要

`Element.focus()` / `blur()` の stub を解消し、GUI 入力接続の前提となる Document 単位の
focus 状態と event 遷移を実装する。

## 背景

- `src/js/dom_bootstrap.js` の `focus()` / `blur()` は完全な no-op
- `document.activeElement` が存在しない
- `Document.hasFocus()` は常に `true` を返す固定値
- `FocusEvent` と event dispatch 基盤（capture/bubble/retarget/relatedTarget）は既存

## 仕様確認

### UI Events: focus event order

A から B へ focus が移る場合の順序:

| # | event | target | bubbles | relatedTarget |
|---|---|---|---|---|
| 1 | `blur` | A | false | B |
| 2 | `focusout` | A | true | B |
| 3 | `focus` | B | false | A |
| 4 | `focusin` | B | true | A |

4 つとも `cancelable: false` / `composed: true`。
仕様の注記「blur は focus が外れた**後**に dispatch する」に従い、`blur` / `focusout` の
dispatch 中は `activeElement` が body fallback になる（ブラウザ実挙動と一致）。

### HTML spec: focus fixup rule

focus 中の要素が DOM から外れた場合、その Document の viewport（`activeElement` は body）へ戻る。
同期版の fixup では blur / focusout を発火しない。Firefox はこれに従い、Chromium は
"update the rendering" 経由で blur を発火する差異がある。本実装は同期版に合わせる。

## 実装範囲

- Document ごとの active element。初期値と blur 後の fallback は body、なければ documentElement
- `HTMLElement.focus()` / `blur()`
- `focus` / `blur` / `focusin` / `focusout` の順序、bubbles、relatedTarget
- 同じ要素への再 focus は no-op
- disconnected・disabled form control を focus 対象外にする
- node removal 時に activeElement を安全に fallback させる（focus fixup rule）
- `Document.hasFocus()` を実状態へ接続

## 設計

すべて `src/js/dom_bootstrap.js` で完結する（native binding の追加は不要）。

focused element は Document の wrapper 自身に `__focusedElementId` として持たせる。
wrapper は node id ごとにキャッシュされて同一性が保たれるので、iframe の sub-document は
自然に自分の active element を持ち、別 Map を持つより document の寿命と state の寿命が揃う。

```js
let focusedDocumentId = null;  // null は top-level document を意味する
```

`focusedElementOf(doc)` が focus fixup rule を遅延適用する。`activeElement` getter と
`focus()` / `blur()` の両方がこれを通ることで、DOM から外れた要素へ blur が飛ぶことを防ぐ。

`hasFocus()` は focused document とその祖先 document で `true`。iframe の owner 要素を
`__omoikane_document_owner_iframe` で辿り、top へ到達できない壊れた chain
（iframe reload / 削除後）は top-level document へ復帰させる。

## focusability

issue の記述は「disconnected・disabled form control を focus 対象外にする」だけだが、
Firefox 152 で実測すると tabindex を持たない `<div>` は focus できない。
DOM だけで判定できる範囲を Firefox に合わせた:

focusable = 接続済み && disabled control でない &&
（整数 tabindex を持つ ‖ `a[href]` ‖ `input`(type!=hidden) / `select` / `textarea` / `button`
 ‖ `iframe` / `embed` ‖ `audio[controls]` / `video[controls]` ‖ `details` 直下の `summary`
 ‖ contenteditable の editing host 自身）

`object` / `area` / `details` / `dialog` / `img` / `label` / `option` / `fieldset` は focus 不可。
tabindex は HTML の整数パース規則に従うため `"1.5"` や `"+2"` は有効、`"abc"` は無効。
body は tabindex を付けたときだけ focusable（fallback として返るのとは別）。

## Firefox 152 との実測比較

Marionette 経由の Firefox と omoikane で**同一スクリプト**を実行して比較した
（60 以上の検証点）。イベント順序・bubbles・composed・cancelable・relatedTarget・
blur 中の activeElement・removal 時の無イベント・再 focus・disabled/disconnected・
focusability 39 ケース・detached document の `hasFocus()` はすべて完全一致。

差分は 6 点・根本原因 4 件で、すべて follow-up issue に切り出した:

| 差分 | Firefox | omoikane | issue |
|---|---|---|---|
| iframe 内 focus 後の親 `activeElement` | iframe 要素 | 親自身の focus 対象 | #254 |
| `display:none` / `visibility:hidden` の focus | 不可 | 可 | #255 |
| `documentElement.focus()` | html を focus | 変化なし | #255 |
| `fieldset[disabled]` 子孫の focus と `:disabled` | 不可 / true | 可 / false | #256 |

## 対象外

Tab 順序、platform keyboard 入力、IME、selection、`:focus-visible`、
完全な Shadow DOM focus delegation。

加えて、iframe 内の要素を focus したときに親 document の activeElement が
その iframe 要素になる focus chain の伝播も対象外とする（#254）。state だけ真似ると
chain 各エントリへの event 発火と不整合になるため。
各 document の activeElement は独立し、`hasFocus()` は chain を辿るのでこの範囲でも整合する。

## テスト

`src/js/mod.rs` の unit test:

- `focus_tracks_active_element_and_blur_falls_back_to_body`
- `focus_dispatches_blur_focusout_focus_focusin_in_order`
- `focus_events_carry_related_target_both_directions`
- `refocusing_the_same_element_dispatches_no_events`
- `blur_on_a_non_focused_element_is_a_no_op`
- `focus_ignores_disconnected_nodes_and_disabled_controls`
- `removing_the_focused_element_falls_back_to_body_without_events`
- `active_element_falls_back_to_document_element_without_body`
- `active_element_and_has_focus_are_isolated_per_iframe_document`
- `focus_only_applies_to_focusable_areas` — Firefox 152 実測に基づく focusability 32 ケース
- `body_is_only_focusable_with_a_tabindex`
- `focus_events_are_composed_and_not_cancelable`
- `moving_the_focused_element_to_another_document_clears_the_active_element`
- `detached_document_never_reports_focus`

`tests/web_api_surface/manifest.json` に regression guard として 5 features を追加する
（`dom.active-element` / `dom.focus-event-order` / `dom.focus-related-target` /
`dom.focus-fixup-rule` / `dom.document-has-focus`、すべて `baseline_supported: true`）。

## 完了条件

通常の全テスト・WPT smoke・doc test 成功。既存 Web API surface regression 0。

GitHub Issue: #243（親 #173）
