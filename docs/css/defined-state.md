# Element definition state

`:defined` はHTMLのcustom element stateが`uncustomized`または`custom`である要素に一致する。
通常要素・HTML名前空間以外の要素・予約名は初期状態で一致し、未登録のcustom element名や
作成時に`is`を指定したHTML要素は一致しない。upgradeの構築中・失敗時も一致せず、
成功時に一致する。直接登録済みクラスを`new`する場合は`super()`内でcustom状態になる。
関数形式の`:defined(...)`はCSSOM、DOM検索、CSS.supportsで拒否する。

DOMは判定結果と作成時の`is`値を所有する。後から`is`属性を変更しても作成時のidentityは変わらない。
HTML/XMLパーサーとDOM作成APIで初期化し、cloneはこの値を複製する。
clone/importはscoped registryの関連付けを維持し、global registryはコピー先文書の
effective global registryへ置き換える。nullのelementにはimport時のfallbackを使う。
shadow rootやtemplate contentへimportのfallbackを伝播しない。
Document自身のcloneもscoped registryを維持し、その子孫を各registryでupgradeする。
global registryは複製先Documentへ引き継がず、nullの候補をglobal定義でupgradeしない。
JS側の状態変化をsame-originの内部native関数からDOMへ同期し、styleを無効化する。
CSS matchingは作者のlocalName、namespaceURI、custom element stateのgetterを呼ばない。
`:has()`、兄弟選択子、`:nth-child(of :defined)`もupgrade後に再評価する。

組み込み要素の拡張では、登録名・local name・作成時の`is`値から定義を検索する。
HTMLコンストラクター間では内部tokenで実際のinterfaceを伝え、登録内容と異なる
HTML interfaceへの`super()`呼び出しを拒否する。複数タグで共有するinterfaceや
`Reflect.construct()`もこの実際の呼び出し元で判定する。組み込み要素にはattachInternalsを許可しない。

名前判定はRustの純粋関数をregistryとnative CSSで共有する。
現行HTML/DOM規則に従い、先頭ASCII小文字、ハイフンを含むこと、ASCII大文字や
DOM local nameで禁止される文字・予約名を含まないことを確認する。
古いPCEN文字範囲の制限は使わない。

参照: [HTML custom elements](https://html.spec.whatwg.org/multipage/custom-elements.html)、
[DOM element definition state](https://dom.spec.whatwg.org/#concept-element-defined)。

検証は`tests/css_defined_states.rs`と固定原本WPT2件を使う。
通常WPT manifestにも同じ原本を登録し、期待値を書き換えない。

## Scoped custom element registries

`new CustomElementRegistry()`で独立した定義集合を作成できる。同名の要素でも、作成時の
`customElementRegistry`と`initialize(root)`による関連付けから定義を選ぶ。
scoped registryでの組み込み要素拡張は拒否する。直接の`new`はglobal registryへfallbackし、
upgrade中は共有construction stackで選択中のregistryを保持し、入れ子の失敗後も元に戻す。
callbackと`observedAttributes`・`disabledFeatures`の取得は定義中の例外後にも回復できる。
`disabledFeatures`に`shadow`を含む定義は、既存のShadow Rootを持つ要素のupgradeを
コンストラクター実行前に失敗させる。open/closedの双方で`:defined`はfalseのままとなり、
同じ定義によるShadow Rootのない要素の後続作成には影響しない。
内部のフラグ判定では保存済みの組み込み関数を使い、作者が置換した`Set.prototype.has`を呼ばない。
文字列sequenceはiterableなobjectを要求し、iterator/nextのgetterを各一度だけ取得する。

Document、Element、ShadowRootは所有するregistryを返す。関連付けは作者が変更できる
公開propertyに保存せず、realm間で共有する内部状態で管理する。scoped registryは移動先文書を
定義のupgrade候補として記録し、adoptionはglobal/scoped/nullの区別を維持する。
`initialize()`の走査は入れ子のshadow rootを越えない。

ShadowRootの既定値はhostのregistryではなく文書のregistryである。
`shadowrootcustomelementregistry`を持つ宣言的shadow rootはnullを保つ。
fragment parserはcontextのregistryを使い、template contentはinertのままとする。
`getHTML()`の属性出力は文書とshadow rootの実際のregistryから決める。
clonableなshadow treeは浅いhost複製でも子孫を複製し、scriptの未実行・実行済み・inert状態も維持する。
`document.write()`はscript実行の境界で新規nodeを登録し、内部callbackでnamed propertyと
upgradeを更新する。通常のtemplate contentをこの走査で早期upgradeしない。

検証は`tests/scoped_custom_elements.rs`と固定原本35ファイルの
`tests/wpt/scoped-registry.json`で行う。同じ原本を通常manifestにも登録する。
`custom-elements/registries/pseudo-class-defined.window.js`の3ケースを含む。
原本の期待値、実行時間制限は変更しない。

Firefox 158.0.1で`dom.scoped-custom-element-registries.enabled=true`として26契約を比較すると、
20契約が一致し、6契約に差が残った。差はscopedでのbuiltin拡張拒否に関する2ケース、
shadow cloneのserializable、Document cloneのregistry、template内の明示的scoped候補のupgrade、
別realmでのscoped form control作成である。これらをFirefox側の成功とは数えない。
Omoikaneの期待値は[HTMLの定義登録・検索](https://html.spec.whatwg.org/multipage/custom-elements.html#dom-customelementregistry-define)と
[DOMのclone規則](https://dom.spec.whatwg.org/#concept-node-clone)に従う。
registryの内部定義データは同一runtimeのrealm間で共有し、cloneはtemplate contentを含む
作成順にupgrade候補を処理する。authorのchildNodes/content/ownerDocument getterを複製時に呼ばない。


残件はIssue #1278・#1269で管理する。固定revisionのregistry全40ファイルに対し、
次の5ファイルは上の35ファイルに含めていない。全40ファイルの成功とは扱わない。

- `initial-about-blank.window.js`: 初期about:blankから同一originへの遷移でWindowの状態を再利用し、文書のregistryだけを置き換える処理が未完了。
- `scoped-registry-effective-global-registry.html`: 多数のiframeを作成する原本が現行の実行時間上限に達する。完走・全assertionの成功は未確認。
- `scoped-registry-append.html`: globalからinert文書、scoped親へ移動するケースに原本期待と差が残る。現行DOMのeffective global registry規則との照合が必要。
- `scoped-custom-element-registry-customelementregistry-attribute.html`および同`-in-xhtml.xhtml`: 旧提案のelement属性を前提とする。現行HTMLには含まれず、[HTML PR #12000](https://redirect.github.com/whatwg/html/pull/12000)・[DOM PR #1500](https://redirect.github.com/whatwg/dom/pull/1500)は調査時点で未マージ。

DOM作成APIの名前境界・不正UTF-16、parser token直後のconstructor実行順、GC/retirementの
追加検証も残る。この実装だけで`:defined`・Issue #1278全体を完了とは扱わない。
