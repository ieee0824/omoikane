# レイアウトメトリクスのキャッシュ

#619 の測定記録。500要素の文書で同じ要素の5種類のサイズを1,000回読む場合、メトリクスのnative取得を5,000回から1回へ集約した。5回の中央値は961.9msから175.7msとなり、81.7%短縮した。

| 測定 | 変更前 | 変更後 |
| --- | ---: | ---: |
| サイズ取得の中央値（各5回） | 961.9ms | 175.7ms |
| メトリクスのnative取得回数 | 5,000 | 1 |
| 取得値の合計（全回一致） | 762,000 | 762,000 |
| Acid3 selectorTestの中央値（交互実行、各3回） | 979.4ms | 975.5ms |
| Acid3スコア（全実行） | 100/100 | 100/100 |

native取得回数は幾何情報の計算・JSON生成を行う `__omoikane_layout_metrics` の回数。各getterでは世代を確認する軽いnative呼び出しが残り、その時間も測定値に含む。

2026-09-08、aarch64 Linux、Rust 1.98.0、devビルド（opt-level=0、debug=0）で測定した。変更前は `b77685c4c14b357bd423ec465ce70115e52572bf` と同じ実装ツリー。各回でruntimeを新しく作り、bodyのサイズ取得でレイアウトを準備してから、未取得の末尾要素を測定した。値、全サンプル、変更後ソースのSHA-256は [layout-metrics-results.json](layout-metrics-results.json) に保存している。

## 動作と無効化

JSのWeakMapに要素ごとの最新メトリクスを保持する。既存のstyle・layout・paint無効化処理からCSSOM用の世代を更新し、DOM、CSSOM、スクロール、transition、viewport、iframe文書の変更・破棄を反映する。子文書の変更では、親の公開RenderGenerationsは従来どおり維持する。

世代確認前に保留中のCSSOM編集を反映する。幾何情報を作る際のスクロールclampでも世代が進むため、取得後の世代を保存する。キャッシュ内部はfreezeし、公開する矩形・矩形配列は新しく作る。

## 再現

```sh
CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo run --locked --example layout_metrics_benchmark -- 500 1000
```

Acid3は [acid3-selector-probe.js](acid3-selector-probe.js) を使い、`tests/acid3_common/harness.rs` のscript実行後に `runtime.eval()` で計測を仕込んだ。既存のDirectDriveを回し、runtimeの破棄前に `JSON.stringify(selectorSamples)` を取得する。各回36回のselectorTestと335件のstyle確認を実行した。

比較の前後で、fetchしたページURLを `JsRuntime::with_document_and_url` に渡した。従来のharnessはresource baseだけを後から設定しており、文書がopaqueのまま子文書へアクセスできなかった。今回harnessもURL付きの初期化へ修正した。URL未設定の試行は性能比較から除外した。

最初の各5回の連続測定ではselectorTestの中央値が941.6ms／1017.3msとなったが、サイズ取得を使わないDOM・GCのtest 26にも時間差があった。前→後→後→前→前→後の順で再測定し、上表ではその中央値を採用した。連続測定もJSONへ残している。

selectorTestはDOM・stylesheet変更とgetComputedStyleを含み、今回のキャッシュによる高速化は確認できていない。既存resolverのキャッシュを利用し、追加のstylesheet増分解析やinline style解析キャッシュは、解析時間を個別に評価する対象とした。この変更は同一状態でのメトリクス取得の重複を解消する。

## 検証

メトリクス取得の集約、要素間の独立性、矩形の書き換え、inline style／CSSOM／ツリー変更、スクロール／transform／viewport、transition、子文書ルートの切り離しを回帰テストで確認した。ignoredと固定WPTを含む全2,281テストおよび `cargo build` は成功した。
