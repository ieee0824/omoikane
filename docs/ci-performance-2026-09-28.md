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

Gate 5 workflow全体は初回31分04秒からwarm rerunの25分09秒へ**5分55秒（約19%）短縮**した。成功jobの実行時間合計は106分15秒から86分15秒。初回には新しいキャッシュキーの保存が含まれる。変更前の別revisionの成功runとの差は、この変更単独の効果とは扱わない。

macOSの残り10件は、復元したfingerprintよりcheckout直後のソースの更新時刻が新しいため再ビルドされた。ソース変更時のキャッシュキー切り替えにはBoaのRustソース、manifest、lockfileを含めた。後続の[Boaソース変更run](https://github.com/ieee0824/omoikane/actions/runs/36427398360)ではBoaキーが切り替わってキャッシュを作り直し、[ホストソースのみの変更run](https://github.com/ieee0824/omoikane/actions/runs/36490103911)ではBoaキーが維持され、3ターゲットで復元された。前者のLinux x86_64 suiteは初回にCDPテスト1件がtimeoutで失敗したが、同一headの失敗job再実行で成功した。

初回runで保存されたsuite用キャッシュの容量は、Linux ARM64がRust 385 MB・Boa 66 MB、Linux x86_64がRust 400 MB・Boa 68 MB、macOS ARM64がRust 298 MB・Boa 58 MB。これはGitHub cache一覧の表示値を十進MBへ丸めたもので、転送量や展開後のディスク使用量ではない。

WPT準備は変更前のGate 5で49〜72秒、専用WPT jobで73〜86秒。PR #1110ではGate 5で3.3〜5.5秒、専用jobで5秒だった。旧新のsparse-checkoutパターン239件は一致し、WPTテストも成功した。

PR #1110の通常CI、Browser behavior、Native JIT、CodeQL、Gate 5、`--include-ignored`、Acid3、WPT、Web API、aggregateは成功した。

## 後続のキャッシュ検証

[PR #1127](https://github.com/ieee0824/omoikane/pull/1127)の調査では、mainから復元されたv2 RustキャッシュがLinux ARM64で約101 MB、Linux x86_64で約106 MBのままfull hitになり、ホストソースだけを変えたrunで約235件を再コンパイルした。これらのキャッシュが作られた原因は確定していない。PR #1127はキーをv3に切り替え、失敗したjobでのキャッシュ保存を止めた。

v3の初回にLinux ARM64 385 MB、macOS ARM64 298 MBのsuite用Rustキャッシュを保存した。x86_64 suiteは長時間完了しなかったため初回runを中止した。同じheadの[再実行](https://github.com/ieee0824/omoikane/actions/runs/36493116020)は2・3回目とも全targetのsuite/packageとaggregateが成功し、2回目にx86_64も401 MBを保存した。

| target | v3 cold suite | v3 warm suite | 次のwarm suite | `Compiling` 件数 |
| --- | ---: | ---: | ---: | ---: |
| Linux ARM64 | 1429.6秒 | 1116.5秒 | 1191.1秒 | 242→10→10 |
| Linux x86_64 | 1085.9秒 | 1317.9秒 | — | 245→10 |
| macOS ARM64 | 1492.6秒 | 1402.8秒 | 1322.8秒 | 242→10→10 |

x86_64のcoldは中止した初回で保存できず、表のcold値は2回目、warm値は3回目。x86_64のwarmでは再コンパイルが減った一方、この2runのsuite時間は232.0秒長くなった。2回目のGate 5全体は26分44秒、3回目は23分35秒で3分09秒短かった。runner負荷の変動も含む観測値として扱う。

## 文書のみの変更判定

ルートの`AGENTS.md`と`README.md`、`docs/`配下のMarkdownだけを軽量経路の対象にする。workflow、スクリプト、fixture、WPT入力の変更、変更範囲が確定できないイベントは通常の検証を実行する。

判定用checkは文書のみの場合も実行する。required checkが未作成のまま残らないよう、重いjobはworkflow全体を除外せずjob単位でskipする。

変更前の`AGENTS.md`のみの[PR #1103](https://github.com/ieee0824/omoikane/pull/1103)では、全workflowの完了まで30分43秒かかった。変更後の文書のみの[PR #1114](https://github.com/ieee0824/omoikane/pull/1114)では3分53秒で、**26分50秒（約87%）短縮**した。各成功jobの実行時間の合計は271分06秒から33秒になった。この合計は課金時間ではなく、各jobの開始・完了時刻の差である。両PRは別revisionで、runnerの待ち時間も異なる。

PR #1114では分類checkが成功し、重いjobはskipされ、全checkが完了してマージできた。続けてpushした際、[旧通常CI run](https://github.com/ieee0824/omoikane/actions/runs/36426192831)はcancelled、[最新run](https://github.com/ieee0824/omoikane/actions/runs/36426348878)はsuccessになった。旧runはrunner割当前に中止されたため、この例の削減runner時間は0秒。
