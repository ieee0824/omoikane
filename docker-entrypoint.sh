#!/bin/bash
# サンドボックス用エントリポイント。
#
# named volume は「新規作成時のみ」マウント先ディレクトリの所有者をイメージから引き継ぐ。
# 既存の（過去の設計で作られた root 所有の）ボリュームを再マウントすると dev が書き込めず、
# /target への出力や cargo キャッシュの書き込みが Permission denied になる。
# そこでコンテナ起動時に所有者を自己修復する。所有者が既に dev のときは何もしない
# （初回のみ再帰 chown が走り、2 回目以降はスキップされるので遅くならない）。
set -e

self_uid="$(id -u)"
# CLAUDE_CONFIG_DIR（既定 /home/dev/.claude）は Claude Code 設定の named volume（claude-config）。
# Dockerfile の ENV と整合させるためパスを直書きせず参照する。他と同様に所有者を自己修復する。
# CODEX_HOME（既定 /home/dev/.codex）は Codex CLI の named volume（codex-config）。
for d in /target /usr/local/cargo/registry /usr/local/cargo/git \
         "${CLAUDE_CONFIG_DIR:-/home/dev/.claude}" "${CODEX_HOME:-$HOME/.codex}"; do
    if [ -d "$d" ] && [ "$(stat -c %u "$d")" != "$self_uid" ]; then
        sudo chown -R "$(id -u):$(id -g)" "$d" || true
    fi
done

exec "$@"
