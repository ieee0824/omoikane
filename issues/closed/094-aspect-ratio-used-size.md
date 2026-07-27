---
number: 094
slug: aspect-ratio-used-size
github: 247
status: closed
priority: high
---

# replaced element の aspect-ratio used-size core を実装する

## 概要

`aspect-ratio` を computed style と replaced element の used size へ接続する。

## Firefox 152 実測

### computed value（25 パターン）

| 入力 | Firefox |
|---|---|
| 初期値 / `auto` | `auto` |
| `1/1` / `2 / 1` / `2` / `0.5` | `1 / 1` / `2 / 1` / `2 / 1` / `0.5 / 1` |
| `auto 2/1` / `2/1 auto` / `auto 2` | すべて `auto 2 / 1`（`auto` を先に正規化） |
| `0/1` / `1/0` / `0/0` | そのまま（退化 ratio も computed value には残る） |
| 負値 / `1 1` / `a/b` / `1/` / `auto auto` | 無効 → `auto` |
| `calc(1) / calc(2)` | `1 / 2` |

パーサの表現は `1/1` が `Keyword("1/1")`、`2 / 1` が `List([Number, Keyword("/"), Number])` と
揺れるため、両形を token 列へ平坦化してから文法を検査する。

### used size（intrinsic 4x2 の画像、31 ケース）

| ケース | Firefox |
|---|---|
| `width:100px; aspect-ratio:1/1` | 100x100（author ratio が intrinsic を上書き） |
| 両 auto + `aspect-ratio:1/1` | 4x4（**intrinsic 幅**を基準に高さを ratio で導出） |
| 両指定 + ratio | 指定どおり（ratio は無視） |
| `aspect-ratio:auto 1/1` | intrinsic ratio が優先 |
| `aspect-ratio:0/1` / `1/0` / `0/0` | 退化 ratio は無視して intrinsic ratio へ |
| `width:100px; max-height:20px` | **100x20**（指定済みの幅は再スケールしない） |
| 両 auto + `max-width:2px` | 2x1（両方 derived なので ratio 維持） |

**min/max の規則は既存バグだった**: omoikane は指定済みの軸も再スケールしていた
（`width:100px; max-height:20px` → 40x20）。CSS 2.1 §10.4 の制約違反表どおり、
**clamp される軸の相手が auto のときだけ**相手を再導出するのが正しい。
この挙動を固定していた既存テストは無かった。

## 実装

| 層 | 内容 |
|---|---|
| `src/css/style.rs` | `is_supported_property` に追加。`validate_declaration` で `auto \|\| <ratio>` の文法検査（負値・余剰成分・length を含む `calc()` は Invalid）。`compute_value` の `aspect-ratio` 分岐が ctx を使って `auto` / `W / H` / `auto W / H` へ正規化（`calc()` も解決）。初期値 `auto` と css-wide keyword の解決を追加 |
| `src/layout/inline.rs` | `preferred_aspect_ratio()` が computed value と intrinsic ratio から使用する ratio を決める（`auto` 付きは intrinsic 優先、退化 ratio は intrinsic へ fallback）。`resolve_image_rendered_size` は片方 auto なら ratio で導出、両 auto なら intrinsic 幅を基準に高さを導出し、min/max では **derived な軸だけ**を再スケールする |

## Firefox との parity

同一の 31 ケースを両エンジンで測定し、**28 ケースが完全一致**。
残る 3 件はすべて `box-sizing` 未適用に起因するもので #268 に切り出した。

## テスト

- `src/css/style_tests.rs` 3 本: computed value 15 パターン（Firefox 実測表）、無効値 13 パターンの破棄と earlier-valid の保護、css-wide keyword（`inherit` / `initial` / `unset` / `revert`）
- `src/layout/tests.rs` 7 本: author ratio の上書き（HTML 属性含む）、両 auto での導出、両指定と退化 ratio の無視、`auto <ratio>` の intrinsic 優先、**min/max が derived な軸だけを再スケールすること**、ratio 無しの回帰、`box-sizing` の現状 pin

共有ヘルパ `object_keyword` は 2 つの機能で使うようになったので `image_computed_keyword` へ改名した。

## 対象外

non-replaced block / flex / grid item への完全適用、object-fit との相互作用、writing mode。

## 検証中に見つけた既存の不具合（別 issue）

| 内容 | issue |
|---|---|
| replaced element の sizing に `box-sizing` が未適用（Firefox 100x60 / omoikane 120x70） | #268 |
| inline replaced element の `getBoundingClientRect()` が 0x0（inline img は LayoutBox を持たない） | #269 |

## 完了条件

既存 img intrinsic sizing を壊さず、通常全 suite 成功。

GitHub Issue: #247（親 #173）
