# syntax=docker/dockerfile:1

FROM runtime
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev curl procps \
    && rm -rf /var/lib/apt/lists/*
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
# Agent sessions are given only the variables `session-env` names besides
# the basic ones, and their builds need these, as does the base image's
# pinned Claude Code.
ENV LINEWRIGHT_SESSION_ENV=RUSTUP_HOME,CARGO_HOME,RUSTC_WRAPPER,SCCACHE_DIR,DISABLE_AUTOUPDATER
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
