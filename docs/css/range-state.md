# Range状態と入力値

`:in-range` / `:out-of-range`を、HTML inputの現在の値・型・範囲から判定する。
対象はnumber、range、date、month、week、time、datetime-local。HTML名前空間と
local nameで分類し、属性はnull名前空間だけを読む。関数形式の選択子は受理しない。

constraint validationの候補であり、min/maxによる範囲制限がある場合にだけ一致する。
readonly、実際のdisabled状態、datalistの子孫はどちらにも一致しない。
fieldsetの最初のlegendの例外は祖先ごとに評価する。空の値はunderflow/overflowに
ならない。timeのmin > maxは周期をまたぐ範囲とし、maxとminの間の値には両フラグが立つ。
rangeには既定のmin=0/max=100がある。

## 所有と更新

実際のdirty値は既存のprivate JS状態にUTF-16のまま保持する。Native DOMは型と数値の
比較用scalarだけを所有し、文字列・DOMハンドル・借用を長期保持しない。
値設定、適用される属性変更、clone/import、resetの境界で同期してstyleを無効化する。
型変更で文字列が同じでもscalarを更新する。disabled/readonly/requiredのIDL反映も
null名前空間だけを更新し、foreign属性を保持する。cleanなDOMでは属性から直接計算するため、
JavaScript runtimeがなくてもmatchingできる。Native matchingは作者定義のIDL getterを呼ばない。

日付、月、ISO週、時刻、local datetimeの構文検査と比較用数値への変換は純粋関数で行い、
現在時刻・タイムゾーン・ホストDateの年上限を参照しない。年の構文検査は文字列から
400年周期を求める。時刻は整数ミリ秒で組み立ててから比較値へ変換する。
local datetimeはT区切りと最短の時刻表記へ正規化する。
入力値の構文検査と、属性に適用するHTMLの浮動小数点解析を分ける。

range値は既定値、clamp、step丸めを適用する。有限binary64値の最短十進表記を
整数格子へ変換し、同距離なら正方向を選ぶ。許容誤差は使わない。
stepMismatchも同じNativeの十進格子で判定する。型のstep scaleを整数で掛けるため、
スケール後のbinary64のoverflowや許容誤差に依存しない。
整数計算にはBoaから既に間接依存しているnum-bigint 0.4.8を直接使用する。
追加のpackage/versionはなく、範囲・stepのブラウザ仕様は自前で実装する。
指数幅は有限binary64の範囲であり、作者の巨大な指数文字列をそのまま整数の桁数にしない。

stepに揃った値の`5.00`や`.3`という表記は保持する。dirty値のvalue属性を変えただけでは
再正規化しない。同一のmin/max/step属性の再設定も値を変えない。
`valueAsNumber`の取得・設定、`stepUp` / `stepDown`も現在値と比較用scalarを更新する。
数値設定でInfinity/BigIntはTypeError、非対応の型への設定はInvalidStateErrorとなる。
これらの追加はHTMLInputElement全体の対応完了を意味しない。

固定WPTのrange.htmlが検査する算出overflowのため、overflow-x/yの初期値visibleと、
算出shorthandの軸の直列化も補う。DOMから外された要素の空の算出宣言には
初期値を合成せず、保持済みの算出宣言も空文字列へ追従する。

## 検証と参照ブラウザ

`tests/css_range_states.rs`では文法、型、空値、validationの対象外、周期的time、
clamp/step、dirty/clone/import/reset、relational style、名前空間、private matching、
十進表記、属性の数値解析、数値API、算出overflowを検査する。
`src/dom/input_value.rs`と`input_range.rs`には構文・数値変換・stepの単体テストがある。

固定revisionの原本8ファイルを`tests/wpt/range-state.json`と通常manifestへ追加した。
選択子3ファイル、number.html、range.html、range-2.html、input-valueasnumber.html、
datetime-local.htmlが対象。原本・アサーション・期待値は変更しない。

Firefox 158.0.1ではmonth/weekがtextへフォールバックするため、これらを含む原本と
独自ケースに差がある。また、HTMLが定める属性のUA数値解析と異なり、空白・先頭+・
数値接頭辞を含むminを無効として扱う観測がある。同じJavaScriptケース16件の比較では14件が一致し、この2点に差がある。Nativeでは
値の構文を緩めず、属性は
仕様のUA解析を適用する。この差をFirefoxへ合わせて期待値を変更しない。
負のstep回数による逆方向の移動はFirefoxの観測と一致させている。

- [HTML inputの各状態と数値API](https://html.spec.whatwg.org/multipage/input.html)
- [日時と数値のmicrosyntax](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html)
- [HTMLのrange選択子](https://html.spec.whatwg.org/multipage/semantics-other.html#selector-in-range)
