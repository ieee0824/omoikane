# Primitive stringのlength参照（#659）

`GetPropertyByName` のprimitive処理を `primitive_named_lookup_target` へ分け、
primitive stringの `length` は既存のUTF-16長から返す。プロパティ名を数値キーなどへ
分類する処理と、解決に不要なreceiverのコピーを省く。専用helperはinline展開せず、
objectのinline-cacheを含むopcode本体へprimitive専用の分岐を増やさない。
他のキー、boxed string、getter、Proxyには既存の検索を使う。

GCのroot/ephemeron/remembered-set、JITの生成コードと実行契約、公開API、依存lockは
変更しない。反復回数や計算量の次数を変える最適化ではない。

## 旧fixtureの再現と原因の絞り込み

[GCポインタ集合変更の記録](gate6-gc-performance.md)を保持する。
2026-09-11に当時の保存バイナリをSHA256で照合し、同じfixture v1・body・回数・
4 pass中最小値でJIT有効の10組を再測定した。回数は実行前に固定し、前後の順序を交互にした。

| workload | 今回の旧バイナリ比較 | 遅い組 |
| --- | ---: | ---: |
| primitive-string-property | +6.25% | 7/10 |
| string-concat | +0.41% | 6/10 |

元の+4.87%・+2.58%という観測も残す。旧v1の配列結果には既知の不備があるため、
これは歴史的な性能の再現記録である。現行の正しさと改善前後は
[fixture v2](benchmark-v2.md)で確認し、新旧fixtureを跨いだ比率は算出しない。

同じworktree・compiler・lock・profileでGC集合の型だけをstdへ戻した対照では、
旧bodyを20,000回実行した総命令数の差は両項目とも0.04%未満だった。
0/5,000/20,000回でGC関連の命令数は反復に比例せず、旧バイナリの
`primitive_property_lookup` も両者263命令だった。GCの走査や反復計算量の増加は、
この結果からは時間差を説明できない。CPUの実イベントは取得しておらず、配置やcacheなどの
どれが歴史的な時間差の直接原因かは未確定である。今回の改善を、その証明とは扱わない。

## 不採用にした候補

JIT helper cacheで `contains_key` と `get_mut` の二重検索を減らし、既存hashbrownを
使う案では、文字列参照・連結の命令/反復が3.07%・2.76%減った。しかし、通常実行の
JIT有効5組ではそれぞれ+3.00%・+2.52%遅くなったため戻した。命令数削減だけでは採用しない。
JIT無効・有効、全11項目と代表ページの全100サンプル、patch、バイナリhashを保持する。

次にopcode本体へlengthの早期returnを追加した案では、初回5組で対象項目は短縮したが、
JIT無効の全体+1.75%、文字列method+10.44%などの増加があった。追加10組とページ各20組を
開始前に固定して継続性を確認した。合算は各組の比率から求め、初回のサンプルを除外しない。

| 指標 | 無効・初回5組 | 無効・追加10組 | 無効・全15組 | 有効・初回5組 | 有効・追加10組 | 有効・全15組 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| primitive-string-property | -3.90% | -4.06% | -3.97% | -4.36% | -4.73% | -4.52% |
| string-concat | -3.25% | -3.94% | -3.90% | -3.02% | -4.62% | -4.06% |
| primitive-string-method | +10.44% | -0.05% | +0.57% | -1.64% | -1.14% | -1.29% |
| arith | +1.97% | +1.21% | +1.71% | -1.10% | -0.21% | -0.71% |
| 全11項目のプロセス | +1.75% | +0.67% | +0.99% | +0.72% | -0.68% | -0.59% |

代表ページ各40組は無効-1.84%、有効-0.69%。全220サンプルの計算結果・診断・描画は一致した。
無効の全体+0.99%（10/15組遅い）が残ったため、この配置のままは採用せず専用helperへ分けた。
命令の配置がこの増加の原因だと断定するものではない。

## 専用helper候補の命令数

文字列2項目は反復ごとにprimitive stringの `length` を読む。診断driverは旧bodyを
1回呼び、0/5,000/20,000回の結果を独立した式で検査する。表は5,000回から20,000回への
命令数増分を15,000で割った値である。

| workload | 基準の命令/反復 | helper候補の命令/反復 | 変化 |
| --- | ---: | ---: | ---: |
| primitive-string-property | 5248.35 | 5069.01 | -3.42% |
| string-concat | 5759.25 | 5571.45 | -3.26% |

