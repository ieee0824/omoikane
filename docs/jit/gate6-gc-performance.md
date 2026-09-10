# Gate 6: GC内部ポインタ集合の性能変更

2026-09-10、#515 / #553 の独立化後の改善として、GC内部の割り当てアドレス集合を
既存依存の `hashbrown::HashSet` に切り替える。ブラウザ操作の不具合修正と基準確定
（PR #654）を先に行った。root/ephemeron/remembered-setの要素の同一性、参照数、
走査・回収条件、公開APIは保持する。一般の `std::collections::HashSet` の `Trace`
実装は変更しない。依存の追加やlockfileの更新はない。

## 選定根拠

変更前のallocation workloadをCallgrindで調べると、内部ポインタの標準ハッシュ計算が
命令実行の相当部分を占めていた。診断ではbootstrapを除き、元bodyの0/5,000/20,000回を
5形状で比較し、全15条件で計算結果を確認した。この命令数の診断と、以下の元の回数での
実時間比較を区別する。計算量の次数が変わったという主張ではない。

全11項目を実行するテスト全体はJIT無効で中央値9.79%、有効で4.64%短縮し、
固定HTTPページでも追加20組の比較で4.84%・6.25%短縮したため、この変更を採用する。
JIT有効時の文字列2項目の遅延は残り、[Issue #659](https://github.com/ieee0824/omoikane/issues/659)
で追跡する。すべてのworkloadが速くなったという判断ではない。

別候補のroot変換削減は、複数項目で5組すべて遅くなったため採用しなかった。
その[不採用記録](https://github.com/ieee0824/omoikane/issues/553#issuecomment-5617362030)も保持する。

## 再現条件

- 比較元: `484e26e5573a3381fe9505cc2a5a6113cc97edd2`。
  比較後は同じrevisionにGCの2ファイルと計算結果確認テストを追加したもの。
  当時の全patch SHA256は `e820ba5ffd809472cb1a7f5226e67668e79199828afca10b24c8700dc94478de`。
  時計修正PR #656の前の同一基準でGC変更を切り分けた測定であり、統合後ソースの測定とは称さない。
- Linux aarch64 / Rust 1.98.0、同一ホスト・toolchain・lock・fixture。
  `cargo test` の既存profile（Omoikaneはopt-level 1、依存は2、debug assertionsとoverflow checks有効）。
  既存レポートのprofile欄は `dev`。releaseの速度や他CPUの速度を保証する値ではない。
- JIT無効と `--features baseline-jit` 有効でそれぞれバイナリを作成し、hashを保存。
  ビルドを時間に含めず、測定中は他の重いローカル作業を重ねない。
  referenceも保存バイナリを新たに実行し、過去の測定値を流用しない。
- 各サンプルは新しいプロセス/runtime。`OMOIKANE_JS_BENCH_RUNS=1`、既存の
  `js_execution_benchmark_reports_every_shape --exact --nocapture --test-threads=1` を実行。
  元の11 bodyと回数、各4 pass中の最小ns/opを保持し、変更前/後の実行順を交互にする。
- 初回は各mode 5組。初回のJIT有効文字列参照の遅延とページのばらつきを受け、実行前に
  「JIT有効5組追加、ページ各mode 20組追加」と固定して実施した。初回を捨てず個別・合算で示す。
- 初回途中で終了143が発生した。完了6サンプルと未完了ディレクトリを保持し、ソース・8バイナリの
  hash一致を確認して未完了14サンプルのみ再開した。最初のログwrapperがexit 0と記録した点は訂正済み。
  終了シグナルの送信元は未確認で、速度に基づくサンプル除外は行っていない。

[各サンプルの実測値・順序・入力/バイナリhash・診断counter](measurements/gc-pointer-sets-2026-09-10.json)
を保存する。元レポートの歴史的baselineやSpiderMonkey値は今回の比較に使用しない。

## 全11項目

各組の変更後/変更前の時間比を取り、その中央値を変化率で表示する。負値は短縮。
JIT無効は初回5組、有効は初回5組・追加5組と合計10組を併記する。

| workload | 元の回数 | JIT無効5組 | 有効・初回5組 | 有効・追加5組 | 有効・全10組 |
| --- | ---: | ---: | ---: | ---: | ---: |
| arith | 2,000,000 | +0.31% | +1.99% | -0.45% | -0.23% |
| prop-mono | 1,000,000 | -9.62% | +2.97% | -0.78% | +0.00% |
| prop-mega | 500,000 | -13.51% | -9.03% | -10.06% | -9.82% |
| call | 1,000,000 | -7.02% | -3.75% | -3.20% | -3.57% |
| closure-alloc | 300,000 | -14.21% | -8.33% | -9.79% | -9.78% |
| object-alloc | 300,000 | -14.69% | -7.87% | -9.07% | -8.48% |
| string-concat | 200,000 | +0.36% | +0.75% | +3.31% | +2.58% |
| array | 500,000 | -7.22% | -2.65% | -2.60% | -2.63% |
| primitive-string-property | 500,000 | +1.11% | +4.62% | +5.13% | +4.87% |
| primitive-string-method | 500,000 | -5.53% | -1.57% | -3.02% | -2.98% |
| proto-method | 500,000 | -6.00% | -5.44% | -5.75% | -5.44% |

全11項目を4 passずつ実行するRustテスト全体の時間（セットアップを含むlibtestの
`finished in`）は、JIT無効5組で中央値-9.79%、有効の初回5組-4.06%、追加5組-4.80%、
有効全10組-4.64%。無効5/5組、有効10/10組で短縮した。上表の変化率の平均ではない。

JIT有効の `primitive-string-property` は9/10組、`string-concat` は8/10組で遅延した。
この差の直接原因は未確定であり、測定誤差として片付けない。

## 代表ページと意味論

固定HTTP fixtureの `http_page_input_fetch_and_dom_changes_reach_the_painted_frame` を
各サンプルで新しいプロセスから実行し、起動・HTTP取得・入力・fetch・DOM更新・描画までの
実時間を測る。ネット上のサイトやJS時間だけの測定ではない。

| mode | 初回5組の中央値 | 追加20組の中央値 | 追加20組で短縮した組 | 追加20組の変化率範囲 |
| --- | ---: | ---: | ---: | ---: |
| JIT無効 | -5.52% | -4.84% | 18/20 | -12.94%〜+1.19% |
| JIT有効 | -4.56% | -6.25% | 20/20 | -9.53%〜-0.05% |

全100回のページ実行で操作結果が成功し、入力前後2枚のPNG hashが一致した。
JIT診断はコンパイル時間を除く全counterが各組で一致した。

GCの既存50テスト、通常build、JIT無効/有効の計算結果確認テストが成功した。
新しい [engine_benchmark_results](../../tests/engine_benchmark_results.rs) は元の各bodyを
元の回数で個別に実行し、11結果を比較する。有限値は整数計算から導出しV8でも照合した。
既存array fixtureは途中でundefinedを読みNaNを返すため、shared sinkだけでは
意味論を保証できない（[Issue #658](https://github.com/ieee0824/omoikane/issues/658)）。
今回はfixtureを変えずその現象も個別に検査する。改善時は新しいbaselineが必要になる。

統合したソースの全ブラウザsuite・操作画像・Acid3/WPT/Web API、GC/JIT stress、
engine全Test262比較、3環境の実archiveはPR #660の最終CIで成功した。
[最終検証](gate6-final-verification.md)に実行ソース・ケース照合・配布hash・既知の制限を記録する。
運用方式・原本・更新責任・rollbackは [ADR](gate6-engine-ownership.md) に従う。
