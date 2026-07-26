---
number: 089
slug: backdrop-filter-local-region
github: 251
status: closed
priority: high
---

# backdrop-filter を対象領域だけ抽出して処理する（perf）

## 概要

`apply_backdrop_filters` が viewport canvas 全体を clone して filter をかけている処理を、
対象 border box と filter が必要とする padding 分の局所領域だけに限定する。

## 背景

- `apply_backdrop_filters` は `canvas.clone()` で全面複製 → `apply_filters` に渡している
- `apply_filters` は `alpha_bounds` で処理範囲を絞るが、背景が全域 opaque な通常のページでは
  alpha bounds も全画面になるため crop が効かない
- 実際に canvas へ戻すのは border box ∩ inherited clip の領域だけなので、
  それ以外の pixel に対する blur / color filter 計算は完全に無駄
- 1280x720 の viewport に 60x40 の backdrop-filter 要素が 1 つあるだけで
  921,600 pixel を blur していた

## 実装範囲

- 出力 area を border box ∩ inherited clip ∩ canvas bounds から算出する
- filter chain が必要とする**入力** padding を加えた source area だけを局所 Canvas へコピーする
- 局所座標で既存の filter 処理を適用する
- 出力 area だけを元 canvas へコピーバックする
- blur が対象領域の外周 pixel を参照する境界意味論を維持する
- test-only metric で処理 pixel 面積を固定する

## 設計メモ

### 入力 padding は `filter_padding` の鏡像になる

既存の `filter_padding` は「出力がソースの外へどれだけ広がるか」を返す
（`subtree_paint_bounds` / `apply_filters` の crop 拡張に使われる出力側の量）。

backdrop では逆向きの量、つまり「ある出力領域を得るために読む必要のある入力範囲」が必要。

- blur は対称なので `radius` のまま
- drop-shadow は左右・上下が反転する。`apply_drop_shadow` は `sx = x - dx` で読むため、
  `dx > 0`（右へずれる影）は**左側**の入力を必要とする

そのため `filter_source_padding` を別関数として追加する。

### chain 全体の padding は各 filter の和で上界になる

出力 area の pixel は source area の端から padding 以上内側にあるため、
prefix 適用後の「正しい」pixel だけを参照して計算される（数学的帰納法）。
`box_blur` は 1 pass・reach = radius・端はカーネル縮小なので、
端から radius 以上離れた pixel の値は全面計算と一致する。
source area が canvas 境界で clamp される場合は全面計算と同じ境界になるため、こちらも一致する。

## 副次的な修正（Copilot レビュー指摘を含む）

`filter_padding` / `filter_source_padding` は `blur(1e23px)` のような病的な長さで
`usize::MAX` まで飽和するため、`out_x1 + right` が overflow して panic していた
（`filter` 側の `apply_filters` も同様）。加算を `saturating_add` にし、
`box_blur` の半径をキャンバスサイズで clamp した（半径がキャンバスより大きい場合
カーネルは常に全体を覆うため結果は不変）。


border box が canvas の完全に外側にある場合、旧実装は
`x0 = floor(area.x).max(0)` と `x1 = ceil(right).min(width)` で `x0 > x1` となり
スライス範囲が逆転して panic していた（例: viewport 300px で `left: 400px` の要素）。
出力 area を canvas bounds と交差させることで解消する。

## テスト

- `backdrop_filter_processes_only_the_local_region` — 小要素の面積削減（厳密値）
- `backdrop_filter_color_filter_processes_exactly_the_border_box` — padding 0 の色 filter
- `backdrop_filter_blur_samples_pixels_outside_the_element` — blur 境界意味論
- `backdrop_filter_local_region_respects_inherited_clip` — clip
- `backdrop_filter_local_region_clamps_to_canvas_edges` — 画面端 / 負座標
- `backdrop_filter_fully_offscreen_element_is_skipped` — 完全画面外（panic 回帰）
- `backdrop_filter_multiple_elements_each_use_a_local_region` — 複数 backdrop
- `backdrop_filter_drop_shadow_reads_padding_from_the_source_side` — 入力 padding の向き
- `backdrop_filter_canvas_checksum_does_not_regress` — 全 pixel の checksum を固定
- `filter_source_padding_mirrors_the_output_padding` — 出力側 padding との鏡像関係
- `backdrop_filter_saturates_pathological_lengths_to_the_canvas` — `usize` 飽和
- `backdrop_filter_blur_larger_than_the_canvas_averages_the_source` — canvas 超の blur 半径
- PNG 非退行 — 変更前後で blur / color / clip / 画面端を含むページの PNG が完全一致することを確認

## 完了条件

- 全 canvas clone を除去する
- `cargo test` 全 suite と benchmark fixture の determinism test が通る

## 計測

1280x720 の viewport に blur(8px) / blur(12px)+saturate / brightness の
backdrop-filter 要素を 3 つ置いたページ（`render_document` 15 回の最小値）:

| | 処理 pixel 数 | render |
|---|---|---|
| before | 2,764,800 (921,600 x 3) | 58.95 ms |
| after | 58,872 | 40.70 ms |

GitHub Issue: #251（親 #173、関連 #238）
