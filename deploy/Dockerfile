# syntax=docker/dockerfile:1

# Playwright MCP and Playwright, as the lockfile pins them. npm, with the
# hundreds of packages Debian splits it into, stays in this stage.
FROM runtime AS playwright
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends nodejs npm \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/playwright
COPY deploy/playwright/package.json deploy/playwright/package-lock.json ./
# Fails the build unless the one playwright installed is the version
# @playwright/mcp uses, so the browser installed below is the one it drives.
RUN npm ci --no-audit --no-fund \
    && test ! -e node_modules/@playwright/mcp/node_modules \
    && node -e ' \
        const want = require("@playwright/mcp/package.json").dependencies.playwright; \
        const got = require("playwright/package.json").version; \
        if (got !== want) throw new Error(`playwright ${got}, @playwright/mcp wants ${want}`);'

FROM runtime
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev curl procps nodejs \
    && rm -rf /var/lib/apt/lists/*
# A headless Chromium that sessions drive through Playwright MCP, to see the
# pages they serve on localhost. The browser is the revision the pinned
# Playwright expects, read-only to the agents. Both of its builds are
# installed: Playwright MCP runs the full Chromium, and a script's
# chromium.launch() with no channel runs the headless shell.
COPY --link --from=playwright /opt/playwright /opt/playwright
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright \
    NODE_PATH=/opt/playwright/node_modules
# fonts-dejavu-core: without it, Chromium resolves the generic monospace and
# system-ui families to WenQuanYi Zen Hei, one of the CJK fonts --with-deps
# installs, so Latin text set in them renders in a CJK font.
RUN ln -s "$NODE_PATH/playwright/cli.js" /usr/local/bin/playwright \
    && playwright install --with-deps chromium \
    && apt-get install -y --no-install-recommends fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*
COPY --chmod=755 deploy/playwright-mcp /usr/local/bin/playwright-mcp
# Takes exclusive control of MCP: in this image, a repository's .mcp.json,
# plugins' servers and --mcp-config don't load.
# https://code.claude.com/docs/en/mcp#managed-mcp-configuration
COPY deploy/managed-mcp.json /etc/claude-code/managed-mcp.json
# sccache, so each session's fresh clone compiles the dependencies from the
# cache that earlier sessions filled, not from scratch.
ARG SCCACHE_VERSION=0.18.0 TARGETARCH
RUN <<EOF
set -eu
case "$TARGETARCH" in
amd64) target=x86_64 sha=45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89 ;;
arm64) target=aarch64 sha=2b3284d5da3b46a47dc4229e75bb7b88ac4aa99c8d754fb7d2f84997e5a4354a ;;
*) echo "sccache: no release for $TARGETARCH" >&2; exit 1 ;;
esac
name="sccache-v$SCCACHE_VERSION-$target-unknown-linux-musl"
curl --proto '=https' --tlsv1.2 -sSfL -o /tmp/sccache.tar.gz \
  "https://github.com/mozilla/sccache/releases/download/v$SCCACHE_VERSION/$name.tar.gz"
echo "$sha  /tmp/sccache.tar.gz" | sha256sum -c -
mkdir -p /usr/local/libexec
tar -xzf /tmp/sccache.tar.gz -C /usr/local/libexec --strip-components=1 "$name/sccache"
rm /tmp/sccache.tar.gz
EOF
COPY --chmod=755 deploy/sccache /usr/local/bin/sccache
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    RUSTC_WRAPPER=sccache SCCACHE_DIR=/var/cache/sccache
ENV LINEWRIGHT_SESSION_PASS_ENV=RUSTUP_HOME,CARGO_HOME,RUSTC_WRAPPER,SCCACHE_DIR,PLAYWRIGHT_BROWSERS_PATH,NODE_PATH
RUN install -d -o linewright -g "$(id -g linewright)" \
    "$RUSTUP_HOME" "$CARGO_HOME" "$CARGO_HOME/registry" "$SCCACHE_DIR" /data
USER linewright
RUN install -d -m 700 /home/linewright/.claude
COPY rust-toolchain.toml /tmp/toolchain/
ARG RUSTUP_VERSION=1.29.1
RUN curl --proto '=https' --tlsv1.2 -sSf -o /tmp/rustup-init.sh https://sh.rustup.rs \
    && RUSTUP_VERSION=$RUSTUP_VERSION sh /tmp/rustup-init.sh -y --no-modify-path \
        --profile minimal --default-toolchain none \
    && rm /tmp/rustup-init.sh \
    && cd /tmp/toolchain && rustup toolchain install
COPY linewright.toml ./
COPY --link --from=base /usr/local/bin/lw /usr/local/bin/lw
