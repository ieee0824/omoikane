---
number: 012
slug: acid2-official-conformance
status: open
---

# Acid2 公式リファレンス一致

公式 `reference.html` と `reference.png` を fixture 化し、比較用の ignored テストも追加したが、
現状の `acid2.html` レンダリングは公式リファレンスと大きく乖離している。

## 現状
- `paint::tests::acid2_fixture_matches_official_reference_rendering` を `--ignored` で実行すると失敗する
- 差分ピクセル数は現在 22,006
- 差分画像は `tests/output/acid2/acid2.official-reference.{actual,expected,diff}.png` に出力される

## 進捗メモ
- `background: none` が `background-image` しか消していなかった問題を修正し、`.picture` の赤ベタ背景を解消
- `float: left/right` と `clear: both` の最小対応を入れ、Acid2 部品の横配置を少し改善
- `border-top` / `border-left` など side shorthand を展開し、side ごとの border color paint に対応
- color keyword に `yellow` / `navy` / `purple` / `maroon` を追加し、Acid2 の配色を改善
- ローカル baseline `acid2.baseline.png` は今回の描画改善に合わせて更新済み
- `head` / `title` / `meta` / `style` / `script` / `link` をレイアウト対象から除外し、`reference.html` 側の不要テキスト描画を抑制
- `title` テキストが `<head>` ではなく本文側へ落ちていた tree builder の扱いを修正し、`&nbsp;` の文字参照も decode するようにした
- ignored の公式比較テスト差分は `66,465 -> 26,020 -> 23,114` まで減少
- `reference.html` 側では、本文テキストが `Hello World!` のみになることをテストで確認できた
- `content: ''` の擬似要素を border 図形として描けるようにし、legacy な `:before` / `:after` も pseudo-element として扱うようにした
- inline 画像フラグメントが自身の style（padding / border / background）を持てるようにし、`object` fallback では実際に画像を解決した内側要素の style を使うようにした
- interlaced PNG を `png` クレートで decode するようにし、Acid2 の目画像自体は読み出せるようになった
- forgiving CSS parse を declaration salvage ベースに寄せ、壊れた declaration を含む rule でも有効な declaration を救えるようにした
- `#eyes-b` 相当の rule で `width` / `height` / 左右 border / `float` が残ることをテストで確認した
- paint 順序を単純な children 順から、負 z-index positioned / 通常 block / float / inline content / auto z-index positioned / 正 z-index positioned の段階描画に切り替えた
- inline replaced element で content box と border/padding box を分離し、background・border の上に画像本体を描くようにした
- 追加した診断テスト
  - `paint::tests::forgiving_parse_preserves_valid_declarations_in_partially_invalid_rule`
  - `paint::tests::inline_replaced_element_with_padding_border_and_background_paints_in_order`
  - `paint::tests::absolute_inline_content_paints_above_float_siblings`
- absolute positioned `width: auto` の shrink-to-fit 計算で inline image の padding / border を落としていた問題を修正し、`.eyes` の auto width が実画像の外形を含めて広がるようにした
- `float` 付き inline 要素や out-of-flow positioned 要素を inline line layout から除外し、nested float を block/floating child として扱う基礎を入れた
- 追加した回帰テスト
  - `layout::tests::absolute_auto_width_includes_inline_image_padding_and_border`
  - `layout::tests::floated_inline_element_is_taken_out_of_inline_line_layout`
  - `paint::tests::acid2_eyes_inline_layer_stays_at_same_origin_as_float_and_block_layers`
