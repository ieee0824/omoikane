# 開発用サンドボックス
#
# ホスト環境を汚さずに omoikane のビルド・テスト・Claude Code 実行を行うためのコンテナ。
# ビルド・テストは CI（.github/workflows/ci.yml）と揃えた環境で再現できるようにする。
FROM rust:1-bookworm

# aws-lc-sys（rustls 0.23 の依存）のビルドに cmake が必須。
# build-essential / pkg-config は rusqlite（bundled SQLite）などのネイティブビルドに使う。
# build-essential / pkg-config / git / curl / ca-certificates / gpg は rust:1-bookworm
# （buildpack-deps ベース）に既に含まれるが、ベースイメージの変化に備えて明示的に指定する。
# フォントは CI と同一のもの（fonts-dejavu / fonts-liberation / fonts-noto-core）に加え、
# CJK レンダリング用に fonts-noto-cjk を追加する。
# ripgrep（rg）はシェルでのコード検索に、gpg は gh の apt リポジトリ鍵の検証に使う。
RUN apt-get update && apt-get install -y --no-install-recommends \
        cmake \
        pkg-config \
        build-essential \
        git \
        curl \
        ca-certificates \
        gpg \
        sudo \
        ripgrep \
        fonts-dejavu \
        fonts-liberation \
        fonts-noto-core \
        fonts-noto-cjk \
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

USER ${USERNAME}
WORKDIR /workspace

# CLAUDE_CONFIG_DIR を dev 所有で用意しておく。
# 空の named volume はマウント先ディレクトリの所有者を引き継ぐため、ここで作成しておくと権限問題を避けられる。
RUN mkdir -p ${CLAUDE_CONFIG_DIR}

# Claude Code のネイティブインストーラ。~/.local/bin にインストールされる。
# 公式が推奨するインストール方法であり、チェックサム検証なしで取得する設計判断。
# パイプ（curl | bash）だと途中で curl が失敗しても層が成功しうるため、
# ダウンロードと実行を分離し、同一 RUN 内で claude --version による検証まで行う。
RUN curl -fsSL https://claude.ai/install.sh -o /tmp/claude-install.sh \
    && bash /tmp/claude-install.sh \
    && rm /tmp/claude-install.sh \
    && "$HOME/.local/bin/claude" --version
ENV PATH=/home/${USERNAME}/.local/bin:${PATH}

# colima のバインドマウントはコンテナ内で root 所有に見えるため、
# git が "dubious ownership" でリポジトリを拒否しないよう safe.directory を設定する。
RUN git config --global --add safe.directory /workspace

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

CMD ["bash"]
