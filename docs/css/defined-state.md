# Element definition state

`:defined` はHTMLのcustom element stateが`uncustomized`または`custom`である要素に一致する。
通常要素・HTML名前空間以外の要素・予約名は初期状態で一致し、未登録のcustom element名や
作成時に`is`を指定したHTML要素は一致しない。upgradeの構築中・失敗時も一致せず、
成功時に一致する。直接登録済みクラスを`new`する場合は`super()`内でcustom状態になる。
関数形式の`:defined(...)`はCSSOM、DOM検索、CSS.supportsで拒否する。

DOMは判定結果と作成時の`is`値を所有する。後から`is`属性を変更しても作成時のidentityは変わらない。
HTML/XMLパーサーとDOM作成APIで初期化し、cloneはこの値を複製する。
clone/importの構築にはコピー先文書のregistryを使い、元のregistryで先にupgradeしない。
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

scoped registryの作成・initializeと、DOM作成APIの名前境界は引き続き未対応・未完了である。
`custom-elements/registries/pseudo-class-defined.window.js`の3ケースはこの段階の成功件数に含めない。
全体の進捗はIssue #1278で管理し、この実装だけで同Issue全体を完了とは扱わない。
