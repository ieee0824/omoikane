---
number: 053
slug: css-style-declaration-methods
status: closed
---

# element.style の CSSStyleDeclaration メソッド実装（removeProperty 等）

## 概要

inline style オブジェクト（`element.style`）に CSSStyleDeclaration の標準メソッド
（`removeProperty` / `setProperty` / `getPropertyValue` / `getPropertyPriority` / `item` / `length` / `cssText`）を実装する。

## 背景（実サイトログ由来: kasaneteto.jp）

`cargo run --example screenshot -- "https://kasaneteto.jp/"` で以下の JS エラーが発生する:

```
TypeError: not a callable function
    at register (lib.js:21:17188)          ← GSAP ScrollTrigger の register()
    at _createPlugin / registerPlugin      ← gsap.registerPlugin(ScrollTrigger, ScrollToPlugin)
    at app.bundle.js:5892
```

原因: GSAP ScrollTrigger は登録時にスクロールバー計測のため `body.style.borderTop` を一時設定し、
復元時に `n ? r.borderTop = n : r.removeProperty("border-top")` を呼ぶ。
Omoikane の `element.style`（dom_bootstrap.js）は camelCase プロパティの get/set のみで
`removeProperty` が未実装のため TypeError となり、**ScrollTrigger/ScrollToPlugin の登録ごと中断**、
ページの GSAP 依存初期化がすべて動かない。レンダリング崩れの一因。

## スコープ

- `element.style` に以下を実装:
  - `removeProperty(name)` — kebab-case 名で宣言を削除し、削除前の値を返す
  - `setProperty(name, value, priority?)` — kebab-case 名で設定（priority は保持のみで可）
  - `getPropertyValue(name)` / `getPropertyPriority(name)`
  - `item(index)` / `length` / `cssText`（get/set）
- getComputedStyle 側の読み取り専用 CSSStyleDeclaration（`__makeComputedStyle`）と挙動・命名規則を揃える
- style 属性への反映（既存の camelCase setter と同じ経路で `__omoikane_set_attribute` に反射し、
  レイアウト/カスケードの dirty 化も既存経路に乗せる）

## 受け入れ条件

- `style.removeProperty("border-top")` で該当宣言が消え、style 属性・getComputedStyle に反映される
- `setProperty`/`getPropertyValue` が kebab-case で動作し、camelCase アクセスと一貫する
- kasaneteto.jp のレンダリングで上記 TypeError が解消し、`gsap.registerPlugin` が完走する
- 単体テスト（各メソッドの期待値明示）を追加

## 関連

- 044 Web API 体系的カバレッジ（実サイト JS 完走トラック）
- 047 inline style カスケード適用（style 属性 → レイアウト反映の経路と同根）
- 051 CSS プロパティ値検証（setProperty 経由の宣言にも同一検証を適用する将来接続）
