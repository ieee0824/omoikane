# Omoikane — エージェント向け開発ガイド

このファイルはリポジトリ全体の作業指針です。ユーザーの明示的な指示を優先します。

## 基本方針

- ユーザーへの説明は日本語で、変更内容・理由・検証結果を簡潔に伝える。
- 依頼された範囲の調査・実装・検証を進める。承認済みの作業について、着手や通常の実装判断の確認を繰り返さない。
- 大きな変更は短い方針を共有し、レビュー可能な単位に分ける。サブエージェントの利用はユーザーの指示に従う。
- 作業前にブランチ、作業ツリー、関連するIssue・PRを確認する。既存の変更や別worktreeを上書き・削除しない。
- 検索は `rg`、`rg --files`、`git ls-files` を使い、ビルド成果物や別worktreeを不用意に再帰探索しない。
- 進捗は実装、最新のIssue・PR、実行結果で確認する。READMEの件数や過去の計測値を現在の保証として扱わない。

## プロジェクトと技術方針

OmoikaneはRustで開発するブラウザエンジンです。HTTP、HTML/CSS、DOM、レイアウト、描画、JavaScript、CDP互換API、C FFIを提供します。

- ブラウザ固有のコア機能は自前実装を基本とし、依存の追加は必要性を説明する。TLS・暗号・圧縮などのインフラにはpure Rustクレートを利用できる。
- JavaScriptは取り込み済みのBoaを `engine/boa/` で保守する。関連クレートは同ツリー内のpath依存に揃え、ルートとengineの `Cargo.lock` を管理する。由来・変更検証は [engine/README.md](engine/README.md) に従い、取込元記録を変更履歴の隠蔽に使わない。
- JITの実験用featureと本番の実行経路を区別する。設計判断・対応範囲は `docs/jit/` と関連Issueを確認する。
- データ構造・公開APIを変える際は、DOM、JS、レイアウト、描画、CDP、FFIの利用側への影響を確認する。
- 公開APIにはdocコメントを付ける。既存のコード構成とRustの書式に合わせ、無関係な変更を混ぜない。

### 主な配置

| パス | 役割 |
| --- | --- |
| `src/http/`、`src/html/`、`src/dom/` | 通信、HTML解析、DOM |
| `src/css/`、`src/layout/`、`src/font/`、`src/paint/` | スタイル、配置、文字、描画 |
| `src/js/`、`engine/boa/` | Web APIバインディング、JavaScriptエンジン |
| `src/cdp/`、`src/ffi/`、`include/` | 外部API、Cヘッダー |
| `src/screenshot/`、`src/platform*.rs`、`src/bin/` | スクリーンショット、プラットフォーム連携、GUI |
| `tests/`、`examples/`、`scripts/` | 回帰テスト、利用例、検証用スクリプト |
| `.github/workflows/` | CI、リリースの実行条件 |

## Issueと進捗の管理

- タスク・未解決の課題・設計判断・相談はGitHub Issueで管理し、関連するIssue・PRの最新の議論を引き継ぐ。
- ローカルIssueは今後作成・運用しない。`issues/` は過去の記録を参照するためのアーカイブとし、進捗は移行先のGitHub Issueへ記録する。
- 子IssueはGitHub上で親Issueとの関係を明記し、親からリンクする。親Issueは子Issueがすべて完了してから閉じる。
- Issueを追加する前にGitHubのopen/closed Issueを検索し、重複を避ける。完了条件を満たし、検証結果を該当Issue・PRへ記録してから閉じる。

## 作業環境・証跡の管理