Callgrindのself命令数と総命令数を使う。生成trampolineを含むinclusive call treeは復帰の
追跡が不正確で全体を超える数を出したため、帰属の根拠に使わない。cache/branch simulationも
ホストCPUの実測ではなく、instrumented実時間は速度に使わない。長いtimeoutは診断driverだけに
設定し、通常のテストや性能比較の制限は変更しない。

## 専用helper候補の通常実行

基準は `57fb64f15518cf51dfae9afd2f1719d297c2bf3c`。同じworktreeで前後のバイナリを保存し、
Linux aarch64 / Rust 1.98.0 / 既存test profile（Omoikane opt-level 1、依存2、
debug assertions・overflow checks有効）で比較する。releaseや別CPUの速度の保証ではない。

両者ともfixture v2（SHA256 `177eb80ae73d1081eb620f747b4703e2a9f2731e0255bb60131b73b7dcac6464`）。
有効・無効を別にビルドし、各mode10組を開始前に固定した。全11 bodyと回数、4 pass中最小値を
保持する。各サンプルは新しいプロセス/runtime、各passの計算結果を検査する。
前後の順序は交互、通常実行の計測中はビルドやinstrumented実行を重ねない。

表は各組の候補/基準から求めた変化率の中央値で、負は短縮。履歴baselineや別エンジンの
参照値は使わない。不採用候補との比率も混ぜない。

| workload | JIT無効10組 | 遅い組 | JIT有効10組 | 遅い組 |
| --- | ---: | ---: | ---: | ---: |
| arith | +0.44% | 7/10 | -0.21% | 4/10 |
| prop-mono | +0.19% | 7/10 | -0.60% | 3/10 |
| prop-mega | -0.31% | 5/10 | -2.06% | 1/10 |
| call | +0.14% | 5/10 | +0.17% | 5/10 |
| closure-alloc | +0.24% | 7/10 | +5.87% | 7/10 |
| object-alloc | -1.26% | 2/10 | +0.92% | 5/10 |
| string-concat | -3.58% | 0/10 | -5.51% | 4/10 |
| array | +2.39% | 10/10 | -0.52% | 4/10 |
| primitive-string-property | -3.82% | 0/10 | -5.58% | 2/10 |
| primitive-string-method | -0.68% | 2/10 | -2.22% | 2/10 |
| proto-method | -1.93% | 1/10 | -0.99% | 4/10 |

| 全体の実時間 | JIT無効 | JIT有効 |
| --- | ---: | ---: |
| 全11項目・各10組 | -0.39% | -0.02% |
| 代表ページ・各20組 | -1.66% | -0.27% |

対象2項目が短縮し、両modeの全体とページの中央値に増加がなかったため、この候補を選ぶ。
JIT有効の全体-0.02%はほぼ同等であり、全体の速度向上を実証したとは扱わない。
個別にはJIT無効のarrayが+2.39%（10/10組遅い）、有効のclosure-allocが+5.87%（7/10組遅い）
という増加も残る。すべてのworkloadが速くなる変更ではなく、この差も採用判断の記録に含める。

全11項目のプロセス時間は全4pass・runtime準備・結果検査も含む。代表ページは
`http_page_input_fetch_and_dom_changes_reach_the_painted_frame` の入力→click→fetch→DOM更新→描画。
各mode20組を事前固定し、レポートの詳細と2枚のPNG hashを照合する。ベンチマークのJIT診断counterは
コンパイル時間を除き組ごとに照合する。ページのレポートにはJIT counterは含まれない。

## 動作検証と再現データ

追加した2テストは改善前にも成功を確認した。UTF-16、同じcall siteの異なる受信対象、
getter/Proxy/super、nullish例外、連結前のalias保持と明示GCを含む。
候補のJIT有効で、全11 workloadの結果・不正結果拒否を含む5テストが成功した。
関連検証はEngine value 127、runtime-helper 9、GC 50テストと、native JITの8契約バイナリ・
16件のnative/interpreter比較も成功した。CIはLinux x86_64 / Linux ARM64 / macOS ARM64の
Engine、native JIT、ブラウザ互換性を検証し、同じrevisionの配布gateも実行する。Intel macOSは対象外。

[全候補の測定と再現入力](measurements/string-length-2026-09-11.json)に、歴史的な再現、GC型の対照、
不採用候補を含む全サンプル、順序、計算結果、診断値、バイナリhash、patch、profile driverと
コンパイル/実行引数を保持する。再実行時はfixture・出力の絶対パスをチェックアウトに合わせ、
同条件の前後を作成する。生ログは `.artifacts/agent-worklog/2026-09-09/issue659/` に保持する。
