# Parsed radio checkedness

HTML名前空間のradioは、同じordinary tree・form所有者・空でないnameを持つ
グループの中で、最後にcheckedな状態で挿入された要素を選択する。
HTMLだけでなく、XHTML/XMLの接頭辞付きHTML要素にも適用する。
peerのcheckednessを解除してもchecked属性とdirty checkednessは変えない。
そのため`:default`は保持し、最後の選択を解除するとグループ全体が
`:indeterminate`になる。form resetもcontent defaultから状態を復元できる。

フォーム所有者、名前空間、正確なUTF-16のname/IDの分類はDOMの
`form_controls::state_index`に置き、CSSとHTML/XMLパーサーで共有する。
選択子用の索引計算は読み取り専用で、結果に保持するのは数値IDだけとする。
パーサーの同期処理は別の関数で、要素の実際の挿入位置とparser form所有者が
決まった後にcheckednessを更新する。分類中に使うNodeHandleは短命の所有値である。

解析終了時に最終DOM順だけで選択し直す方法は使わない。
例えばtable内で後からfoster parentingされたradioは、先に解析したtd内のradioより
DOM上は前に現れる。`WriteParser`でも既存の兄弟の前へ新しいradioを挿入できる。
どちらも、挿入された時点のcheckednessとグループを反映する必要がある。

IDの追加で、form属性が最初に参照する要素がformでなくなる場合も同期する。
重複したhtml/body開始タグから既存要素へIDを追加する経路も対象である。
空の要素をordinary tree全体の末尾へ追加しただけでは既存IDを先取りできず、
新しい空のformとのグループ結合も起きないため、ID再関連付けの全木走査を省く。

パーサーは、radioのform属性で参照されたUTF-16 IDだけを所有する。
参照されていないIDやIDのない要素では、位置確認や再関連付けの全木走査を行わない。
WriteParserで次の入力にIDが含まれる場合は、文書と現在の挿入先treeから参照を
読み直し、削除済みの参照IDを落とす。挿入先が切り離されたりadoptされた場合も
対象にする。shadow/template treeも走査するが、保持するのはID文字列だけである。
SVGなどHTML以外の要素のIDがform参照を遮る経路も同期する。

根拠は[HTMLのradio button group](https://html.spec.whatwg.org/multipage/input.html#radio-button-group)と
[form所有者の関連付け](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#association-of-controls-and-forms)を参照する。

## 検証

```sh
cargo test --locked --test css_radio_initialization --test css_indeterminate_state --test css_default_state --test css_form_states --test css_placeholder_shown
cargo test --locked --lib html::tree_builder
cargo test --locked --lib xml::
```

6つの回帰は、JSを使わない初期化、外部formとreset、foster parenting、
scriptで作成したpeerとdocument.write、重複ID、接頭辞付きXHTMLを確認する。
別のWriteParser単体テストは、JavaScriptを介さずに既存の選択済み兄弟の前へ挿入し、
挿入順に従うことを確認する。作成後にradioへ変わった要素とSVG IDの経路も確認する。
document.writeの回帰は、旧peerの算出色も赤から緑へ変化することを確認する。
CSSの索引共有・DOM解放テストも維持する。

既存の位置確認カウンターを使った深いHTML/XMLの回帰では、64要素から128要素へ
増やしたときの仕事量を線形に制限する。同じHTML入力の修正前は2285回→8653回、
修正後は70回→134回だった。XMLは修正後67回→131回である。
削除済みのradioが参照していたIDを後の入力が使う場合も、同じ線形の制限を確認する。

#1278の残るフォーム・要素・メディア状態は同Issueで継続する。
