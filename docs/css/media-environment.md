# メディア環境の実装と検証

Issue #1279のメディア特性は、所有する `MediaEnvironment` を入力として評価する。
クエリの評価自体はOS設定・入力デバイス・時刻を読み取らない。
GUI、テスト、CDPが同じ型のスナップショットを渡す。

## 環境値とクエリ

`src/css/media/environment.rs` に離散特性の許容値と初期値を定義する。
`hover` / `any-hover`、`pointer` / `any-pointer`、`update`、`scan`、`grid`、
`overflow-block` / `overflow-inline`、`color-gamut`、`prefers-*`、`forced-colors`、
`inverted-colors`、`scripting`、`dynamic-range` / `video-dynamic-range`、
`display-mode`を評価する。`any-pointer`は複数デバイスの能力の和集合を表せる。

数値特性にはviewport寸法、device寸法・比率、resolution、color、color-index、
monochromeを使う。resolutionはDPRをdppxで保持し、dpi・dpcmとの変換、range構文、
数式を解析する。未知の特性や未対応の値は `unknown` として三値論理で合成し、
最終結果がtrueの場合にだけ一致する。未知を単にfalseとして否定しない。

初期値はソフトウェア描画のデスクトップ環境を表す。DPRは1、色は成分ごとに8bit、
直接色、sRGB、標準dynamic range、scripting enabled、display-mode browser。
テレビのscanは利用不可で、明示的に利用可能とした環境のみ値を評価する。
`device_width` / `device_height`が未指定の実行環境では、トップ文書のviewportを
仮想画面寸法として使う。子文書のviewport寸法とは別に保持する。

## 実行環境への反映

`JsRuntime::set_media_environment`は、環境値を所有して保持する。
無効なDPRは1へ正規化する。CSS、`matchMedia().matches`、`devicePixelRatio`、
`screen`の読取値は変更直後の環境を観測する。

MediaQueryListは作成したWindowの文書・viewportで評価する。changeの通知は
描画機会まで保留してまとめ、resize/scrollの報告後、animation-frame callback前に
行う。購読中のMediaQueryListを保持し、購読がないものは弱参照で管理する。
once・AbortSignal・removeEventListener・onchangeの変更に追従する。
接続中の子文書は親から順に報告し、退役した文書を通知対象から除く。

GUIはWindowから得たDPR・monitor寸法を反映してから初期URLへ移動する。
文書を置き換える際も、最新の環境とviewportを初期スクリプトへ引き継ぐ。
ホスト設定の取得は `src/platform_media/`、GUIへの反映は
`src/bin/omoikane/media_environment.rs` に分ける。

| ホスト | ネイティブ入力 |
| --- | --- |
| Linux | XDG portalのappearance設定。GUI/X11ではXInput2のデバイス一覧も読む |
| Windows GUI | Win32のanimation・high-contrast設定、system colors、Raw Input・Pointer Devices |
| macOS GUI | メインスレッドのAppKit accessibility設定、IOKitの入力デバイス一覧 |

Linuxのportalは各要求に1秒のtimeoutを設定する。未提供の設定は既存環境を保持する。
[portal仕様](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Settings.html)
で予約されたcontrast/reduced-motionの値はno-preferenceとして扱う。
WaylandにはX11の一覧を流用せず、観測した入力をfallbackに使う。
macOSは通知とメインrun-loopのtimerで変更を捕捉し、他のホストは変更時のみ
更新を送るsamplerを使う。monitorの破棄後は新たな更新を発行しない。
未提供の色域・HDR・data-saving設定を実機から取得できた値として扱わない。

## CDPと強制色

`Emulation.setEmulatedMedia`は変更を検証してから一括適用する。
例えば次のparamsでprintとreduced-motionを選択する。

```json
{"media":"print","features":[{"name":"prefers-reduced-motion","value":"reduce"}]}
```

空のmediaはmedia overrideを解除し、空のfeatures配列はfeature overrideを解除する。
ホストの基底スナップショットを別に保ち、無効な指定では既存状態を変えない。

`ForcedColorPalette`は11色のRGB値を所有する。Windowsのsystem color取得は
OS所有のbrushを保持しない。palette変更だけでも更新を通知する。
通常の計算済みスタイルとvisited snapshotを保持し、描画時のowned styleから
強制色を導出する。明示的なsystem colorの由来を保持し、作者の同じRGB値と区別する。
`forced-color-adjust`の継承・opt-out・preserve-parent-color、SVGのUA既定値、
rootの方針を使ったviewport背景を適用する。背景をviewportへ伝播する場合は
元のboxで同じ背景色を二重に描かず、fullscreen要素自身の背景は描画する。

## 検証

`tests/p1_media.rs`は数値・離散特性、unknown、MediaQueryListの通知・Realm・購読を
検証する。`tests/p1_media_paint.rs`は強制色、system color、背景伝播、通常スタイルの
保持と設定解除を画素・計算済み値で確認する。CDPとnative capture/monitorの契約は
それぞれのモジュール内で検証する。

```sh
cargo test --locked --test p1_media --test p1_media_paint
WPT_ROOT=target/wpt scripts/fetch-wpt.sh
WPT_ROOT=target/wpt WPT_REQUIRED=1 cargo test --locked --test p1_media_reftests -- --nocapture
cargo build --locked --features gui --bin omoikane --example media_native_probe
cargo run --locked --features gui --example media_native_probe
```

固定revisionのメディア関連testharnessを `tests/wpt/manifest.json` に登録する。
reftestは改変しない上流HTML18件とXHTML参照を800×600で比較し、指定領域の緑色も
検査する。WPTがない場合の任意skipを実行済みとして扱わない。
GUIの最初のスクリプトのDPR・画面寸法・pointerを
`scripts/test-gui-media-environment.py` で検証する。
`.github/workflows/media-native.yml` はmacOS/WindowsでGUIをビルドし、メインスレッドの
native probeで実際のcapture・更新・破棄後の通知停止を確認する。
結果・revision・未完了のOS検証はIssue #1279と対象PRに記録する。
