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
- 差分ピクセル数は現在 40,379
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
- 追加した回帰テスト
  - `layout::tests::percentage_height_in_auto_sized_container_becomes_auto`
  - `layout::tests::percentage_width_resolves_for_positioned_elements`
  - `css::tests::expands_background_attachment_and_position`
  - `css::tests::tokenizes_negative_dimensions`
  - `paint::tests::paints_background_image_with_position_offset`
  - `paint::tests::fixed_background_image_uses_viewport_origin`
- 追加した Acid2 回帰テスト
  - `paint::tests::acid2_eyes_block_layer_stays_overlapping_float_layer`
- float / absolute の `width: auto` を初期 layout 時点で shrink-to-fit するよう寄せ、`.smile` 内の nested float が zero-height block child (`strong { width: 6em; display: block; }`) 由来の幅を失わないようにした
- positioned `width: auto` は、初期 shrink-to-fit 幅で一度レイアウトしたあと、実際の child/line 使用幅が広ければその幅で再レイアウトするようにし、`.eyes` の親 absolute box が `#eyes-b/#eyes-c` 幅を取り込んだ後に `#eyes-a` の line box も right align し直せるようにした
- 追加した Acid2 回帰テスト
  - `paint::tests::acid2_smile_nested_float_keeps_block_width_source_descendant`
- 強化した Acid2 回帰テスト
  - `paint::tests::acid2_eyes_inline_layer_stays_at_same_origin_as_float_and_block_layers`
- 現時点の主な残差分は、Acid2 本体側で `eyes-a` / `eyes-b` / `eyes-c` の layering と inline 配置が崩れ、目が横長の赤帯として描かれていること
- 公式比較差分は今回 `40,872 -> 40,423` まで改善し、`.eyes-c` が右へ逃げる崩れはひとまず抑えられた
- 負 margin 自体は通るようになったが、下半分の位置ずれはまだ大きい。`.smile` の nested float 幅崩れは回帰テストで固定でき、ignored の公式比較も `40,999 -> 40,379` まで戻せた一方、主戦場は引き続き `.eyes-a` の inline image / fixed background の最終 paint と下半分の細かな位置合わせにある

## 子issue

- [012-1 `<link rel="stylesheet">` による外部スタイルシート適用](012-1-link-stylesheet-loading.md)
- [012-2 負margin collapsingと clear の負clearance](012-2-negative-margin-collapsing-and-clear.md)
- [012-3 `overflow: hidden` によるクリッピング](012-3-overflow-hidden-clipping.md)
- [012-4 `display: table` / `table-cell` レイアウト](012-4-table-layout.md)
- [012-5 `min-height` が `max-height` を override する処理](012-5-min-max-height-override.md)

## タスク
- [x] 公式比較の差分画像を見て、主要な未実装要素を特定する
- [x] 参照ページ側で欠けている描画要素（背景、見出しテキスト、画像配置など）を切り分ける
- [x] Acid2 本体側で欠けている描画要素（stacking / inline alignment / positioned paint order など）を切り分ける
- [x] 必要なら子 issue に分割して段階的に差分を減らす
- [ ] 子issueを順次実装し、ignored の公式比較テストを常時通る状態へ近づける

## 次フェーズのプラン
1. `.eyes` の 3 レイヤーを個別に固定する
   - `#eyes-c` は通常 flow block として最下層に残ること
   - `#eyes-b` は float としてその上に乗ること
   - `#eyes-a` は inline content を含む `height: 0` box として最上層に乗ること
2. `#eyes-a object` の inline formatting を詰める
   - fallback 後の replaced content が inline のまま line box に参加すること
   - `vertical-align: bottom` と `text-align: right` が目画像の最終位置に効くこと
   - inline の `width` / `height` 指定が Acid2 コメントどおり効かないこと
3. positioned descendant の stacking を狭く修正する
   - Appendix E 全体を一気に実装するのではなく、`positioned + float + inline` の混在ケースを優先する
   - `.eyes` の子孫については descendant 順ではなく paint phase 順で描けることを回帰テストで固定する
4. 目が安定したら smile / chin を次の子タスクに切り分ける
   - 口は `relative + absolute + nested float`
   - あごは `line-height` と fixed background の最終確認

## 直近の確認観点
- `#eyes-a` の line box が `#eyes-b` と `#eyes-c` に押し下げられず、同じ `.eyes` 原点付近に残ること
- `#eyes-a` の inline image fragment が border/padding を含んだ外形で right align されること
- `#eyes-b` の float width が 10em + 左右 border として確保されること
- `.eyes` の paint 順が block -> float -> inline になっていること
