---
number: 031
slug: cursor-transform-origin
parent:
status: closed
---

# cursor / transform-origin の基本対応

## 概要

実サイトで頻出する `cursor` と `transform-origin` プロパティを
is_supported_property に追加し、未対応ログのノイズを削減する。

## 背景

実サイト ページの未対応CSSログ:
- `transform-origin` (36回)
- `cursor` (18回)

ヘッドレスブラウザでは `cursor` は視覚的影響なし。
`transform-origin` は `transform` 未実装のため当面影響なし。

## 対応内容

- `cursor` を is_supported_property に追加（レンダリング影響なし、ログ抑制のみ）
- `transform-origin` を is_supported_property に追加（transform 実装時に再検討）
- `animation-*` 系を is_supported_property に追加（ヘッドレスでは静的スナップショットのみ）

## 受け入れ条件

- 上記プロパティが未対応ログに出なくなる
- 既存テスト全通過

## 関連 issue

- [051 CSS プロパティ値検証と computed style serialization](051-css-property-value-validation.md)
  - 本 issue は supported property 登録まで、051 は `cursor` keyword の妥当性検証・無効宣言破棄・初期値 `auto` の serialization を担当する

## 実装プラン（2026-07-13, Fable）

### 現状整理

- `cursor` は 051（closed 済み）で `is_supported_property` 登録＋キーワード検証・serialization まで実装済み。本 issue の残スコープは `transform-origin` と `animation-*` 系のみ。
- `is_supported_property`（`src/css/style.rs:1402`）を参照するのは `log_unsupported_css_if_enabled`（同 `:1134`）だけであり、純粋なログ抑制 allowlist。登録してもカスケード・レイアウトへの影響はない（宣言は従来どおり computed 値として properties に入る）。
- `animation` shorthand は `expand_animation_shorthand`（`src/css/shorthand.rs:1414`）が `animation-name` / `animation-fill-mode` / `animation-duration` に展開しつつ、未展開プロパティ保全のため元の `animation` 宣言も再 emit する。このため allowlist 未登録の `animation`・`animation-duration` は shorthand 使用ページで必ず未対応ログに乗る。

### 実装方針

`src/css/style.rs` の `is_supported_property` に以下を追加する:

- `transform-origin` — レンダリング影響なし、ログ抑制のみ（transform 実装時に値解釈を再検討）
- `animation` — shorthand 再 emit 分の抑制
- `animation-delay` / `animation-direction` / `animation-duration` / `animation-iteration-count` / `animation-play-state` / `animation-timing-function` — classic longhand のうち未登録の6つ（`animation-fill-mode` / `animation-name` は登録済み）

`animation-composition` / `animation-timeline` 等の新しい longhand は実サイトログに現れた時点で追加する（今回は見送り）。

### テスト計画

- `src/css/style_tests.rs` の `identifies_supported_property_names` を拡張し、追加した全プロパティ名を明示的に assert する（既存の grid 系テストと同じ列挙ループパターン）。負例 `filter` の assert は維持。
- `animation: fade 0.3s forwards` を解決した際に shorthand 展開で候補となる宣言名（`animation` / `animation-name` / `animation-fill-mode` / `animation-duration`）がすべて supported であることを確認する。

### 回帰リスク

- allowlist はログ抑制のみでカスケード挙動に影響しないため、リスクはタイポ登録程度。テストの明示列挙で担保する。

## クローズ記録（2026-07-13、PR #139）

- `cursor` は 051（closed 済み）で登録・キーワード検証まで実装済みだったため、本 issue の残スコープとして
  `transform-origin` と animation 系 8 エントリ（shorthand `animation` + 未登録 classic longhand 6 つ）を
  `is_supported_property` に追加。allowlist はログ抑制専用でカスケード・レイアウトへの影響なし。
- テストは `identifies_supported_property_names` を拡張（追加 8 プロパティの明示 assert +
  `animation` shorthand 展開候補一式の supported 確認）。`cargo test -j1` 全通過（lib 1121 passed）。
- Copilot レビュー指摘 1 件（`align-*` 系と `animation-*` 系の分断）を並び替えで解消済み。
- `animation-composition` / `animation-timeline` 等の新 longhand は実サイトログに現れた時点で追加する。
- 実装は Opus（general-purpose, model: opus）。
