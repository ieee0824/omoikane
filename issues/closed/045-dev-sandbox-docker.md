---
number: 045
slug: dev-sandbox-docker
status: closed
---

# 開発用サンドボックス Docker 環境

## 目的

ホスト環境を汚さずに omoikane のビルド・テスト・Claude Code 実行ができるコンテナ環境を用意する。

- 開発者ごとの環境差異（フォント・ネイティブビルドツールの有無）を排除し、CI と同一の前提でビルド・テストを再現できるようにする
- Claude Code をコンテナ内で実行し、ホストの設定やツールチェインと分離する
- ビルド成果物をホストの `target/` と分離し、ホスト側のビルドキャッシュを壊さないようにする

## 設計

- **ベースイメージ**: `rust:1-bookworm`（安定版 Rust ツールチェイン同梱）
- **cmake**: `aws-lc-sys`（rustls 0.23 の依存）のビルドに必須のため導入する。あわせて `build-essential` / `pkg-config` を入れ、rusqlite（bundled SQLite）等のネイティブビルドにも対応する
- **フォント**: CI（`.github/workflows/ci.yml`）と同一の `fonts-dejavu` / `fonts-liberation` / `fonts-noto-core` に加え、CJK レンダリング用に `fonts-noto-cjk` を追加する
- **開発 CLI**: シェルでのコード検索用に `ripgrep`（`rg`）、PR ワークフロー用に GitHub CLI（`gh`、公式 apt リポジトリからインストール）を入れる
- **非 root ユーザー**: 作業ユーザー `dev`（uid/gid 1000）を作成し、コンテナ内はこのユーザーで操作する
- **Claude Code**: ネイティブインストーラ（`curl -fsSL https://claude.ai/install.sh | bash`）でインストールし、`~/.local/bin` を PATH に通す
- **ログイン永続化**: `CLAUDE_CONFIG_DIR` を設定し、その先を named volume でマウントすることで、コンテナを作り直してもログイン情報を保持する
- **ビルド出力分離**: `CARGO_TARGET_DIR=/target` を設定し、ホストの `target/` と分離した専用ボリュームに出力する
- **cargo キャッシュ永続化**: `/usr/local/cargo/registry`（crates.io インデックス / crate ソース）と `/usr/local/cargo/git`（git 依存）を named volume でマウントし、コンテナを作り直しても（`down` / `up`）クレートを再ダウンロードしないようにする
- **所有者の自己修復**: エントリポイント（`docker-entrypoint.sh`）でコンテナ起動時に `/target` と cargo キャッシュボリュームの所有者を `dev` に修復する。過去の設計で作られた root 所有の named volume を再マウントしても Permission denied にならない（所有者が既に `dev` の場合はスキップするので遅くならない）
- **git safe.directory**: colima のバインドマウントはコンテナ内で root 所有に見えるため、git が "dubious ownership" でリポジトリを拒否しないよう `safe.directory` に `/workspace` を設定する

## 作成ファイル一覧

- `Dockerfile` — サンドボックスイメージ定義
- `docker-compose.yaml` — サービス・ボリューム定義（バインドマウント / target 分離 / cargo キャッシュ永続化 / Claude Code 設定永続化）
- `docker-entrypoint.sh` — 起動時に `/target` と cargo キャッシュボリュームの所有者を自己修復するエントリポイント
- `.dockerignore` — ビルドコンテキストから除外するファイル
- `README.md` — 「## クイックスタート」に「### Docker サンドボックス」を追加

## 検証

- [x] `docker compose build` が成功する
- [x] スモークテスト（コンテナ起動 → シェルに入れる）
- [x] コンテナ内で `cargo build` が成功する
- [x] コンテナ内で `CI=1 cargo test -- --include-ignored` が成功する

## 検証結果

検証環境: ホスト colima / Docker Engine 29.2.1（server）/ arm64。`docker-compose.yaml` が定義するサービスは `dev` の 1 つのみ（`docker compose config --services` で確認）。サービスが `tty: true` のため、TTY を持たない環境では `docker compose run -T` を使う必要がある。コマンドはすべてスペース区切りの `docker compose`（v2）で実行。

- **イメージビルド（`docker compose build dev`）**: 成功。所要 ~163s、`omoikane-dev:latest`（~2.75GB）。apt で cmake / pkg-config / build-essential / git / curl / ca-certificates / sudo とフォント（dejavu / liberation / noto-core / noto-cjk）を導入。`dev` ユーザー（uid/gid 1000、NOPASSWD sudo）を作成し、`CARGO_TARGET_DIR=/target` を dev 所有で用意。Claude Code ネイティブインストーラ成功（v2.1.206、`~/.local/bin/claude`）。`git safe.directory /workspace` 設定済み。
- **スモークテスト**: 合格。`whoami=dev` / `id`=uid=1000(dev) gid=1000(dev) / rustc 1.97.0 / cargo 1.97.0（>=1.85）/ claude 2.1.206（`which claude`=`/home/dev/.local/bin/claude`）/ cmake 3.25.1。`CARGO_TARGET_DIR=/target`・`CLAUDE_CONFIG_DIR=/home/dev/.claude` 設定確認。フォント（dejavu / liberation / noto）配置確認。`/workspace` へのバインド書き込み OK。`git -C /workspace status` は "dubious ownership" エラーなし（safe.directory 有効）。
- **コンテナ内 `cargo build` / `CI=1 cargo test -- --include-ignored`**: green（ワークフロー全体の検証で成功と判定）。

### 検証中に見つかり解消した問題

1. **`gh` / `rg` 未導入**: 検証時点のイメージに GitHub CLI（`gh`）と ripgrep（`rg`）が入っておらず `command not found` となった。→ Dockerfile に `ripgrep` と `gh`（公式 apt リポジトリ、鍵検証用に `gpg` も）の導入を追加して解消。
2. **`/target` への書き込みが Permission denied**: 過去の設計で作られた root 所有の stale な `omoikane_cargo-target` named volume を再マウントしたため初回書き込みが失敗。→ 起動時に所有者を自己修復する `docker-entrypoint.sh` を用意。検証では stale ボリュームと保持コンテナを削除し、新規ボリュームが dev:dev を継承して `TARGET_WRITE_OK` を確認。

## 備考

- 上記 1（`gh` / `rg`）の修正を含めてイメージを再ビルドし、実機で導入を確認済み（`gh` 2.96.0 / `ripgrep` 13.0.0 / Claude Code 2.1.206 入り）。
- `colima` 環境では `docker compose build` 時に `buildx` 未導入の警告が出て legacy builder にフォールバックするが、ビルドは成功する（`buildx` が必須の Docker 環境では `brew install docker-buildx` 等の導入が必要になる可能性がある）。
