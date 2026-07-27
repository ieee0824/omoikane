---
number: 092
slug: object-fit-core
github: 246
status: closed
priority: high
---

# replaced element の object-fit・object-position core を実装する

## 概要

画像等を content box 全面へ単純 scale していた処理を CSS Images の object sizing に置き換える。

## 背景

- img の描画経路は 2 箇所だけ: `paint_replaced_image_box`（positioned）と `paint_inline_image_fragment`（inline / block）。どちらも `draw_image_scaled_clipped(image, content_box, clip)` で content box 全面へ stretch していた
- `object-fit` / `object-position` は `is_supported_property` に無く、CSSOM の宣言検証で拒否され、初期値も存在しなかった

## Firefox 152 実測

Marionette 経由で computed value 33 パターンと、full-page screenshot による描画ジオメトリを実測した。

### computed value

`object-fit` は初期値 `fill`、`contain` / `cover` / `none` / `scale-down` を受け付け、ASCII 大文字小文字を区別しない。不正値・複数値・`initial` / `unset` は宣言破棄で `fill` になる。

`object-position` は **2 成分の `<x> <y>` へ正規化**される。edge キーワードは百分率（`left` → `0%`、`right` → `100%`、`top` → `0%`、`bottom` → `100%`、`center` → `50%`）、長さは px（`2em` → `32px`）。1 値指定は他軸が `50%`。`top center` は `50% 0%` へ**順序が入れ替わる**。素の `0` は長さ扱いで `0px`。負の百分率も有効。

### 描画ジオメトリ

4x2 の画像を 100x100 の content box へ描画した destination rect:

| fit / position | rect |
|---|---|
| `fill` | (0,0,100,100) |
| `contain` | (0,25,100,50) |
| `cover` | 200x100 を box へ crop |
| `none` | (48,49,4,2) |
| `scale-down`（box が大きい / 2x1 box / 4x2 box） | `none` / `contain` (0,0,2,1) / `none` (0,0,4,2) |
| `contain` + `left top` / `right bottom` | (0,0,100,50) / (0,50,100,50) |
| `cover` + `0% 50%` / `100% 50%` | 左端揃え / 右端揃え（負の free space） |
| `none` + `25% 75%` / `10px 20px` / `left` | (24,74,4,2) / (10,20,4,2) / (0,49,4,2) |

## 実装

| 層 | 内容 |
|---|---|
| `src/css/style.rs` | `is_supported_property` に 2 プロパティ追加。`validate_declaration` で `object-fit` のキーワード検証と `object-position` の文法検証（`object_position_components` が 1〜2 値形式の軸割り当てを行う）。`compute_value` の `object-position` 分岐が `render_clip_path_value` と同じ要領で ctx を使い、キーワード→%・長さ→px・`calc()` を解決して `"<x> <y>"` に正規化。`apply_initial_values` に `fill` / `50% 50%`、`resolve_non_inherited_css_wide_keywords` に両方を追加 |
| `src/paint/image.rs` | `object_fit_destination(content_box, image_w, image_h, style)` が concrete object size と配置を決めて destination rect を返す。`object_position_offsets` は free space（負値含む）に対して解決する |
| `src/paint/mod.rs` / `src/paint/text.rs` | 両方の img 描画経路が destination を渡し、**clip に content box を追加**する。`cover` が box 外へ出る分はこの clip が crop する |

`draw_image_scaled_clipped` が destination 基準で source を mapping し clip は切り抜きとして働くため、crop 用の追加 primitive は不要だった。`fill` では destination == content box なので既存描画と完全に同一で、acid2 baseline PNG と benchmark fixture は不変。

## Firefox との parity 検証

同一の HTML（14 ケース）を Firefox と omoikane で描画し、PNG から destination rect と 4 分割の色サンプルを抽出して比較した。

**14 ケースすべてで destination rect が一致**。色サンプルも 13 ケースで一致。

唯一の差は 4x2 → 2x1 の縮小ケースで、Firefox は隣接 source pixel を平均した中間色、omoikane は最近傍で拾った原色になる。これは `draw_image_scaled_clipped` の再サンプリング品質の問題で object sizing とは独立なので #265 に切り出した。

## Copilot レビュー対応

指摘 5 件のうち 2 件は実際の挙動バグだった。Firefox 152 で正解を確認して修正した。

| 指摘 | 内容 | 修正 |
|---|---|---|
| CSS-wide keyword | `object-position: inherit` が文法検証で破棄され、親の値を継承しなかった（`object-fit` 側は元から受け付けていたので不整合） | 単独の CSS-wide keyword は文法検証を通し cascade に委ねる。Firefox は親が `10px 20px` のとき `inherit` で `10px 20px` を返す |
| 単位なし `calc()` | `calc(1)` / `calc(1 + 2)` / `calc(0)` が有効扱いされ、単位なし文字列として computed value に入っていた（描画時は中央寄せに退避） | `calc()` の引数に長さか百分率が含まれることを要求する。Firefox もこの 3 つを破棄する |
| doc の取り違え | `background_size` の doc comment が挿入した関数の上に残っていた | 元の位置へ戻した |
| test helper の doc | 100x100 固定と書いてあったが `box_size` を取る | 実際の挙動に合わせて書き直した |
| 保証しない記述 | 「positioned な replaced box は inline fragment を持たない」と書いたが、実際は生成される（#264） | fallback である旨と #264 への参照に書き換えた |

## テスト

- `src/css/style_tests.rs` 7 本: 初期値、`object-fit` キーワード、不正値の破棄と earlier-valid の保護、`object-position` の正規化 21 パターン（Firefox 実測値）、`object-position` の不正値、CSS-wide keyword の解決（`inherit` の継承と `initial` / `unset` / `revert`）、単位なし `calc()` の破棄と単位付き `calc()` の解決
- `src/paint/tests.rs` 11 本: `fill` / `contain`（透明余白含む）/ `cover`（crop 方向と box 外非描画）/ `none` / `scale-down` 3 分岐 / `object-position` の各方向・百分率・px・1 値指定 / inline・block・positioned の 3 経路 / 無指定時の従来描画維持

## 対象外

`aspect-ratio`、video frame decode、SVG preserveAspectRatio 完全準拠、複雑な writing mode。

`object-position` の 3/4 値構文（`left 10px top 20px`）も対象外（issue の「基本形」に従う）。`<video>` / `<canvas>` は現状 img 以外の replaced 描画経路が無いため、helper 共有で将来対応できる形にしてある。

## 検証中に見つけた既存の不具合（別 issue）

| 内容 | issue |
|---|---|
| 絶対配置された replaced element が inline 静的位置と絶対位置の両方で二重描画される（`main` で再現確認済み） | #264 |
| 画像の縮小描画が最近傍サンプリング | #265 |

## 完了条件

PNG pixel assertion と通常全 suite 成功。既存 benchmark fixture の PNG は意図しない変更なし。

GitHub Issue: #246（親 #173）
