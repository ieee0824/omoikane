# Indeterminate form state

`:indeterminate` はHTML checkboxのindeterminateフラグ、同じradioグループに
checkedな要素がないradio、value属性を持たないprogressに一致する。
属性は名前空間なしのものを評価し、HTML名前空間の接頭辞付き要素も対象にする。
radioの名前・form参照のIDはUTF-16のまま比較し、ordinary treeとform所有者を区別する。

checkboxのIDLフラグは型変更とform resetで保持し、clone/importにもコピーする。
checkednessとdirty checkednessは別の状態として扱う。radioの自動的なpeer更新は
IDL setterによる自身のdirty更新と区別し、resetでdirtyを解除する。
名前・型・form属性、ID、接続・並べ替えによるグループ変更にも追従する。
radioとdefault submitの索引は変更のない選択子評価中に共有し、キャッシュには
DOMを保持せずIDだけを保存する。

MouseEventのclickではイベント配送前にcheckednessを更新し、checkboxの
indeterminateを解除する。キャンセル時は状態を戻し、radioでは以前の相手が
現在も同じグループにいるか確認する。成功した接続済み要素の操作ではinput/changeを
配送する。別RealmのMouseEvent、再帰的なclick、disabledなclick()と直接dispatch、
通常のEventによるclickの違いも回帰テストで確認する。

inputのtype判定と値の正規化は共通の純粋なRust関数を使う。
ASCIIだけでキーワードを比較し、不明な値はtextとする。
型セレクターは修飾名ではなくローカル名を比較し、接頭辞付きinputのradio探索を可能にする。
この変更で型セレクターの既存の大文字・小文字比較方針は変更していない。
`:indeterminate()` のような関数形式はCSSOM、DOM検索、CSS.supportsで拒否する。

仕様は[HTMLのindeterminate状態](https://html.spec.whatwg.org/multipage/semantics-other.html#selector-indeterminate)、
[inputの状態・cloning・activation](https://html.spec.whatwg.org/multipage/input.html#the-input-element)、
[progress](https://html.spec.whatwg.org/multipage/form-elements.html#the-progress-element)を参照する。

## 検証

```sh
cargo test --locked --test css_indeterminate_state --test css_default_state --test css_form_states --test css_placeholder_shown
cargo test --locked --lib css::matcher
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/indeterminate-state.json cargo test --locked --test wpt_smoke selected_wpt_testharness_cases_match_expectations -- --nocapture
```

13件の回帰でIDL、動的スタイル、activation、dirty状態、名前空間、shadow tree、
form所有者、正確なUTF-16名を確認する。共有索引の単体テストは構築回数とDOMの解放も確認する。
固定revision `dff7cf45938f6e16427f5607bac82a803534484b` のWPT原本6ファイルを
`tests/wpt/indeterminate-state.json` に列挙し、通常manifestにも含める。
6ファイル・23 subtestはOmoikaneとFirefoxでPASSした。

## Firefoxとの比較

Firefox 158.0.1 / geckodriver 0.37.1で同じ13件をページ内のwindow.evalから実行し、
9件が一致した。残る4件では次の差を観測した。

| 操作・状態 | Omoikane | Firefoxで観測した結果 |
| --- | --- | --- |
| indeterminateなcheckboxをclone/import | IDLもselectorもtrue | IDLはtrueだがselectorはfalse、再代入するとtrue |
| progressに空・不正なvalue属性 | determinate | indeterminate |
| キャンセルしたradio click中に以前のpeerのグループを変更 | 以前のpeerを復元せず、対象をfalseへ戻す | 別グループのpeerも復元し、対象もtrueのまま |
| scriptでpeerを自動uncheck後、元のchecked属性を除去・再追加 | 元のpeerが再びchecked | resetまでfalseを保持 |

最初の3件は、それぞれHTMLのcloning steps、value属性の存在によるprogress状態、
legacy-canceled-activationの現在のグループ確認に基づく期待値を使う。
4件目はchecked IDL setterが自身のdirtyフラグを更新し、radioグループの他要素の
checkedness更新にはdirty更新が指定されていないことからの仕様解釈である。
Firefoxとの不一致を隠すための期待値調整は行っていない。
既存checked属性の値を再設定するだけでは属性追加にならず、dirtyでない要素でも
checkednessを戻さないことを別段階で確認する。

既存のcheckednessテストはtext inputのIDL値もfalseと期待していたため、
HTMLのchecked getterの規定とFirefoxで確認して訂正した。CSSの一致をcheckbox/radioに
限定する検証は残し、text/checkbox間の型変更と一致件数も追加した。

#1278の他のフォーム・要素・メディア状態は引き続き同Issueで扱う。
