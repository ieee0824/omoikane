# 開発用サンドボックス
#
# ホスト環境を汚さずに omoikane のビルド・テスト・Claude Code 実行を行うためのコンテナ。
# ビルド・テストは CI（.github/workflows/ci.yml）と揃えた環境で再現できるようにする。
FROM rust:1-bookworm

# aws-lc-sys（rustls 0.23 の依存）のビルドに cmake が必須。
# build-essential / pkg-config は rusqlite（bundled SQLite）などのネイティブビルドに使う。
# build-essential / pkg-config / git / curl / ca-certificates / gpg / openssh-client は
# rust:1-bookworm（buildpack-deps ベース）に既に含まれるが、ベースイメージの変化に備えて明示的に指定する。
# openssh-client は SSH 鍵の生成・利用（ssh-keygen / ssh。README の SSH 鍵運用参照）に使う。
# フォントは CI と同一のもの（fonts-dejavu / fonts-liberation / fonts-noto-core）に加え、
# 日本語（CJK）レンダリング用に fonts-noto-cjk と IPA フォント（ゴシック・明朝）を追加する。
# IPA フォントはフォント探索の CJK フォールバック候補（IPAGothic 等。src/font/mod.rs 参照）に対応する。
# ripgrep（rg）はシェルでのコード検索に、vim は編集に、gpg は gh の apt リポジトリ鍵の検証に使う。
RUN apt-get update && apt-get install -y --no-install-recommends \
        cmake \
        pkg-config \
        build-essential \
        git \
        curl \
        ca-certificates \
        gpg \
        openssh-client \
        sudo \
        ripgrep \
        vim \
        fonts-dejavu \
        fonts-liberation \
        fonts-noto-core \
        fonts-noto-cjk \
        fonts-ipafont-gothic \
        fonts-ipafont-mincho \
    && rm -rf /var/lib/apt/lists/*

# Firefox（Mozilla 公式 APT リポジトリ）。レンダリング結果の比較用ブラウザとして使う。
# 署名鍵は Mozilla が公開するフィンガープリントと完全一致することを確認してから登録する。
RUN install -d -m 0755 /etc/apt/keyrings \
    && curl -fsSL https://packages.mozilla.org/apt/repo-signing-key.gpg \
        -o /etc/apt/keyrings/packages.mozilla.org.asc \
    && test "$(gpg --batch --quiet --show-keys --with-colons /etc/apt/keyrings/packages.mozilla.org.asc \
        | awk -F: '$1 == "fpr" { print $10; exit }')" = "35BAA0B33E9EB396F59CA838C0BA5CE6DC6315A3" \
    && printf 'Types: deb\nURIs: https://packages.mozilla.org/apt\nSuites: mozilla\nComponents: main\nSigned-By: /etc/apt/keyrings/packages.mozilla.org.asc\n' \
        > /etc/apt/sources.list.d/mozilla.sources \
    && printf 'Package: *\nPin: origin packages.mozilla.org\nPin-Priority: 1000\n' \
        > /etc/apt/preferences.d/mozilla \
    && apt-get update \
    && apt-get install -y --no-install-recommends firefox firefox-l10n-ja \
    && rm -rf /var/lib/apt/lists/*

# GitHub CLI（gh）。PR 作成などの Claude Code / PR ワークフローで使う。
# 公式 apt リポジトリ（cli.github.com/packages）を鍵付きで追加してインストールする。
RUN mkdir -p -m 755 /etc/apt/keyrings \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# 非 root ユーザー dev を作成する（コンテナ内での作業ユーザー）。
# sudo をパスワードなしで全コマンド許可するのは意図的（サンドボックス内での ad-hoc な
# apt install 等の利便性のため）。root 化してもホストへの露出はバインドマウントした
# リポジトリのみで、そこは dev でも書けるため実質的なリスク増はない。
ARG USERNAME=dev
ARG USER_UID=1000
ARG USER_GID=1000
RUN groupadd --gid ${USER_GID} ${USERNAME} \
    && useradd --uid ${USER_UID} --gid ${USER_GID} --create-home --shell /bin/bash ${USERNAME} \
    && echo "${USERNAME} ALL=(ALL) NOPASSWD:ALL" \
        > /etc/sudoers.d/${USERNAME} \
    && chmod 0440 /etc/sudoers.d/${USERNAME}

# Rust の静的解析・フォーマットをコンテナ再作成後も利用できるようにする。
RUN rustup component add clippy rustfmt

# ビルド成果物はホストの target/ と分離した専用の場所に出力する。
ENV CARGO_TARGET_DIR=/target
RUN mkdir -p /target && chown ${USER_UID}:${USER_GID} /target

# cargo の crates.io インデックス / crate ソース / git 依存キャッシュを named volume に
# 永続化する（docker-compose.yaml でマウント）。イメージ側に dev 所有のディレクトリを用意しておくと、
# 新規ボリュームがこの所有者を引き継ぐため、毎回のクレート再ダウンロードを避けられる。
RUN mkdir -p /usr/local/cargo/registry /usr/local/cargo/git \
    && chown ${USER_UID}:${USER_GID} /usr/local/cargo/registry /usr/local/cargo/git

# 起動時に /target と cargo キャッシュボリュームの所有者を自己修復するエントリポイント。
# 古い root 所有の named volume を再マウントしても Permission denied にならないようにする。
# root のうちに COPY + chmod しておく（--chmod は BuildKit 専用のためレガシービルダーでも動くようにする）。
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod 0755 /usr/local/bin/docker-entrypoint.sh

# Claude Code のログイン情報・設定の永続化先（compose の named volume でマウントする）。
ENV CLAUDE_CONFIG_DIR=/home/${USERNAME}/.claude

# Codex CLI のホームディレクトリ（ログイン情報・本体パッケージ）。
# compose の codex-config volume のマウント先および docker-entrypoint.sh の
# 所有者自己修復ループと整合させるため、ENV で明示的に固定する。
ENV CODEX_HOME=/home/${USERNAME}/.codex

# Bash 履歴を compose の named volume に保存し、対話シェル間で即時共有する。
ENV HISTFILE=/home/${USERNAME}/.bash-history/history \
    HISTSIZE=10000 \
    HISTFILESIZE=20000 \
    HISTCONTROL=ignoreboth:erasedups \
    PROMPT_COMMAND="history -a; history -n"

USER ${USERNAME}
WORKDIR /workspace

# CLAUDE_CONFIG_DIR を dev 所有で用意しておく。
# 空の named volume はマウント先ディレクトリの所有者を引き継ぐため、ここで作成しておくと権限問題を避けられる。
RUN mkdir -p ${CLAUDE_CONFIG_DIR}

# SSH 鍵の置き場所（compose の ssh-config volume でマウントして永続化する）。
# ホストの ~/.ssh はマウントせず、コンテナ内で生成した鍵をここに置く運用。
# 空の named volume は所有者・パーミッションを引き継ぐため、dev 所有・700 で用意しておく。
RUN mkdir -p /home/${USERNAME}/.ssh /home/${USERNAME}/.bash-history \
    && chmod 700 /home/${USERNAME}/.ssh /home/${USERNAME}/.bash-history

# Claude Code のネイティブインストーラ。~/.local/bin にインストールされる。
# 公式が推奨するインストール方法であり、チェックサム検証なしで取得する設計判断。
# パイプ（curl | bash）だと途中で curl が失敗しても層が成功しうるため、
# ダウンロードと実行を分離し、同一 RUN 内で claude --version による検証まで行う。
RUN curl -fsSL https://claude.ai/install.sh -o /tmp/claude-install.sh \
    && bash /tmp/claude-install.sh \
    && rm /tmp/claude-install.sh \
    && "$HOME/.local/bin/claude" --version
ENV PATH=/home/${USERNAME}/.local/bin:${PATH}

# Codex CLI（OpenAI）。code-reviewer / codex-specialist エージェント等の codex exec で使う。
# 参考: https://learn.chatgpt.com/docs/codex/cli#getting-started
# 本体は ~/.codex/packages に、ランチャーは ~/.local/bin/codex に入る。
# ~/.codex はログイン情報も含むため named volume（codex-config）で永続化する。
# Claude Code と同様、公式推奨のインストーラをチェックサム検証なしで取得する設計判断。
# パイプ（curl | sh）は途中の curl 失敗を握りつぶすため、ダウンロードと実行を分離し、
# 同一 RUN 内で codex --version による検証まで行う。
RUN curl -fsSL https://chatgpt.com/codex/install.sh -o /tmp/codex-install.sh \
    && sh /tmp/codex-install.sh \
    && rm /tmp/codex-install.sh \
    && "$HOME/.local/bin/codex" --version

# colima のバインドマウントはコンテナ内で root 所有に見えるため、
# git が "dubious ownership" でリポジトリを拒否しないよう safe.directory を設定する。
RUN git config --global --add safe.directory /workspace

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

CMD ["bash"]
