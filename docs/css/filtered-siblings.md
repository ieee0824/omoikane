# Filtered sibling selectors

`:nth-child(An+B of S)` と `:nth-last-child(An+B of S)` は、`S` に一致する
要素の兄弟だけを数える。`S` は既存の複合選択子・結合子・選択子リストの
matchingを使い、対象自身が一致しなければ結果はfalseになる。
重複して一致する枝があっても同じ要素を二度数えない。

詳細度は擬似クラス自身と、`S` の最大の詳細度の和である。
`:is()` / `:where()` の無効な枝は除外し、matchingにも詳細度にも使わない。
一方、`of` 直下の選択子リストはunforgivingであり、無効な枝があれば全体を拒否する。
`CSS.supports(selector(...))` は、forgivingな枝に未対応構文があってもfalseを返す。

`SimpleSelector::NthChild` はformulaと選択子リストを所有する。
クラス・属性・style属性・操作状態・フォーム状態・scopeの依存先もこのASTを辿る。
兄弟位置のcacheは1回の変更のないmatching処理に閉じ、scopeと選択子の値をキーにする。
DOMノードを所有せず、`:visited` の履歴情報を順序へ反映しない。

Selectors 4に従い、文書要素、切り離された要素、DocumentFragment内の要素も
inclusive element siblingsとして扱う。Acid3原本の旧test 35との仕様差は
[fixtureの説明](../../tests/fixtures/acid3/README.md#selectors-4-と-test-352026-10-25)と
[固定期待結果](../../tests/fixtures/acid3/selectors4-expectation.json)で管理する。
原本の99/100と失敗ログは保持する。

## 検証

- API・cascade・DOM変更・forgiving parsing・scope:
  `cargo test --locked --test css_nth_filter --test css_selector_supports --test dom_selector_cache`
- cacheの再利用・ノードの解放・visitedの秘匿:
  `cargo test --locked --lib css::matcher::sibling_tests`
- 固定revisionのWPT原本（3ファイル、59 subtest）:
  `scripts/fetch-wpt.sh` の後、
  `WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/filtered-siblings.json cargo test --locked --test wpt_smoke selected_wpt_testharness_cases_match_expectations -- --nocapture`
- Acid3の生スコア・正確な失敗集合・完走:
  `cargo test --locked --test acid3_harness`

Firefox 158.0.1 / geckodriver 0.37.1で、`tests/css_nth_filter.rs` と同じHTML・JavaScriptを
9ケース比較し、すべて一致した。兄弟順序、構文、詳細度、DOM変更、scopeに加え、
文書をアクティブにした状態のfocusによる兄弟の再評価も確認した。
focusのケースでは`document.hasFocus() === true`、`activeElement.id === "a"`、
`activeElement.matches(":focus") === true`を別途確認した。

#1278にある他のフォーム・メディア等の状態擬似クラスは引き続き同Issueの対象である。
