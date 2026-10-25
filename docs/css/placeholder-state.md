# Placeholder state

`:placeholder-shown` は、placeholder属性を持つ空値のinput・textareaに一致する。
inputはtext/search/tel/url/email/password/numberと、未知の型のtext fallbackが対象である。
空文字のplaceholder属性も対象になる。HTML名前空間とlocal nameで要素を判定し、
属性は名前空間なしのものを使う。関数形式はCSSOM・DOM検索・CSS.supportsで拒否する。

dirty valueの実体はJavaScript側のUTF-16文字列として保持する。
DOMが所有する`Option<bool>`には空値かどうかだけを同期し、reset後は属性・既定値を使う。
値・型の変更でスタイルを無効化し、`:has()`や`:nth-child(of :placeholder-shown)`も
再評価する。cloneの値の複製は内部状態を使い、`__value`のgetterや
`HTMLInputElement[Symbol.hasInstance]`を経由せず、単独サロゲートも保持する。

初期属性値とIDLの値設定には、型・multiple属性・UTF-16入力を明示的に渡す
共通の純粋な正規化関数を使う。text系は改行を除去し、URL/emailは前後のASCII空白も除去する。
multiple emailは各アドレスの前後の空白を除去し、numberの不正な値は空にする。
未変更のJavaScript文字列は同じストレージを使い、編集が必要な場合にUTF-16を生成する。
inputのtype・value/defaultValue・multipleの反映も名前空間なしの属性を使う。

仕様は[HTML inputの値処理](https://html.spec.whatwg.org/multipage/input.html#number-state-(type=number))、
[浮動小数点の構文](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-floating-point-number)、
[placeholder状態](https://html.spec.whatwg.org/multipage/semantics-other.html#selector-placeholder-shown)を参照する。

## 検証

```sh
cargo test --locked --test css_placeholder_shown --test css_form_states
cargo test --locked --lib dom::input_value::tests
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/placeholder-state.json cargo test --locked --test wpt_smoke selected_wpt_testharness_cases_match_expectations -- --nocapture
```

`css_placeholder_shown`は、適用される型、動的スタイル、dirty valueとclone/reset、
名前空間と構文、ユーザー定義getterを呼ばない値の複製、値の正規化、IDL属性反映、
textareaの初期改行を8ケースで確認する。同じHTML・JavaScriptを
Firefox 158.0.1 / geckodriver 0.37.1で比較し、8ケースすべて一致した。
固定WPTはrevision `dff7cf45938f6e16427f5607bac82a803534484b`の原本を使う。

inputのその他の型の値処理と、#1278の残る状態は引き続き同Issueで扱う。
