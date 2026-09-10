# Gate 6: ブラウザ動作の基準

対象は #515 の子Issue #549/#550/#551。性能チューニングの前に、現在のブラウザを
通常実行で操作し、見つかった不具合を修正する。Boa forkの独立化後と性能変更後にも
同じ操作・互換性検証を要求する。

## 修正前の基準

- Omoikane: `406c072daa1e18436c4a84a97548c1ad77425a4d`
- Boa fork: `ee98fdacffb38093d9d220c2ac21a4ed6839ce37`
- production default（インタプリタ）、WPTは `tests/wpt/revision.txt` の固定revision。
- このrevisionで既存suiteは ignoredを含め2341 passed / 0 failed / 0 ignored、build成功。
  追加した最初の操作4ケースは3件失敗した。既存suite成功は操作の正しさの代替にしない。

記録: [#549](https://github.com/ieee0824/omoikane/issues/549)、
[#550](https://github.com/ieee0824/omoikane/issues/550)。これらの値は当該revisionの
結果であり、将来のcheckoutの保証や未実施検証の代用には使わない。

## 見つかった不具合と回帰契約

| 不具合 | 修正後に求める動作 | 回帰契約 |
| --- | --- | --- |
| positioned祖先のないabsolute要素がstatic祖先を基準にする | viewportの初期包含ブロックを使う。positioned祖先とauto insetの既存動作も保持 | `absolute_without_positioned_ancestor_uses_the_initial_viewport` と既存positioningテスト |
| 外部CSSを描画だけに適用し、入力座標と一致しない | documentが保持するauthor cascadeとlayoutをCSSOM、入力、フレーム、session screenshotで共有 | `browser_journeys` の入力/fetch/描画・stylesheetケース |
| タブごとに別localStorageを生成する | 同じbrowser・originで共有。sessionStorage、別origin、別browserは分離。tab close時にsessionStorageを解放 | `tabs_share_local_storage_but_keep_session_storage_separate` とstorage単体テスト |
| absolute/blockの入力欄が値を表示しない | live valueとselectionをフォーム部品の描画情報へ渡す。CSSのbox sizingを維持 | `blockified_text_controls_keep_live_values_in_their_border_boxes` と入力欄内部の画像検査 |
| 通常フローのフォーム部品の座標が0になる | inline imageと同様にFormControl/IconFormControlの描画情報を集計する。変形・非表示とoffset/client寸法も照合 | `inline_form_controls_expose_painted_border_boxes_and_invalidate_when_hidden` と通常フローのフォームPOST操作 |
| `Node.parentElement` が未実装で親要素を使うスクリプトが止まる | 親がElementなら同じwrapperを返し、Document・DocumentFragment・ShadowRootや親なしならnullを返す | `parent_element_tracks_reparenting_without_crossing_document_or_shadow_roots` とフォームの祖先transform検証 |

外部CSSは文書単位で読み込んだ応答を保持し、DOM変更や再描画のたびには取得し直さない。
URL変更は新しいresourceを読み込み、文書の再読み込みでは新しいruntime/cacheを作る。
読み込みは現在の同期APIに合わせたlazy loadであり、非同期stylesheetロードスケジューラの
実装を完了したとは扱わない。inline/adopted styleと外部styleのCSP制限は維持する。

## 再検証

操作と画像の取得手順は [tests/README.md](../../tests/README.md#browser-operation-journeys)。
通常実行とbaseline-jitの両方でAcid3の両drive modeを100/100にする。ブラウザ操作は
Linux x86_64、Linux ARM64、macOS ARM64のCIで実行し、ログ・JSON・PNG・Cargo.lock・
対象revision・toolchainをartifactへ保存する。macOS x86_64は対象外。

2026-09-10の本修正のローカル検証（Linux x86_64、production default）では、
全suiteが2353 passed / 0 failed / 0 ignored、操作7ケース成功、Acid3両方式100/100、
WPTは69ケース中66 pass・3 improvement・regression 0、Web APIは95/97 supported・
error 0・regression 0だった。`cargo build --locked` と
`cargo check --locked --features gui` も成功し、入力・送信・履歴の実画像を目視確認した。
3環境のCIと移行後の再検証は、対応PRの実行結果で別途確認する。

最終判定には全suite（ignored含む）、cargo build、gui feature check、WPT regression 0、
Web API surface regression 0、native JIT契約、3環境配布の検証を合わせる。
限定fixtureの合格だけでは #515 または子Issueを閉じない。

## 独立化と性能改善への引き継ぎ

#546/#551で依存・実装・テストの棚卸しを残し、方式決定後に #552 で独立管理を実装する。
既存parser・VM・built-ins・GC・JIT・bootstrapを継承する。性能比較は同じページ・入力・
viewport・実行モード・ビルド条件で測り、操作結果と画像の正しさを確認してから採用する。
独立化と性能改善の完了証拠は、このブラウザ修正の検証とは別に #553 へ記録する。
