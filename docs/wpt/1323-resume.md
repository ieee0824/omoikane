# #1323 の作業再開手順

2026-10-13 時点の引き継ぎ。進捗の正本は [Issue #1323](https://github.com/ieee0824/omoikane/issues/1323) と関連PRであり、この文書の状態は再開時に再確認する。

## 保存時の状態

- #1324〜#1334 は完了済み。最新の #1324 は [PR #1345](https://github.com/ieee0824/omoikane/pull/1345) でmain `94ac9aac1214e4c69fa170dab0598f7679aed474` へマージ済み。
- #1324 の最終検証は全体テスト3,997 PASS / 0 FAIL / 0 ignored、build・コミット後の書式検査成功。PRのCIは46 SUCCESS / 3 SKIPPED / 0 FAIL。この結果を今後の変更の検証として流用しない。
- #1323 は未完了。[上流WPT PR](https://redirect.github.com/web-platform-tests/wpt/pull/63537) は承認済みだが、2026-10-13の確認ではOPEN・未マージ。修正コミットは `f86b84f6033681e84bb9a4d666f3c3592b1a2823`。上流のマージ権限は持っていない。
- 固定WPTは [`tests/wpt/revision.txt`](../../tests/wpt/revision.txt) の `dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`。[`tests/wpt/manifest.json`](../../tests/wpt/manifest.json) は `characterData ProcessingInstruction: data mutations` の1件のみを既知失敗として登録している（期限2026-11-08）。
- この保存コミットは引き継ぎ文書のみ。固定revision・fixture・期待値・既知失敗の登録はまだ更新していない。

## #1323 の修正内容と確認済みの結果

`dom/nodes/MutationObserver-characterData.html` の旧fixtureはHTMLの `<?processing data?>` をbogus commentとして扱い、初期data `?processing data?` を期待していた。現行HTMLのProcessingInstruction解析ではdataは `data` になるため、oldValueが不一致になる。

上流PRでは `document.createProcessingInstruction("processing", "data")` で本物のProcessingInstructionを明示的に作る。Commentの独立したケースと3回のmutationの検証を維持する。

投稿前の独立検証はFirefoxで元・修正後とも23 PASS、Omoikaneで元22 PASS / 1 FAIL → 修正後23 PASS、双方script error 0。この結果は修正案の検証であり、正式な固定WPT更新後の成功を証明するものではない。

## 再開時の手順

1. `ctx-sync agent start` で共有状態を読み、最新main・作業ツリー・Issue・PRを確認する。別worktreeと未コミット変更を保持し、このdraftのブランチを最新mainへ統合する。
2. 上流PRの `state`、`mergedAt`、`mergeCommit` を取得する。未マージなら、このIssueの完了条件である正式な上流revisionの更新はまだ行えない。上流へ追加コメントを投稿する場合はユーザーの指示を確認する。
3. マージ済みなら、修正を含む上流revisionと対象ファイルを確認して `tests/wpt/revision.txt` を更新する。PRの未マージheadを正式な上流revisionとして扱わない。
4. `scripts/fetch-wpt.sh` で新しい固定revisionを取得する。元のファイルや期待値を手作業で変更せず、対象fixtureが上流から取得されたものと一致することを確認する。pin更新による他のfixture/helperの変更も確認する。
5. manifestの対象ケースから #1323 の `known_failure` オブジェクトを削除する。他の既知失敗や期待値は維持する。
6. 対象ケースの全23 subtest、WPT smoke全体、locked全体テストとbuildを実行し、結果と固定revisionを保存する。pin更新で新たな失敗が出た場合は原因を調べ、成功と報告しない。
7. 変更をコミットし、コミット済み差分のレビュー・書式検査とCIを完了して、このdraftをreadyへ変更する。ユーザーが依頼済みのマージまで進め、検証結果を #1323 に記録してから閉じる。
8. #1323〜#1334 の最新状態を再監査し、全範囲の完了を確認する。

状態確認:

```sh
gh pr view 63537 --repo web-platform-tests/wpt \
  --json state,mergedAt,mergeCommit,reviewDecision,mergeStateStatus
gh issue view 1323 --repo ieee0824/omoikane --json state,body,comments
```

検証はworktree専用targetで容量guardを使用する。下記は `/tmp/omoikane-issue1323-resume` から実行する例。容量不足ならREADMEの容量guard手順に従い、別作業者のtarget・証跡は削除しない。

```sh
export CARGO_TARGET_DIR=/target/omoikane-issue1323-resume
export CARGO_BUILD_JOBS=1
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
export WPT_ROOT="$CARGO_TARGET_DIR/wpt"
scripts/fetch-wpt.sh
WPT_REQUIRED=1 python3 scripts/cargo-space-guard.py run \
  --required-free-gib 6 --minimum-free-gib 2 --maximum-target-gib 14 -- \
  cargo test --locked --test wpt_smoke -- --nocapture
CI=1 python3 scripts/cargo-space-guard.py run \
  --required-free-gib 6 --minimum-free-gib 2 --maximum-target-gib 14 -- \
  cargo test --locked -- --include-ignored
python3 scripts/cargo-space-guard.py run \
  --required-free-gib 3 --minimum-free-gib 2 --maximum-target-gib 14 -- \
  cargo build --locked
python3 scripts/check_rustfmt.py
```

## この環境に保存した証跡

ローカルの詳細証跡は `/workspace/.artifacts/issues1323-1334/` に保持している。これらはGitに含めず、他環境では上流PRからパッチを取得して再検証できる。

- `progress.md`: 完了事項・未解決事項と検証ログへの索引。
- `1323-upstream-proposal/mutationobserver-real-pi.patch`: 投稿前の修正案。
- `1323-stage251-upstream-state.json`: 上流の未マージ状態の過去の確認結果。
- `1324-stage217-full-test-summary.json`、`1324-stage248-final-ci.json`、`1324-stage248-merged-pr.json`: 完了済み #1324 の証跡。

今回の再開用worktreeは `/tmp/omoikane-issue1323-resume`、ブランチは `docs/issue1323-resume`。#1323の後続実装・検証はこのブランチに追加できる。
