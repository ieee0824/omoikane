---
number: 047
slug: inline-style-cascade
parent:
status: closed
---

# インライン style 属性のカスケード・レイアウト適用

## 概要

`style="..."` 属性をカスケード（specificity 最高位の宣言）としてスタイル計算・レイアウト・描画に適用する。

## 背景（PR #105 レビューでの発見）

- エンジンのカスケード/レイアウトは現状 `<style>`/`<link>` ルールと presentational HTML 属性のみを適用し、
  **インライン style 属性を一切見ていない**
- 一方 016-8 の getComputedStyle は JS 側 `__parseInlineStyle` でインライン値をカスケード結果に上書きマージするため、
  「getComputedStyle だけがインラインを反映する」観測可能な不整合が生じている
  - 例: `<div style="width:100px">` で `getComputedStyle(el).width === "100px"` だが `el.offsetWidth` はインライン無視
- さらに `__parseInlineStyle` は `;` の素朴 split のため `url(data:...;base64,...)` を含む値を誤パースする
- インライン値は生文字列（`"blue"` / `"1em"`）のまま返り、px/rgb への computed value 解決もされない

## 対応内容

- style 属性のパースを Rust 側カスケードに統合（author style より高い specificity、!important 対応）
- レイアウト・描画がインラインスタイルを反映する
- getComputedStyle は JS 側マージを廃止し、カスケード結果（解決済み computed value）を一元的に返す
- `;` split の誤パース解消（カスケード統合により JS 側パーサ自体を削除できる想定）

## 優先度

中〜高 — 実サイトでのインライン style 使用率は極めて高く、レンダリング品質に直結する。

## 受け入れ条件

- インライン style がレイアウト結果（offsetWidth 等）と getComputedStyle の両方に一貫して反映される
- `url(data:...;base64,...)` を含む style 属性が正しくパースされるテストを追加

## 設計（2026-07-13 承認済み）

調査の結果、カスケードの挿入点は `StyleResolver::compute_style_with_pseudo`（`src/css/style.rs:264`）の
1箇所で、paint・レイアウトメトリクス（offsetWidth）・getComputedStyle の全消費者に効く。
既存の `<iframe>` 限定インライン style ハック（`style.rs:295-324`）を全要素に一般化する方針。

### Phase 1: CSS パーサ基盤（`src/css/parser.rs`）

- `split_important` のトークン破壊バグ修正（`margin: 1px 2px !important` が `"1px2px"` に化ける。
  元トークン列を `!` 位置で切り詰める方式に）
- `parse_declaration` の値収集ループに括弧深度追跡を追加（`;` は深度0でのみ終端）
  → 引用符なし `url(data:...;base64,...)` が通る
- 公開 API `parse_style_attribute(&str) -> Vec<Declaration>` を新設。
  CSS Style Attributes 仕様の forgiving パース（EOF 終端、不正宣言は宣言単位でスキップ、連続 `;` 許容）。
  既存 `parse_declaration` を再利用し shorthand 展開・!important・名前正規化を継承する

### Phase 2: カスケード統合（`src/css/style.rs`, `src/dom/mod.rs`）

- `NodeHandle::get_attribute(name)` を追加（全属性クローンの `attributes()` を避ける）
- iframe 限定ブロックを全要素に一般化（`pseudo.is_none()` かつ Element のみ）
- `Candidate` に `inline: bool` を追加し、ソートキーを（cascade_rank, inline, specificity,
  source_order）に拡張。CSS 2.1 §6.4.3 の「author !important > インライン通常 > author 通常」
  「インライン !important > author !important」を明示的に実現する
- resolver 単体テスト＋レイアウトテスト（`style="width:120px"` → `content.width == 120.0`）

### Phase 3: JS 側の一元化（`src/js/dom_bootstrap.js`, `src/js/mod.rs`）

- `__parseInlineStyle` と getComputedStyle 内のマージを削除
  （contentWindow ファサードが関数参照を捕捉するため in-place 編集）
- 孤児化する `validate_inline_css_native`＋登録、`validate_inline_declaration` 系を削除。
  既存テスト3件（bogus cursor→`auto`、`MOVE`→`move`、inline が cascade に勝つ）は
  期待値を維持したまま新機構に書き換える
- E2E テスト新設: `style="width:100px"` で `offsetWidth === 100` かつ
  `getComputedStyle().width === "100px"`、setAttribute 後の再レイアウト、data-URI 入り style 属性

### Phase 4: 検証

- `cargo test`（Acid2 ピクセル基準含む）＋ `cargo build`、
  `cargo run --example acid3` を前後で実行し 100/100 維持を PR に記録
  （Acid3 test 46 は iframe の style 属性リサイズに依存、test 45 は `style.cssFloat` の
  CSSOM 宣言値読み — el.style は生宣言値のまま維持すること）

### スコープ外

- `el.style`（CSSOM）の `parseDecls` の `;` 分割バグは [067](067-element-style-cssom-parser-robustness.md) に切り出し
- SVG のプレゼンテーション属性は StyleResolver を通らないため対象外

## クローズ記録（2026-07-14、PR #140）

- style 属性を Rust 側カスケードに統合。`Candidate` の `inline` フラグ + ソートキー拡張
  `(cascade_rank, inline, specificity, source_order)` で CSS 2.1 §6.4.3 の優先順位を実現。
  iframe 限定の偽ルールハックは一般実装で置換
- forgiving 宣言リスト API `parse_style_attribute` を新設。過程で既存バグ2件も修正:
  `split_important` の複数トークン値破壊（`margin: 1px 2px !important` → `"1px2px"`）、
  引用符なし `url(data:...;base64,...)` 内 `;` での値切り詰め
- getComputedStyle の JS 側マージ（`__parseInlineStyle`）を削除しカスケードに一元化。
  受け入れ条件（offsetWidth / getComputedStyle の一貫反映、data-URI パース）は
  パーサ・resolver・E2E の3層のテストで担保
- `cargo test --lib` 1148 passed / 0 failed（ベースライン 1121 から +27）。
  Acid2 ピクセル基準維持、Acid3 100/100（FAITHFUL / DIRECT 両モード）を実測確認
- codex + critical-reviewer の並列レビューを実施（High 指摘ゼロ）。修正対応のほか派生 issue を起票:
  [067](../open/067-element-style-cssom-parser-robustness.md)（el.style CSSOM パーサの同種バグ）、
  [068](../open/068-cascade-important-origin-order.md)（既存の cascade important 段位・animation 後処理の仕様逸脱）
- Copilot レビュー指摘 1 件（`get_attribute` の常時小文字化が `set_xml_attribute` と非互換）を
  完全一致優先・小文字化フォールバックで修正
- detached 要素の getComputedStyle は空を返す挙動に変更（実ブラウザ同等、テストで pin）
- 設計は Fable（6領域並列調査 + 設計）、実装・レビュー修正は Codex（gpt-5.6-sol）
