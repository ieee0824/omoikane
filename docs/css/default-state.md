# Default form controls

`:default` は、checked属性を持つcheckbox/radio、selected属性を持つoption、
form所有者の先頭submit buttonに一致する。現在のcheckedness/selectednessとは区別する。
先頭buttonがdisabledでも既定の対象である。外部のform属性による関連付け、
文書内の順序、shadow treeと切り離されたformも評価する。

buttonの型判定はDOMのtype getterとCSSで共通の純粋関数を使う。
明示的なsubmitが優先され、Auto状態ではcommand/commandfor属性の存在と
親selectを評価する。属性は名前空間なしのものを使い、form参照のIDはUTF-16を
そのまま比較する。getElementByIdも同じ文字単位の比較を使う。

submit buttonの索引は変更のない選択子評価中にrootごとに共有する。
キャッシュに保持するのはIDだけであり、DOMノードを保持しない。
パーサー由来のform所有者は、有効性を確認して読み取り専用で参照する。
型・属性・順序の変更は`:has()`やfiltered sibling selectorsにも反映する。
関数形式はCSSOM・DOM検索・CSS.supportsで拒否する。

仕様は[HTMLのdefault状態](https://html.spec.whatwg.org/multipage/semantics-other.html#selector-default)と
[buttonのAuto条件](https://html.spec.whatwg.org/multipage/form-elements.html#the-button-element)を参照する。

## 検証

```sh
cargo test --locked --test css_default_state --test css_form_states --test css_placeholder_shown
cargo test --locked --lib css::matcher::default_state::tests
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/default-state.json cargo test --locked --test wpt_smoke selected_wpt_testharness_cases_match_expectations -- --nocapture
```

API・動的スタイル・名前空間・正確なID・tree scopeを9ケースで確認する。
索引の共有、DOMの解放、読み取りビュー内の評価は別の単体テストで確認する。
固定WPTはrevision `dff7cf45938f6e16427f5607bac82a803534484b`の
`html/semantics/selectors/pseudo-classes/default.html`原本を使う。
原本の2 subtestはOmoikane・FirefoxともにPASSした。

## Firefoxとの比較

Firefox 158.0.1 / geckodriver 0.37.1で同じHTML・JavaScriptを実行した。
9ケース中7ケースが一致し、残るDOM一致・スタイルの2ケースは、
command属性の動的な変更で次の差があった。

| 操作後の状態 | Omoikane / 現行HTML | Firefoxで観測した結果 |
| --- | --- | --- |
| 先頭Auto buttonへcommandを追加 | type=button、後続submit buttonがdefault | type=buttonだが先頭がdefaultのまま |
| commandforを除去しpopovertargetだけを残す | type=submit、先頭がdefault | type=submitだが後続がdefaultのまま |

差が出た2ケースは最初の差でfalseを返すため、追加の追跡用スクリプトでも
全段階のtype・一致結果・算出色を記録した。select親のAuto状態と明示的なsubmit状態は
この追跡でも確認した。実装と回帰テストの期待値はHTMLのsubmit buttonの条件に基づく。

#1278の他のフォーム・要素・メディア状態は引き続き同Issueで扱う。
