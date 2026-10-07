# 個別transformの実装と検証

`translate`、`rotate`、`scale`は、それぞれの算出値を所有する内部型で解析する。
`transform-origin`の周囲でT→R→S→`transform`を合成し、親のperspectiveとの乗算を
4×4行列で行ってから要素平面へ投影する。構文、数値計算、補間、静的アニメーション、
積層コンテキストの判定は `src/css/style/individual_transform/` に分けている。

`none`と恒等変換を区別する。開始待ち中のCSS animationが作る積層コンテキストは
算出値と別の所有するフラグで保持し、終了・fill-modeの変化で再評価する。
固定配置の包含ブロックは実際の算出値から判定する。
Z方向だけが退化した行列も描画とヒット判定から除外し、CSSOMの座標値は維持する。

位置指定を後から配置するblockの子は、同じz-indexならDOM順へ戻す。
Flexのorderで並べた描画順は、既存の安定したz-indexソートを使う。
ヒット判定で祖先の外側の積層フェーズに参加する子は、祖先までのインデックス列を
使って祖先の変換とoverflowクリップを適用する。

## 検証

`tests/p1_transforms.rs`は算出値、CSS.supports、合成順、包含ブロック、描画、
ヒット判定、transition、静的keyframes、異なる回転軸、退化行列を検証する。
`src/js/individual_transform_tests.rs`はブラウザ用のライブ時計でkeyframesの補間と
開始待ち・終了時の積層コンテキストを検証する。

```sh
cargo test --locked --test p1_transforms
cargo test --locked --lib individual_transform_tests
WPT_ROOT=target/wpt scripts/fetch-wpt.sh
WPT_ROOT=target/wpt WPT_REQUIRED=1 cargo test --locked --test p1_transform_reftests -- --nocapture
```

固定revisionのparsing valid/invalid/computed 9件・178項目と、
`individual-transform`配下の合成・退化行列・積層コンテキストのreftest 11件を確認した。
reftestは改変しない上流HTMLと参照を800×600で比較し、全件の画素差は0だった。
XHTML参照はXMLとして読む。積層テストは指定領域の色も検査し、参照側にも同じ
未対応機能があるときに比較だけで成功しないようにする。
使用した25ソースのSHA256と固定revisionは
[individual-transforms-validation.json](individual-transforms-validation.json) に記録した。
対象のtestharnessケースは `tests/wpt/manifest.json` に登録し、reftestはWPT CIでも実行する。
