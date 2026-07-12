---
number: 016-16
slug: iframe-onload-event
parent: 016
status: closed
---

# iframe の load イベント発火

## 目的

iframe のサブ文書ロード完了時に load イベント（`iframe.onload` / addEventListener('load')）を発火させる。

## 背景

- 016-9 で contentDocument（サブ文書の生成・遅延ロード）は実装済みだが、load イベントは未発火
- Acid3 test 65 は iframe onload で `kungFuDeathGrip` を蓄積し、test 69 がそれを検証するため、
  未発火だと test 69 が「kungFuDeathGrip.title was null」で fail する
- 016-4 で整備した load イベント基盤・on* インラインハンドラ配線と連携する

## スコープ

- サブ文書ロード完了（遅延ロードの初回確定時点）での load イベント dispatch
- `iframe.onload` プロパティ / `onload` 属性 / addEventListener の3経路
- document.write で動的生成された iframe でも発火すること（Acid3 の実パターン）

## 受け入れ条件

- Acid3 test 69 の kungFuDeathGrip 検証が前進する（test 65/69 の該当部分が PASS）
- 静的・動的生成の両方の iframe で load が発火するテストを追加

## 追記(2026-07-11, 016-15 実装時)

- `HTMLElement.title` getter は仕様上、属性欠如時に `""` を返すべきだが、load イベント未発火の現状で `""` を返すと Acid3 test 69 が null チェックを通過して無限 retry になり、FAITHFUL 実行が test 69 で停止する(61→41 に退行)。暫定で属性欠如時は null を返している(dom_bootstrap.js の title getter 参照)。本 issue で load イベントを発火させる際に `?? ""` の仕様準拠デフォルトへ戻すこと。

## 実装結果 (2026-07-12)

- iframe と `data` 属性を持つ object が文書へ接続された時点で resource load マクロタスクを予約する。静的 HTML、`appendChild`、`insertBefore`、detached 部分木の一括接続、`document.write` を対象とした。
- iframe はタスク実行時にサブ文書をロードし、その完了後に非 bubbling の `load` を要素へ dispatch する。`onload` プロパティ、`onload` 属性、`addEventListener('load')` の3経路を確認した。object は contentDocument を作らず load dispatch のみ行う。
- detached のままでは発火しない。接続後に発火する。接続済み文書内の移動は重複発火せず、detach 後の再接続は実ブラウザの navigation model に合わせ、サブ文書を再生成して load を再発火する。
- `HTMLObjectElement.data` を content attribute に反射させ、Acid3 test 65 の `object.data = "svg.xml"` も load 対象になるようにした。
- `HTMLElement.title` getter は属性欠如時に `""` を返す仕様準拠の `?? ""` へ復元した。

### Acid3 実測

- FAITHFUL: 83/100、test index 100、99 iterations、drive error なし。
- DIRECT: 83/100、test index 100、100 iterations、drive error なし。direct-drive でも各 update 後に0ms resource task を処理し、ページ本来の10ms timer chain は進めない。
- test 65: iframe×6 + object×1 の全7 load handler が完了し、`kungFuDeathGrip.title.length === 7` へ到達。
- test 69: `title was null` および retry を通過し、後続 assertion の `t was null` で失敗。`svg.xml` の XML/SVG パースは 016-14 の範囲であり、本 issue の到達点を満たす。
