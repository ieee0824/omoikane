# Required and optional form controls

`:required` と `:optional` はHTML名前空間のinput・select・textareaに対して評価する。
inputはrequired属性が適用される型だけが対象であり、hidden・range・color・button・
image・reset・submitはどちらにも一致しない。未知の型はtextとして扱う。
disabled・readonlyはrequirednessを変えない。

属性は名前空間なしのtype・requiredを読み、requiredの値ではなく存在を使う。
XML文書の接頭辞付きHTML要素もlocal nameで判定する。
型・required属性の変更は既存のスタイル無効化を使い、`:has()`や
`:nth-child(of :required)`も再評価する。新しい保持状態や公開APIは追加しない。
関数形式はCSSOM・DOM検索・`CSS.supports()`のいずれでも無効とする。

仕様は[HTMLのrequired/optional](https://html.spec.whatwg.org/multipage/semantics-other.html#selector-required)
を参照する。

## 検証

`cargo test --locked --test css_form_states` は型の適用範囲、属性の名前空間、
動的スタイル、構文拒否を4ケースで確認する。同じHTML・JavaScriptを
Firefox 158.0.1 / geckodriver 0.37.1で実行し、4ケースとも一致した。

固定WPT revision `dff7cf45938f6e16427f5607bac82a803534484b` の
`required-optional.html` は原本の6 subtestすべてがOmoikane・FirefoxでPASSした。
取得後は次のコマンドで実行する。

```sh
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/form-requiredness.json cargo test --locked --test wpt_smoke selected_wpt_testharness_cases_match_expectations -- --nocapture
```

同revisionの`required-optional-hidden.html`も原本のまま比較した。
このテストはhidden inputが`:optional`に一致する期待を持ち、OmoikaneとFirefoxの
両方で1 subtestがFAILした。失敗はhiddenの隣接spanの背景色が期待pinkに対してlimeに
なる箇所で一致する。現行HTML仕様ではrequiredが適用されないinputは`:optional`の
対象外であるため、実装と原本の期待を変更しない。このファイルは取得対象に含めるが、
PASSを要求するmanifestには追加しない。textからhiddenへの動的変更は独立した
`css_form_states`のテストで確認する。

Issue #1278の他のフォーム状態・要素状態・メディア状態は引き続き同Issueの対象である。
