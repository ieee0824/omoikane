---
number: 056
slug: window-global-class
status: open
---

# Window グローバルクラスの定義（instanceof Window 対応）

## 概要

`globalThis.Window` クラスを定義し、`window instanceof Window` が true になるようにする。
iframe の contentWindow facade も instanceof Window で判定できるようにする。

## 背景（実サイトログ由来: kasaneteto.jp）

053（element.style の removeProperty 等）解消後、次の JS エラーとして顕在化:

```
ReferenceError: Window is not defined
    at E (lenis.min.js:1:1038)        ← Lenis の Dimensions クラス
    at lenisStart (app.bundle.js:5434)
```

Lenis（スムーススクロールライブラリ）は `this.wrapper instanceof Window` を3箇所で使い、
wrapper が window かどうかでリサイズ監視・寸法取得の分岐をする。`Window` グローバルが
未定義のため参照時に throw し、Lenis の初期化が中断 → ページのスクロール制御・
連動アニメーションが動かない。

## スコープ

- `globalThis.Window` クラスを dom_bootstrap.js で定義・公開する
- `globalThis instanceof Window === true` を成立させる
  （`Symbol.hasInstance` カスタマイズ、または globalThis のプロトタイプ接続。
  Boa での globalThis のプロトタイプ操作可否を確認して安全な方式を選ぶ）
- iframe の contentWindow facade も `instanceof Window` で true になること
- `document.defaultView instanceof Window === true`（メイン文書）
- 要素や document など Window でないオブジェクトは false
- あわせて `window` 自己参照（`globalThis.window === globalThis`）が未定義なら定義

## 受け入れ条件

- `window instanceof Window` / `document.defaultView instanceof Window` が true
- `document instanceof Window` / `div instanceof Window` が false
- kasaneteto.jp のレンダリングで `ReferenceError: Window is not defined` が解消し、
  Lenis の初期化が前進する（次のエラーが出る場合はそれを記録して後続 issue 化）
- 単体テスト（期待値明示）を追加

## 関連

- 044 Web API 体系的カバレッジ（実サイト JS 完走トラック）
- 053 element.style の CSSStyleDeclaration メソッド（同サイトの前段ブロッカー、解消済み）
