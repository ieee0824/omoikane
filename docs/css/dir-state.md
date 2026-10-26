# `:dir()` とHTML directionality

OmoikaneはSelectors Level 4の `:dir(ltr)` / `:dir(rtl)` を、CSSの
`direction` プロパティではなくHTMLのdirectionalityアルゴリズムで評価する。
引数には単一のCSS識別子を受け付ける。`ltr` / `rtl` はASCII大文字小文字を
区別せず照合し、それ以外の識別子は構文上有効だが要素にはmatchしない。
空、複数の識別子、文字列は構文エラーになる。

## 状態の算出

- 有効な `dir=ltr` / `dir=rtl` は指定方向を使う。
- `dir=auto` と暗黙のauto状態を持つ `bdi` は、最初のstrong bidi文字を使う。
- 対象となる `input` / `textarea` はcontent attributeではなくlive valueを使う。
- `bdi`、`script`、`style`、`textarea`、有効な `dir` を持つ子孫は、祖先の
  auto走査から除外する。
- Shadow DOM内の通常要素はhostからdirectionalityを継承する。`slot` のauto
  判定はassigned nodesを使い、manual slot assignmentの変更にも追従する。
- foreign element自身は `dir` 属性を解釈せず、親のdirectionalityを継承する。

HTMLのレンダリング規則は低優先度のpresentational hintとしてCSS
`direction` に反映する。有効な `dir` または暗黙autoの `bdi` にだけhintを
設定するため、`dir` を持たないslotted nodeのCSS値はflat treeのslotから
継承する一方、`:dir()` はordinary treeに基づくHTML directionalityを返す。

方向性に影響するDOM、属性、text、live form value、slot assignmentの変更は
style cacheを無効化する。live valueはDOM要素が所有する文字列として保持し、
長寿命の借用は追加しない。

## 検証

専用回帰テスト:

```sh
cargo test --locked --test css_dir_state -- --nocapture
```

固定revisionのWPT:

```sh
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/dir-state.json \
  cargo test --locked --test wpt_smoke \
  selected_wpt_testharness_cases_match_expectations -- --nocapture
```

WPTには動的text/form value、Shadow DOMとnamed/manual slot、Document/HTMLElement
の `dir` reflector、HTMLでは無視されXMLではtextとして扱われるCDATAを含む。
HTMLのCDATA風markupはbogus commentとして最初の `>` で終了し、後続要素を
飲み込まない。

仕様:

- [HTML directionality](https://html.spec.whatwg.org/multipage/dom.html#the-directionality)
- [Selectors Level 4 `:dir()`](https://drafts.csswg.org/selectors-4/#the-dir-pseudo)