- non-`font-size` の `%` 値を computed style で生の percentage として保持し、`height` / `min-height` / `max-height` は containing block 高さが未確定なら `auto` 扱いに寄せた
- `max-height` / `min-height` と positioned `% width` の clamp を layout に追加し、`empty` の誤った `height: 10px` 化と scalp の shrink-to-fit 潰れを抑制した
- forgiving CSS parse で unquoted `url(data:...)` を正規化して再 parse するようにし、`background: fixed url(...)` から `background-image` を救えるようにした
- `background` shorthand から `background-attachment: fixed` と `background-position-x/y` を展開し、paint 側でも viewport 基準の fixed background と position offset を扱えるようにした
- float 後続 block の containing block を見直し、explicit `width` を持つ block は先行 float の offset で丸ごと右へ逃がさないようにして、`#eyes-c` が `#eyes-b` と水平帯域を共有するようにした
- CSS tokenizer で負の数値を `Dimension/Number` として扱うようにし、`.empty div { margin-bottom: -6em; }` のような負 margin が computed style まで落ちない問題を修正した
- escaped identifier / string を CSS tokenizer で decode するようにし、`[class=second\ two][class="second two"]` のような attribute selector が正しく一致するようにした
- 追加した回帰テスト
  - `layout::tests::percentage_height_in_auto_sized_container_becomes_auto`
  - `layout::tests::percentage_width_resolves_for_positioned_elements`
  - `css::tests::expands_background_attachment_and_position`
  - `css::tests::tokenizes_negative_dimensions`
  - `css::tests::parses_escaped_attribute_selector_values`
  - `paint::tests::paints_background_image_with_position_offset`
  - `paint::tests::fixed_background_image_uses_viewport_origin`
- 追加した Acid2 回帰テスト
  - `paint::tests::acid2_eyes_block_layer_stays_overlapping_float_layer`
  - `paint::tests::acid2_second_line_absolute_shrink_wraps_float`
- float / absolute の `width: auto` を初期 layout 時点で shrink-to-fit するよう寄せ、`.smile` 内の nested float が zero-height block child (`strong { width: 6em; display: block; }`) 由来の幅を失わないようにした
- positioned `width: auto` は、初期 shrink-to-fit 幅で一度レイアウトしたあと、実際の child/line 使用幅が広ければその幅で再レイアウトするようにし、`.eyes` の親 absolute box が `#eyes-b/#eyes-c` 幅を取り込んだ後に `#eyes-a` の line box も right align し直せるようにした
- 追加した Acid2 回帰テスト
  - `paint::tests::acid2_smile_nested_float_keeps_block_width_source_descendant`
- 強化した Acid2 回帰テスト
  - `paint::tests::acid2_eyes_inline_layer_stays_at_same_origin_as_float_and_block_layers`
