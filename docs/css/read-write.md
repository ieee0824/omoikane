# Read-only and read-write state

`:read-write` は、readonlyが適用される可変なHTML input、disabledでもreadonlyでもない
HTML textarea、それら以外のediting host・editable要素に一致する。
`:read-only` は要素についてその補集合であり、TextやDocumentには一致しない。
inputの未知の型はtextへ正規化し、disabled fieldsetの最初のlegend例外は共通DOM判定を使う。
input・textareaはcontenteditableの祖先内でもフォームとしての可変性を優先する。
関数形式と通常の疑似要素の後の状態指定は拒否し、element-backedな`::part()`には対応する。

`src/dom/editability.rs` は実際の属性・名前空間・通常の親から編集状態を計算する。
HTMLのcontenteditableはtrue・plaintext-only・false・inheritを扱い、shadow rootを越えて継承しない。
HTML以外では、仕様上のsvg・mathルートなどの許可されたnode kindに限定する。
CSSとprivateなJS bridgeが同じ判定を使い、authorのgetterや真偽値のexpandoには依存しない。

`HTMLElement.contentEditable` は正規化したキーワードを属性に反映し、inheritでは属性を削除する。
不正値はSyntaxError、SymbolのDOMString変換はTypeErrorとなる。
`isContentEditable` はediting hostまたはeditableかどうかを返す。
属性更新は既存のmutation observer・custom element反応の処理を通る。

`Document.designMode` のフラグはDocumentMetadataが所有し、初期値はoffである。
offからonへの遷移ではフラグ更新とスタイル無効化後、既存のactive rangeの両端を
Document先頭へ戻してからdocumentElementのフォーカス処理を行う。
onの再設定では範囲やフォーカスをリセットしない。不正キーワードは無視する。
別realmのsetterを借用した場合も、privateな文書間ディスパッチを使い、対象文書のrealmで
active rangeとフォーカスを更新する。文書がactiveでない場合は選択範囲とフォーカスを操作しない。
Documentのcloneはoffから始まり、adoptionでは移動先の祖先と文書状態を使う。
iframe破棄後も参照を保持した文書は固有のフラグを保持する。

仕様は[HTMLの状態照合](https://html.spec.whatwg.org/multipage/semantics-other.html#selector-read-write)、
[contenteditable](https://html.spec.whatwg.org/multipage/interaction.html#contenteditable)、
[designMode](https://html.spec.whatwg.org/multipage/interaction.html#designMode)、
[editableの定義](https://w3c.github.io/editing/docs/execCommand/#editable)を参照する。

## 検証

```sh
cargo test --locked --test css_read_write
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/read-write.json cargo test --locked --test wpt_smoke -- --nocapture
```

固定WPTはrevision `dff7cf45938f6e16427f5607bac82a803534484b`の原本を使う。
testharness原本4件に加え、css_read_writeは動的なtype変更の原本reftest2件をスクリプトから実行し、
変更前後のcomputed styleと参照画像との画素一致を確認する。
通常manifestは既存628件を保持し、testharness原本4件を追加する。

Firefox158.0.1との8複合契約は、入力型・属性反映・IDL境界・host/part・関係セレクターの5契約が一致した。
designModeの契約はactive range、編集継承の契約はフォームのisContentEditableで不一致となる。
文書の分離・複製の契約では、Firefoxはdetached DocumentのdesignMode設定でNS_ERROR_FAILUREとなる。
別の診断ではFirefoxが外国名前空間の任意の子孫にもread-writeを継承し、designModeで
元のRangeを保たずBODYに範囲とフォーカスを移すことも確認した。
複合契約は最初の不一致で停止するため、残りの副アサーションの一致を意味しない。
これらの差に合わせてHTML・editableの明示的な定義を弱めない。

これは状態照合とIDL反映の説明である。編集UI、Clipboard、execCommandの実装詳細は
[`docs/dom/clipboard.md`](../dom/clipboard.md)に記録する。#1278の他のselector familyも引き続き扱う。
