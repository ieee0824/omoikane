# Omoikane — エージェント向け開発ガイド

このファイルはリポジトリ全体の作業指針です。ユーザーの明示的な指示を優先します。

## 作業の進め方

- ユーザーへの説明は日本語で、変更内容・理由・検証結果を簡潔に伝える。
- 依頼された範囲の調査・実装・検証を進める。承認済みの作業について、着手や通常の実装判断の確認を繰り返さない。
- 大きな変更は短い方針を共有し、レビュー可能な単位に分ける。独立した調査・実装・レビューは並列化してよい。
- 作業前にブランチ、作業ツリー、対象Issue・PRを確認する。既存の変更や別の作業ディレクトリを上書き・削除しない。
- 検索は `rg`、`rg --files`、`git ls-files` を使い、ビルド成果物や別worktreeを不用意に再帰探索しない。
- 進捗は実装、最新のIssue・PR、実行結果で確認する。READMEの件数や過去の計測値を現在の保証として扱わない。

## プロジェクトと技術方針

OmoikaneはRustで開発するブラウザエンジンです。HTTP、HTML/CSS、DOM、レイアウト、描画、JavaScript、CDP互換API、C FFIを提供します。

- ブラウザ固有のコア機能は自前実装を基本とし、依存の追加は必要性を説明する。TLS・暗号・圧縮などのインフラにはpure Rustクレートを利用できる。
- JavaScriptはBoa forkを利用する。依存revisionは `Cargo.toml` と `Cargo.lock` を確認し、関連するBoaクレート間で揃える。
- JITの実験用featureと本番の実行経路を区別する。設計判断・対応範囲は `docs/jit/` と関連Issueを確認する。
- データ構造・公開APIを変える際は、DOM、JS、レイアウト、描画、CDP、FFIの利用側への影響を確認する。
- 公開APIにはdocコメントを付ける。既存のコード構成とRustの書式に合わせ、無関係な変更を混ぜない。

## 主な配置

- `src/http/`、`src/html/`、`src/dom/`: 通信、HTML解析、DOM。
- `src/css/`、`src/layout/`、`src/font/`、`src/paint/`: スタイル、配置、文字、描画。
- `src/js/`: JavaScript実行とWeb APIバインディング。
- `src/cdp/`、`src/ffi/`、`include/`: 外部APIとCヘッダー。
- `src/screenshot/`、`src/platform*.rs`、`src/bin/`: スクリーンショット、プラットフォーム連携、GUI。
- `tests/`、`examples/`、`scripts/`: 回帰テスト、利用例、検証用スクリプト。
- `.github/workflows/`: CIとリリースの実行条件。

## タスクの記録

- タスク・未解決の課題・設計判断・相談はGitHub Issueで管理し、関連するIssue・PRの最新の議論を引き継ぐ。
- ローカルIssueは今後作成・運用しない。`issues/` は過去の記録を参照するためのアーカイブとし、進捗は移行先のGitHub Issueへ記録する。
- 子IssueはGitHub上で親Issueとの関係を明記し、親からリンクする。親Issueは子Issueがすべて完了してから閉じる。
- Issueを追加する前にGitHubのopen/closed Issueを検索し、重複を避ける。完了条件と検証結果を該当Issue・PRへ記録して閉じる。

## 検証

- 新機能と不具合修正には、期待する振る舞いを確認する具体的なテストを追加する。可能なら修正前の失敗を確認する。
- 既存テストの削除、アサーションの弱化、根拠のない許容誤差の拡大で失敗を隠さない。
- コード変更では関連テストから確認し、コミット前に `cargo test` と `cargo build` を実行する。Rustコードを変更した場合は書式も確認する。
- 文書のみの変更では、内容・リンク・`git diff --check` を確認すればよく、ビルドやテストは不要。
- 実行できない検証や既存の失敗は、コマンド・理由・確認できた範囲を報告する。変更前でも再現するかを確認し、成功したと報告しない。

追加検証は変更箇所に応じて実行する。CIの正確なコマンドは `.github/workflows/ci.yml` を参照する。

```sh
# ignoredを含めたCI相当のテスト
cargo test -- --include-ignored

# GUIの変更
cargo check --features gui

# Web APIの互換性
cargo test --test web_api_surface -- --nocapture

# 固定revisionのWPTを取得して検証
scripts/fetch-wpt.sh
WPT_REQUIRED=1 cargo test --test wpt_smoke -- --nocapture
```

- JITを変更する場合は、対応するfeatureを有効にした契約テストと、インタプリタとの結果比較を行う。
- 描画fixtureと画像差分は `tests/README.md` に従う。baseline更新前に画像を目視確認し、更新理由を記録する。

## コミット・PR・レビュー

- 変更目的ごとにコミットし、コミット済みの差分をローカルでレビューする。
- 問題があれば修正・検証して再コミットし、再度ローカルレビューする。
- ローカルレビューを終えてからpushし、PRを作成・更新する。PRには問題、変更後の振る舞い、検証結果、未解決事項を書く。
- PRの更新時は、タイトルと本文を最終的な変更内容に合わせる。
- 必要なCI結果と関連する不具合を確認する。
- マージはユーザーから依頼されている場合に行う。