- `blockquote.first.one` 配下の `address.second two` に `float:right; width:48px; height:12px` が乗る状態を回帰テストで固定し、Acid2 上半分の second line 用 shrink-wrap がレイアウト上は復活した
- `min-height > max-height` / `min-width > max-width` のときに min 側を採用する正規化を layout に入れ、CSS 2.1 §10.4 / §10.7 に沿うようにした
- 現時点の主な残差分は、Acid2 本体側で `eyes-a` / `eyes-b` / `eyes-c` の layering と inline 配置が崩れ、目が横長の赤帯として描かれていること
- 公式比較差分は今回 `40,872 -> 40,423` まで改善し、`.eyes-c` が右へ逃げる崩れはひとまず抑えられた
- 負 margin 自体は通るようになったが、下半分の位置ずれはまだ大きい。`.smile` の nested float 幅崩れは回帰テストで固定でき、ignored の公式比較も `40,999 -> 40,379` まで戻せた一方、主戦場は引き続き `.eyes-a` の inline image / fixed background の最終 paint と下半分の細かな位置合わせにある
- min/max override 修正で `layout::tests::min_height_overrides_smaller_max_height` / `layout::tests::min_width_overrides_smaller_max_width` を追加した。`cargo test --lib` は通過したが、ignored の公式比較は今回は `40,379 -> 40,427` とわずかに悪化した
- 012-6 の切り分けとして `layout::tests::object_type_width_and_height_do_not_change_nested_inline_fallback_image_size` と `paint::tests::nested_object_fallback_preserves_fixed_background_on_inline_image_fragment` を追加し、nested object fallback 上の width/height 無効化と fixed background paint 自体は成立していることを固定した
- 012-2 の切り分けとして `clear` の押し下げ量を border edge 基準へ寄せ、`layout::tests::clear_both_positions_border_edge_below_float_not_margin_edge` を追加した。局所挙動は改善したが、ignored の公式比較差分は `40,427` のままで変化しなかった
- block layout の float/clear は active float region ベースへ再設計した。`float` の配置探索と `clear` の計算はより CSS 2.1 に近づいたが、ignored の公式比較差分は引き続き `40,427` で、主戦場はまだ `smile/chin` の最終位置か `.eyes` 以外の縦配置に残っている
- 012-8 の切り分けとして `paint::tests::acid2_lower_face_boxes_keep_expected_vertical_order`、`paint::tests::acid2_empty_block_starts_shortly_after_nose`、`paint::tests::acid2_empty_block_creates_large_gap_before_smile` を追加した。`nose -> empty` と `empty -> smile` の局所 gap は大きく崩れておらず、ignored の公式比較差分も引き続き `40,427` のため、下半分の主戦場は単純な block gap ではなく paint phase / stacking か、さらに上流の縦配置に寄っている可能性が高い
- 012-7 の修正として、paint の phase 分離を immediate child ベースから一段広げ、non-positioned subtree の positioned descendant も祖先 stacking phase へ defer するようにした。`paint::tests::positioned_grandchild_paints_above_float_uncle` を追加し、ignored の公式比較差分は `40,427 -> 36,708` まで改善した。local baseline `tests/fixtures/acid2/acid2.baseline.png` も更新済み
- ignored の公式比較ハーネスでは、Acid2 側だけ `#top` 基準で上へ詰めていたため reference 側の `h2` と origin が揃っていなかった。比較前に Acid2 の `#top` content origin を reference の `h2` content origin へ合わせるようにして、より対称な比較へ寄せた結果、ignored の公式比較差分は `36,708 -> 33,957` まで改善した。これは主に比較の整合化であり、renderer 本体の改善量とは分けて扱う
- `.nose` の前提修正として、float 配置時に負 `margin-top` を打ち消していたバグを修正し、`layout::tests::float_preserves_negative_top_margin_offset` を追加した。これは鼻周辺の前提には効くが、ignored の公式比較差分 `33,957` 自体は変わらなかった
- 012-7 の追加修正として、non-positioned subtree の float descendant も祖先 float phase へ defer するように広げ、`paint::tests::float_grandchild_paints_above_block_uncle` を追加した。`cargo test --lib` は維持できているが、ignored の公式比較差分 `33,957` は据え置きだった
- 012-1 は調査の結果、`collect_author_stylesheets` 内で既に `<link rel="stylesheet">` の `data:text/css` URI に対応済みだった。`paint::tests::acid2_link_stylesheet_overrides_picture_background_to_none` テストで確認し close した
- 012-2 の empty element self margin collapsing を実装した。`is_empty_for_margin_collapse` / `collapse_through_empty` で empty 要素とその子孫の全 margin を再帰的に collapse する。`layout::tests::empty_element_collapses_own_margins_through` / `layout::tests::empty_element_with_negative_child_margin_collapses_through` を追加。ignored の公式比較差分は `33,957 -> 39,245` と一時的に悪化（下部要素が viewport 内に入ったため）だが、構造的には正しい方向
- block 要素間の空白テキストノードが line box を生成して cursor_y を不要に進めていた問題を修正。`pending_inline_nodes` が空白テキストのみの場合はレイアウトをスキップするようにした。ignored の公式比較差分は `39,245 -> 34,604` まで改善。baseline も更新済み
- 012-4: table container 内で table-cell/row/row-group 以外の子要素を anonymous cell として扱うようにした（CSS 2.1 §17.2.1）
- 012-4: width:auto の table container に shrink-to-fit 幅を適用し、`.image-height-test` 内 table 等が親の全幅に広がらないようにした。ignored の公式比較差分は `34,604 -> 27,884` まで改善。baseline も更新済み
- block 間空白スキップで `&nbsp;`(U+00A0) を ASCII 空白と同様にスキップしていた問題を修正。Rust の `trim()` は Unicode 空白を除去するため、forehead 内の nbsp×30 テキストが無視されて height=0 になっていた。ASCII 空白のみのバイト判定に変更したことで forehead に高さが戻り、eyes/nose の位置関係が正しくなった。公式比較差分は `27,884 -> 34,601` と一時悪化（パーツが正しい位置に移動した過渡期）
- eyes は `.picture` 上端 +60px（top:5em）に配置され、forehead の下端と一致することを回帰テストで固定した
- `p + table + p` セレクタによる `p.bad` の margin-top:3em は正しく適用されていることを確認（position:fixed で viewport 上 y=144 に配置）
- 012-4: table の intrinsic_width をセル幅の合算で計算するようにした。`ul` テーブルの幅が 12px → 48px に修正され、4 セルが正しく横並びになった。公式比較差分は `34,601 -> 35,033` と微増（テーブル幅修正で周辺レイアウトに影響）
- 残りの赤バー（x=60〜380 付近）は `ul` ではなく `.image-height-test` 内の 64x64 red square PNG と `.chin` の fixed background が原因の可能性。次回以降の調査対象
- margin collapsing の cursor_y 計算を修正: `cursor_y += total_height` を `cursor_y += total_height - collapse_delta` に変更し、collapse_delta の二重加算を解消。同時に float 後の previous_margin_bottom を維持するよう変更。これにより nose→smile gap=48→0, smile→chin gap=42→改善。公式比較差分は `35,033 -> 22,006` に大幅改善
- HTML table/tr/td/th タグを CSS display 指定なしでもデフォルトの table display 値で認識するよう改善。`.image-height-test` 内の `<table>` が 740px→64px に修正（overflow:hidden で見た目は同じ）

