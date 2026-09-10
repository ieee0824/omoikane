# 計算結果を検証する JS benchmark（#658）

fixture v2 は配列を1024回のpushごとに空にする。添字の周期も1024なので、
各反復で読む値は `i` となり、500,000回の合計は `n*(n-1)/2 = 124999750000`。
v1 は1025回で空にするため1025回目から合計がずれ、1026回目に `NaN` になる。

全11 workloadの各passで、時計を止めた後に有限性と独立期待値を検査する。
異常時はworkload名・pass番号・実際の値を含む例外で測定を失敗させる。
`__benchSink` はworkloadごとに全4passの結果を保持し、出力行には計算結果を
5番目のフィールドとして加える。RustとFirefoxの記録側でも結果、回数、
重複・欠落、有限で正の時間を検査する。

期待値は `tests/engine_benchmark_results.rs` の整数和・剰余の式から導出した。
同テストはharnessの期待値表を使わず全bodyを実行し、配列の境界と、不正な
有限値・NaN・無限値・非数値を各passへ注入した場合の失敗も検査する。
Linux x86_64 / Linux ARM64 / macOS ARM64 のJIT無効・有効でCI検証する。

## 新しい測定と旧データの区別

baseline v8と新しい記録は、fixtureのversionとファイル全体のSHA-256を保持する。
Rustはfixtureが異なる場合、比率を計算する前に失敗する。OS・CPU・profile・
pass数・サンプル数・環境名・OmoikaneのJIT設定の差も比較可能性に反映する。
同じfixtureでも環境の違う参照値は参考値であり、速度をCIの合否条件にしない。

旧入力は `tests/js_benchmark/history/` にそのまま保存した。既存のGate 2/3/6の
測定JSONも変更しない。特に #659 の既知の遅延は旧fixtureでの測定であり、
v2の値を旧値と比較して改善したとは扱わない。

## 再測定方法

同じソース・lock・fixtureでJIT無効と有効を先にビルドする。計測とビルドを
同時実行せず、各モード5個の新しいJsRuntime、各workloadの4pass中最小値を
保存する。Omoikaneのbaselineは5個の中央値を使う。

```sh
OMOIKANE_JS_BENCH_RUNS=5 OMOIKANE_JS_BENCH_RAW_REPORT=/tmp/omoikane-off.json \
  cargo test --locked --test js_benchmark js_execution_benchmark_reports_every_shape -- --exact
OMOIKANE_JS_BENCH_RUNS=5 OMOIKANE_JS_BENCH_RAW_REPORT=/tmp/omoikane-on.json \
  cargo test --locked --features baseline-jit --test js_benchmark js_execution_benchmark_reports_every_shape -- --exact
OMOIKANE_SM_BENCH_REPORT=/tmp/firefox-primary.json \
  scripts/record-spidermonkey-reference.sh
OMOIKANE_SM_BENCH_REPORT=/tmp/firefox-confirmation.json \
  scripts/record-spidermonkey-reference.sh
```

`OMOIKANE_JS_BENCH_RAW_REPORT` は、新fixtureを旧baselineと比較せず記録する
専用モード。全passの正しさと測定の完全性は通常と同様に検査する。通常の
レポートには `OMOIKANE_JS_BENCH_REPORT` を使う。

Firefoxは各モード5個の新しいprofile/processを使い、最初の組の最小値を
referenceとする。2組目は同じ性能帯になることを確認するため保存し、良い方へ
差し替えない。比較対象は現在のbaselineで用いるSpiderMonkeyとOmoikaneの
それぞれJIT無効・有効。異なるfixtureや環境のV8旧値を転記しない。

生データ・環境・バイナリ識別は
[`measurements/benchmark-v2-2026-09-10.json`](measurements/benchmark-v2-2026-09-10.json)
に保存する。新baselineへの値の転記と全モードの計算結果はRustテストでも照合する。

## 今回の記録

2026-09-10、Linux ARM64、Rust 1.98.0、Firefox 155.0.1で再測定した。
4設定すべてで全11 workloadの計算結果が一致し、確認用Firefox測定を含む
330サンプルを保存した。OmoikaneのJIT有効測定は各runtimeで2936回の
native compiled entryを記録し、JITが実際に実行されたことも確認した。
旧baselineとの速度改善率は算出していない。
