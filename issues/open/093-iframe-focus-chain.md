---
number: 093
slug: iframe-focus-chain
github: 254
status: open
priority: high
---

# iframe 内の focus を親 document の focus chain へ伝播する

## 概要

iframe 内の要素を focus したとき、親 document の `activeElement` をその iframe 要素にし、
browsing context の切り替えに伴うイベントを発火する。

## 背景

#243（PR #257）では Document 単位の focus 状態だけを実装し、chain の伝播は
「state だけ真似ると chain 各エントリへの event 発火と不整合になる」として対象外にしていた。
その結果、iframe 内を focus しても親の `activeElement` は自分が持っていた要素のままだった。

## Firefox 152 実測

Marionette で 3 階層（top → iframe `f` → 入れ子 iframe `g`）を作り、
window / document / element の全 12 target に capture リスナーを張って測定した。

`a`（top）→ `x`（`f` の document 内）へ移したときのイベント列:

```
blur(a)  focusout(a)      ← 旧要素（relatedTarget は null）
blur(target=doc)          ← 旧 innermost document
blur(target=win)          ←   とその Window
focus(target=sub)         ← 新 innermost document
focus(target=subwin)      ←   とその Window
focus(x) focusin(x)       ← 新要素
```

判明した規則:

| 項目 | 挙動 |
|---|---|
| 親 document の `activeElement` | iframe 要素になる（3 階層では sub が入れ子 iframe `g` を指す） |
| iframe 要素自身へのイベント | **発火しない** |
| Document / Window イベント | innermost focused document が変わったときだけ、旧側に `blur`、新側に `focus`。`focusin` / `focusout` の対はない |
| chain の共通部分 | 1 階層深く入っても top document は blur されない（変わった部分だけ発火） |
| relatedTarget | document を跨ぐ移動では `null`、同一 document 内では従来どおり相手要素 |
| chain から抜けたとき | 抜けた document の `activeElement` は body に戻る |
| iframe 内で `blur()` | 親の `activeElement` は iframe 要素のまま、sub の `hasFocus()` も true のまま |

## 実装

すべて `src/js/dom_bootstrap.js`。

- `documentChain(doc)` を追加。document とその祖先を innermost first で返し、各エントリに
  その document を持つ iframe 要素を添える。top へ到達できない場合（browsing context を
  持たない document）は `null`。`focusChainDocuments()` はこれを使う形に整理した
- `focus()` を chain 対応に書き換え:
  1. focusable でなければ return、browsing context が無ければ return
  2. 旧 innermost document の focused element を消してから `blur` → `focusout`
     （relatedTarget は document を跨ぐとき `null`）
  3. document が変わるときは、旧 chain にあって新 chain に無い document の focused element を消し、
     `blur` を旧 document → 旧 Window、`focus` を新 document → 新 Window へ発火
  4. 対象 document の focused element を新要素にし、**祖先 document には子 document を持つ
     iframe 要素を設定**、innermost を更新
  5. 新要素へ `focus` → `focusin`
- `blur()` と `hasFocus()` は変更なし（Firefox 実測と既に一致していた）

`contentWindow` facade が `addEventListener` / `dispatchEvent` を持っているため、
sub-window へのイベント発火は追加の native 実装なしで動く。
Document を target にしたイベントは event path に `defaultView` が入るので、
window の capture リスナーにも Firefox と同じ順序で届く。

## テスト

`src/js/mod.rs` に 6 本追加。イベント列のテストは **Firefox のログをそのまま期待値として貼り付け**、
18 エントリ単位で一致を固定している。

- `focusing_inside_an_iframe_points_each_ancestor_at_its_frame` — 3 階層の `activeElement` と `hasFocus`
- `crossing_documents_blurs_the_old_browsing_context_and_focuses_the_new_one` — document を跨ぐイベント列
- `returning_from_an_iframe_blurs_only_the_innermost_document` — 深く入るときと出るときの両方
- `same_document_focus_moves_do_not_touch_the_browsing_context` — 回帰防止（document/window イベントを出さない、relatedTarget は維持）
- `blurring_inside_an_iframe_keeps_the_frame_focused`
- `removing_a_focused_iframe_hands_focus_back_to_the_top_document`

既存の `active_element_and_has_focus_are_isolated_per_iframe_document` は
「親は自分の要素を指したまま」という #243 時点の挙動を固定していたので、新しい挙動へ更新した。

`tests/web_api_surface/manifest.json` に `dom.focus-chain` を追加（dom area 12/12 supported）。

## 対象外

Tab 順序、platform keyboard 入力、`:focus-visible`。

iframe 削除時のイベント発火も対象外。Firefox は削除された要素へ blur / focusout と
document / window イベントを飛ばすが、omoikane は既存の focus fixup rule（同期版・無言）に
従って state だけ復帰させる。最終状態は Firefox と一致する（top の `activeElement` は body、
`hasFocus()` は true）。scroll event の非同期化（#259）と同じ「rendering opportunity で
まとめて発火する」仕組みが入ったときに揃えるのが自然。

GitHub Issue: #254（親 #173、前提 #243）