- 大容量のビルド・全体テストにはworktreeごとの `CARGO_TARGET_DIR` と [容量guard](scripts/cargo-space-guard.py) を使う。設定・実行例は [README.md](README.md) のDockerサンドボックス節を参照する。
- Cargo成果物の権限エラーや、その直後の未定義symbolを検出したら同じtargetを再利用しない。guardの `reset`をdry-run後に実行してtarget全体を隔離し、clean targetで再検証する。Cargoを`sudo`で実行しない。
- cache整理はguardの `clean` でdry-runを確認してから実行する。実行中のtarget、未コミット変更、原画像、ログなどの証跡を削除しない。
- 長時間作業は工程・結果・失敗箇所を短く報告し、詳細ログを保存する。区切りごとに目的、完了事項、未解決事項、検証結果、次の手順、worktree・証跡のパスを既存の備忘録へ記録する。
- 画像比較は寸法・容量・数値差分・差分領域を先に確認し、目視が必要な画像を選ぶ。原画像を保持し、無圧縮PNGは別ファイルへ可逆圧縮してから表示する。画素・解像度を維持し、不要な再表示を避ける。
- コンテキスト圧縮（compact）が再び失敗したら、無制限に再試行せず作業状態を保存して報告する。復旧目的で履歴・画像・goal情報を削除しない。

## 検証

- 新機能と不具合修正には、期待する振る舞いを確認する具体的なテストを追加する。可能なら修正前の失敗を確認する。
- 既存テストの削除、アサーションの弱化、根拠のない許容誤差の拡大で失敗を隠さない。
- コード変更では関連テストから確認し、コミット前に `cargo test --locked` と `cargo build --locked` を実行する。Rustコードの書式も確認する。
- 文書のみの変更では、内容・リンク・`git diff --check` を確認すればよく、ビルドやテストは不要。
- 実行できない検証や既存の失敗は、コマンド・理由・確認できた範囲を報告する。変更前でも再現するかを確認し、成功したと報告しない。

追加検証は変更箇所に応じて実行する。CIの正確なコマンドは [ci.yml](.github/workflows/ci.yml) と対象機能のworkflowを参照する。

```sh
# ignoredを含めたCI相当のテスト
CI=1 cargo test --locked -- --include-ignored

# GUIの変更
cargo check --locked --features gui

# Web APIの互換性
cargo test --locked --test web_api_surface -- --nocapture

# 固定revisionのWPTを取得して検証
scripts/fetch-wpt.sh
WPT_REQUIRED=1 cargo test --locked --test wpt_smoke -- --nocapture
```

- `CI=1` はローカル専用の入力を必要とする既存デバッグutilityをCIと同じ条件で扱うために指定する。skipされた検証を実行済みと報告しない。
- CIのRust書式検査は `python3 scripts/check_rustfmt.py` を使う。このスクリプトはコミット済みrevision（既定は `HEAD`）が対象で、未コミット変更は検査しない。
- JITを変更する場合は、対応するfeatureを有効にした契約テストと、インタプリタとの結果比較を行う。
- 描画fixtureと画像差分は [tests/README.md](tests/README.md) に従う。baselineは明示的な更新操作で生成し、目視確認・比較テストを終えてから更新理由とともにコミットする。

## コミット・PR・レビュー

1. 変更目的ごとにコミットし、コミット済みの差分をローカルでレビューする。
2. 問題があれば修正・検証・再コミットし、再度レビューする。
3. レビュー後にpushし、PRを作成・更新する。タイトルと本文は最終差分に合わせ、問題、変更後の振る舞い、検証結果、未解決事項を書く。
4. 必要なCI結果と関連する不具合を確認する。マージはユーザーから依頼されている場合に行う。

## Shared Context

このリポジトリでは [ctx-sync](https://github.com/ieee0824/ctx-sync) で開発コンテキストを共有する。

- Gist IDを含むローカルの `.ctx-sync.toml` はGitに追加しない。未設定ならリポジトリ外で共有されたIDを使って `ctx-sync attach` する。CLIの導入方法はリンク先のREADMEを参照する。
- 作業前に `ctx-sync agent start` を実行し、構成・決定・稼働中の作業者・注意事項を読む。作業範囲が決まったら `ctx-sync claim` で宣言する。
- 作業の区切りには `ctx-sync agent finish` で変更、影響、未解決事項を引き継ぐ。
- 思考過程、一時的なデバッグ記録、認証情報などの秘密は共有しない。タスクと未解決の不具合は引き続きGitHub Issueで管理する。
