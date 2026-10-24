# #1323 の作業再開手順

2026-10-24 時点の引き継ぎ（全体テスト・build成功、コミット・CI・マージは未完了）。進捗の正本は [Issue #1323](https://github.com/ieee0824/omoikane/issues/1323) と関連PRであり、この文書の状態は再開時に再確認する。

## 保存時の状態

- #1324〜#1334 は完了済み。最新の #1324 は [PR #1345](https://github.com/ieee0824/omoikane/pull/1345) でmain `94ac9aac1214e4c69fa170dab0598f7679aed474` へマージ済み。
- #1324 の最終検証は全体テスト3,997 PASS / 0 FAIL / 0 ignored、build・コミット後の書式検査成功。PRのCIは46 SUCCESS / 3 SKIPPED / 0 FAIL。この結果を今後の変更の検証として流用しない。
- #1323 は未完了。[上流WPT PR](https://redirect.github.com/web-platform-tests/wpt/pull/63537) は2026-10-20 16:16:35 UTCにマージ済み。正式なマージコミットは `dff7cf45938f6e16427f5607bac82a803534484b`。
- [`tests/wpt/revision.txt`](../../tests/wpt/revision.txt) を正式な上流コミット `dff7cf45938f6e16427f5607bac82a803534484b` へ更新し、[`tests/wpt/manifest.json`](../../tests/wpt/manifest.json) から #1323 の既知失敗登録を削除した。
- main `18fde88e7cc42b05f2842a351872104b5c28f9d6` を統合し、pin更新で検出したCSSOM・色・イベント・XPath・メディアクエリの不一致を修正した。
- 固定IDLからイベントハンドラー一覧を再生成し、`installresult` と `openerchange` を反映した。生成物の一致検査と、全属性の公開・dispatchを含むイベント関連27テストも成功した。下記の全体結果には一覧更新を含む。
- 新しく検査される `column-span: auto` は未対応で、CSSOM・配置の実装を [Issue #1450](https://github.com/ieee0824/omoikane/issues/1450) で追跡する。この1 subtest名だけを期限付き既知失敗として登録し、既存のnone/allや別の失敗は引き続き回帰として検出する。
- `CI=1 WPT_REQUIRED=1 cargo test --locked -- --include-ignored` は5,569成功・失敗0・無視0で終了した。正式WPT全565ケースは555成功・既知失敗10・回帰0・unexpected pass 0。PIは全23 subtest成功、script error 0。
- 色の単体テスト11件、関連する統合テスト92件、`cargo build --locked`、未コミット差分の書式・空白検査も成功した。コミット後の書式検査、CI、PRのready化・マージは残る。

## #1323 の修正内容と確認済みの結果

`dom/nodes/MutationObserver-characterData.html` の旧fixtureはHTMLの `<?processing data?>` をbogus commentとして扱い、初期data `?processing data?` を期待していた。現行HTMLのProcessingInstruction解析ではdataは `data` になるため、oldValueが不一致になる。

上流PRでは `document.createProcessingInstruction("processing", "data")` で本物のProcessingInstructionを明示的に作る。Commentの独立したケースと3回のmutationの検証を維持する。

投稿前の独立検証はFirefoxで元・修正後とも23 PASS、Omoikaneで元22 PASS / 1 FAIL → 修正後23 PASS、双方script error 0。この結果は修正案の検証であり、正式な固定WPT更新後の成功を証明するものではない。

## 更新後の検証方法

更新したpinの正式fixtureは `scripts/fetch-wpt.sh` で取得する。fixtureやアサーションは編集せず、WPT checkoutが `tests/wpt/revision.txt` と一致し、作業ツリーに変更がないことを確認する。

検証はworktree専用targetで容量guardを使う。`WPT_UPSTREAM` の強制指定はランナー自身の合成fixture検査にも伝播するため使わず、各ケースが指定する通常のtransportを利用する。以下のコマンドはworktreeのルートから実行する。

```sh
export CARGO_TARGET_DIR=/target/omoikane-issue1323-resume
export CARGO_BUILD_JOBS=1
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
export WPT_ROOT=/tmp/omoikane-wpt1323-ext4-copy
scripts/fetch-wpt.sh
CI=1 WPT_REQUIRED=1 python3 scripts/cargo-space-guard.py run \
  --required-free-gib 6 --minimum-free-gib 3 --maximum-target-gib 14 -- \
  cargo test --locked -- --include-ignored
python3 scripts/cargo-space-guard.py run \
  --required-free-gib 3 --minimum-free-gib 2 --maximum-target-gib 14 -- \
  cargo build --locked
python3 scripts/check_rustfmt.py
```

最後の書式検査はコミット済みHEADを検査する。最終結果と固定revisionをPRへ記録し、CI・マージを確認してから #1323 を閉じる。#1324〜#1334 の完了状態もGitHubで再確認する。

## この環境に保存した証跡

ローカルの詳細証跡は `/workspace/.artifacts/issues1323-1334/` に保持している。これらはGitに含めず、他環境では上流PRからパッチを取得して再検証できる。

- `progress.md`: 完了事項・未解決事項と検証ログへの索引。
- `1323-upstream-proposal/mutationobserver-real-pi.patch`: 投稿前の修正案。
- `1323-stage251-upstream-state.json`: 上流の未マージ状態の過去の確認結果。
- `1324-stage217-full-test-summary.json`、`1324-stage248-final-ci.json`、`1324-stage248-merged-pr.json`: 完了済み #1324 の証跡。

今回の再開用worktreeは `/tmp/omoikane-issue1323-resume`、ブランチは `docs/issue1323-resume`。#1323の後続実装・検証はこのブランチに追加できる。