## 子issue

- [012-1 `<link rel="stylesheet">` による外部スタイルシート適用](../closed/012-1-link-stylesheet-loading.md)
- [012-2 負margin collapsingと clear の負clearance](012-2-negative-margin-collapsing-and-clear.md)
- [012-3 `overflow: hidden` によるクリッピング](../closed/012-3-overflow-hidden-clipping.md)
- [012-4 `display: table` / `table-cell` レイアウト](012-4-table-layout.md)
- [012-5 `min-height` が `max-height` を override する処理](../closed/012-5-min-max-height-override.md)
- [012-6 `#eyes-a object` の inline fallback / fixed background 調整](012-6-eyes-inline-fallback-and-fixed-background.md)
- [012-7 `.eyes` の stacking / paint phase 整合](012-7-eyes-stacking-and-paint-phases.md)
- [012-8 `smile` / `chin` 下半分の位置合わせ](012-8-smile-and-chin-alignment.md)

## タスク
- [x] 公式比較の差分画像を見て、主要な未実装要素を特定する
- [x] 参照ページ側で欠けている描画要素（背景、見出しテキスト、画像配置など）を切り分ける
- [x] Acid2 本体側で欠けている描画要素（stacking / inline alignment / positioned paint order など）を切り分ける
- [x] 必要なら子 issue に分割して段階的に差分を減らす
- [ ] 子issueを順次実装し、ignored の公式比較テストを常時通る状態へ近づける
- [ ] `.eyes` と下半分の残差分を個別 issue で管理し、差分悪化時に原因領域を即座に絞れる状態にする

## 次フェーズのプラン
1. 顔パーツの縦位置はほぼ正しくなったので、次は **顔の丸み**（scalp〜forehead〜nose の黒左右 border の連続性）を改善する
2. `p.bad` の赤バーが scalp の下に見えている問題を解消する（z-index / stacking context の精密化）
3. [012-8](012-8-smile-and-chin-alignment.md) で下半分の `smile` / `chin` / `parser` / `ul` の最終位置を詰める
4. [012-6](012-6-eyes-inline-fallback-and-fixed-background.md) / [012-7](012-7-eyes-stacking-and-paint-phases.md) の残タスクを引き続き進める

## 直近の確認観点
- 顔パーツ（forehead/eyes/nose/smile/chin）の Y 座標が期待される相対順序で並んでいること（回帰テスト済み）
- `p.bad` が scalp の背後または absolute テーブルの下に隠れること（現在は赤バーとして見えている）
- `.forehead` の黄色 background-image が正しくタイリングされていること（デコードテスト済み）
- 下半分（chin/parser/ul/image-height-test）の位置が reference に近づくこと

## 実施方針
- renderer 本体の修正は子 issue 単位で進め、各 issue に局所 regression と ignored の公式比較をセットでぶら下げる
- 公式比較は `paint::tests::acid2_fixture_matches_official_reference_rendering --lib -- --ignored` を共通の最終確認とし、局所修正の成否は必ず diff 画像でも確認する
- comparison harness の改善と renderer 本体の改善は分けて記録し、差分減少量を混同しない
