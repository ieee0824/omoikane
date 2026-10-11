# 開発用サンドボックス
#
# ホスト環境を汚さずに omoikane のビルド・テスト・Claude Code 実行を行うためのコンテナ。
# ビルド・テストは CI（.github/workflows/ci.yml）と揃えた環境で再現できるようにする。
FROM rust:1-bookworm

# aws-lc-sys（rustls 0.23 の依存）のビルドに cmake が必須。
# build-essential / pkg-config は rusqlite（bundled SQLite）などのネイティブビルドに使う。
# lld はLinux上のRust test/releaseリンクを短縮する。Cargoのtarget別RUSTFLAGSは後段で設定する。
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
        lld \
        git \
        curl \
        ca-certificates \
        gpg \
        openssh-client \
        sudo \
        ripgrep \
        jq \
        vim \
        fonts-dejavu \
        fonts-liberation \
        fonts-noto-core \
        fonts-noto-cjk \
        fonts-ipafont-gothic \
        fonts-ipafont-mincho \
    && rm -rf /var/lib/apt/lists/*

# GUI（--features gui）の X11 E2E テスト用。CI（.github/workflows/gui-pointer-lock.yml）と同じ
# xvfb / xdotool / x11-utils / openbox / libxkbcommon-x11-0 / python3-pil に加え、
# winit が実行時に dlopen する X11 ライブラリ（Xcursor / Xrandr / Xi）と、
# scripts/test-pointer-lock-x11.py が ctypes で読み込む libX11 / libXfixes を明示する。
# 実行例は tests/README.md の GUI 節（scripts/test-gui-x11.py 等）を参照。
RUN apt-get update && apt-get install -y --no-install-recommends \
        xvfb \
        xdotool \
        x11-utils \
        openbox \
        libx11-6 \
        libxcursor1 \
        libxrandr2 \
        libxi6 \
        libxfixes3 \
        libxkbcommon-x11-0 \
        python3-pil \
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

# Chrome for Testing と同版の ChromeDriver。WPT・描画の参照ブラウザ用。
# バージョン更新時は、両アーキテクチャの Chrome / ChromeDriver の SHA256 も更新する。
# 配布物の chrome_sandbox に標準名 chrome-sandbox を付け、非 root でも sandbox を有効にする。
ARG CHROME_VERSION=155.0.8059.39
RUN apt-get update && apt-get install -y --no-install-recommends \
        libasound2 \
        libatk-bridge2.0-0 \
        libatk1.0-0 \
        libcups2 \
        libdrm2 \
        libgbm1 \
        libgtk-3-0 \
        libnspr4 \
        libnss3 \
        libxcomposite1 \
        libxdamage1 \
        libxext6 \
        libxkbcommon0 \
        libxrandr2 \
        unzip \
    && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
        amd64) \
            chrome_platform=linux64; \
            chrome_sha256=55672d1f392fd3e7b7a08621b6e804e6bcb39d40cf155504abb74b3a021ea8ea; \
            chromedriver_sha256=a65f025c692e686ec2a21fa6b9639c69fa9813683a47af9cabf1ba0b1f453e42 ;; \
        arm64) \
            chrome_platform=linux-arm64; \
            chrome_sha256=b9d44e5d183260ca941a4c4d8d21c8a437d81ef778a489047ef98968b0475dc5; \
            chromedriver_sha256=e375914dc2ee52c38ffc6db4ee8276b462a2872f94ca458c18d787e0f761b129 ;; \
        *) echo "Unsupported Chrome architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    chrome_tmp=$(mktemp -d); \
    trap 'rm -rf "$chrome_tmp"' EXIT; \
    chrome_base_url="https://storage.googleapis.com/chrome-for-testing-public/${CHROME_VERSION}/${chrome_platform}"; \
    curl -fsSL "${chrome_base_url}/chrome-${chrome_platform}.zip" -o "$chrome_tmp/chrome.zip"; \
    curl -fsSL "${chrome_base_url}/chromedriver-${chrome_platform}.zip" -o "$chrome_tmp/chromedriver.zip"; \
    printf '%s  %s\n%s  %s\n' \
        "$chrome_sha256" "$chrome_tmp/chrome.zip" \
        "$chromedriver_sha256" "$chrome_tmp/chromedriver.zip" | sha256sum -c -; \
    install -d -m 0755 /opt/chrome-for-testing; \
    unzip -q "$chrome_tmp/chrome.zip" -d /opt/chrome-for-testing; \
    unzip -q "$chrome_tmp/chromedriver.zip" -d /opt/chrome-for-testing; \
    mv "/opt/chrome-for-testing/chrome-${chrome_platform}" /opt/chrome-for-testing/chrome; \
    mv "/opt/chrome-for-testing/chromedriver-${chrome_platform}" /opt/chrome-for-testing/chromedriver; \
    chown root:root /opt/chrome-for-testing/chrome/chrome_sandbox; \
    chmod 4755 /opt/chrome-for-testing/chrome/chrome_sandbox; \
    ln -s chrome_sandbox /opt/chrome-for-testing/chrome/chrome-sandbox; \
    ln -s /opt/chrome-for-testing/chrome/chrome /usr/local/bin/google-chrome; \
    ln -s /opt/chrome-for-testing/chrome/chrome /usr/local/bin/chrome; \
    ln -s /opt/chrome-for-testing/chromedriver/chromedriver /usr/local/bin/chromedriver; \
    google-chrome --version; \
    chromedriver --version

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

# Linux x86_64/aarch64の開発ビルドだけlldを使う。target別に設定し、将来この
# コンテナから別targetへcross compileする場合のリンカー選択には影響させない。
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=lld" \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=lld"

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
