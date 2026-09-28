# CI所要時間の計測（2026-09-28）

関連Issue: [#1104](https://github.com/ieee0824/omoikane/issues/1104)。各workflowは並行して走るため、所要時間は合算しない。

## 変更前の観測

異なるrevisionの成功runを調べた。Gate 5全体は[27分16秒](https://github.com/ieee0824/omoikane/actions/runs/36409158589)と[27分03秒](https://github.com/ieee0824/omoikane/actions/runs/36411262763)、通常CIは[14分13秒](https://github.com/ieee0824/omoikane/actions/runs/36409158043)と[16分40秒](https://github.com/ieee0824/omoikane/actions/runs/36411262643)だった。この4値は後述の同一revisionでのcold/warm比較とは条件が異なる。

## 同一revisionのGate 5

[PR #1110](https://github.com/ieee0824/omoikane/pull/1110) の同一head `a34b618f` で、[Gate 5 run #36419300432](https://github.com/ieee0824/omoikane/actions/runs/36419300432) の初回とrerunを比較した。suite、package、aggregateは全ターゲットで成功した。

| suite target | 初回 | warm rerun | 短縮 | `Compiling` 件数 |
| --- | ---: | ---: | ---: | ---: |
| Linux ARM64 | 1372.8秒 | 1128.8秒 | 244.0秒 | 242→10 |
| Linux x86_64 | 1665.3秒 | 1415.3秒 | 250.0秒 | 245→10 |
| macOS ARM64 | 1505.7秒 | 1224.2秒 | 281.5秒 | 242→10 |

macOSの残り10件は、復元したfingerprintよりcheckout直後のソースの更新時刻が新しいため再ビルドされた。ソース変更時のキャッシュキー切り替えにはBoaのRustソース、manifest、lockfileを含めた。実際にBoaソースを変更した別runでの再ビルド確認は未実施。

WPT準備は変更前のGate 5で49〜72秒、専用WPT jobで73〜86秒。PR #1110ではGate 5で3.3〜5.5秒、専用jobで5秒だった。旧新のsparse-checkoutパターン239件は一致し、WPTテストも成功した。

PR #1110の通常CI、Browser behavior、Native JIT、CodeQL、Gate 5、`--include-ignored`、Acid3、WPT、Web API、aggregateは成功した。文書だけのPRと連続pushの挙動は別に実行して確認する。
