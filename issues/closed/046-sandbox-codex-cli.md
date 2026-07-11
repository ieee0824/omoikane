# 046: サンドボックスに Codex CLI を追加

## 目的

開発用サンドボックス（issue 045）に Codex CLI をインストールし、コンテナ内でも code-reviewer / codex-specialist エージェント等の `codex exec` ワークフローを使えるようにする。

参考: https://learn.chatgpt.com/docs/codex/cli#getting-started

## 設計

- 公式スタンドアロンインストーラ（`curl -fsSL https://chatgpt.com/codex/install.sh | sh`、Node.js 不要）を Dockerfile のレイヤーとして焼き込む
  - Claude Code と同様、ダウンロードと実行を分離し、同一 RUN 内で `codex --version` による検証まで行う
  - 本体は `~/.codex/packages/`、ランチャーは `~/.local/bin/codex`（PATH 設定済み）に入ることを実機確認済み
- `~/.codex` はログイン情報（auth）も含むため、named volume `codex-config` で永続化する
  - 本体パッケージも volume 側に乗るため、更新はコンテナ内でインストーラを再実行する（イメージ再ビルドでは既存 volume に反映されない）。README に明記
- docker-entrypoint.sh の所有者自己修復ループに `${CODEX_HOME:-$HOME/.codex}` を追加

## 作成・変更ファイル

- `Dockerfile` — Codex CLI インストール層を追加
- `docker-compose.yaml` — `codex-config` volume を追加
- `docker-entrypoint.sh` — 自己修復対象に `~/.codex` を追加
- `README.md` — Docker サンドボックスの説明に Codex CLI を追記

## 検証

- [x] `docker compose build` が成功する（RUN 内の `codex --version` 検証を通過）
- [x] コンテナ内で `codex --version` が動く
- [x] `~/.codex` が dev 所有で書き込み可能（codex-config volume 経由）
- [x] 既存機能のスモーク（claude / gh / rg / cargo build）が引き続き動く

## 検証結果（2026-07-10）

- `docker compose build` 成功（既存レイヤーはキャッシュ、Codex 層のみ追加ビルド。RUN 内の `codex --version` 検証を通過）
- コンテナ内 `codex --version` → `codex-cli 0.144.1`（`/home/dev/.local/bin/codex`）
- named volume `omoikane_codex-config` が新規作成され、イメージの `~/.codex`（packages 含む）を dev 所有 755 で引き継ぎ、書き込み可
- 既存機能のスモーク: `claude --version` 2.1.207 / `gh` 2.96.0 / `ripgrep` 13.0.0 / `CARGO_TARGET_DIR=/target` すべて正常

## 追記: MCP サーバー連携（2026-07-10）

Claude Code から Codex を MCP ツールとして呼べるように、リポジトリ直下に `.mcp.json`（プロジェクトスコープ）を追加した。

- `codex mcp-server`（stdio）を登録。公開ツールは `codex`（新規セッション）と `codex-reply`（スレッド継続）
- ホスト（codex-cli 0.137.0）とコンテナ（0.144.1）の両方で MCP ハンドシェイク・ツール列挙を検証済み
- 利用には初回のプロジェクトスコープ MCP 承認と、`codex login`（環境ごと）が必要
