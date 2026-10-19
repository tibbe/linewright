# syntax=docker/dockerfile:1

ARG DEBIAN_RELEASE=trixie

FROM rust:1-slim-${DEBIAN_RELEASE} AS build
WORKDIR /src
COPY rust-toolchain.toml ./
RUN rustup toolchain install
# The commit the binary is built from, which it reports: the build context
# has no `.git` to read it from.
ARG LINEWRIGHT_COMMIT
RUN --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=build.rs,target=build.rs \
    --mount=type=bind,source=src,target=src \
    --mount=type=bind,source=briefs,target=briefs \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
    && cp target/release/lw /usr/local/bin/lw

FROM debian:${DEBIAN_RELEASE}-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates git python3 tini \
    && rm -rf /var/lib/apt/lists/*
ADD --checksum=sha256:bd70a5e4a268002704024ceba7f8446024114e94f3f0bdd11c23a9e592be81c6 \
    https://downloads.claude.ai/keys/claude-code.asc /etc/apt/keyrings/claude-code.asc
ADD --checksum=sha256:6084d5d7bd8e288441e0e94fc6275570895da18e6751f70f057485dc2d1a811b \
    https://cli.github.com/packages/githubcli-archive-keyring.gpg /etc/apt/keyrings/githubcli.gpg
ARG CLAUDE_CODE_VERSION=2.1.287 GH_VERSION=2.102.0
RUN <<EOF
set -eu
chmod 644 /etc/apt/keyrings/claude-code.asc /etc/apt/keyrings/githubcli.gpg
echo "deb [signed-by=/etc/apt/keyrings/claude-code.asc] https://downloads.claude.ai/claude-code/apt/stable stable main" \
  > /etc/apt/sources.list.d/claude-code.list
echo "deb [signed-by=/etc/apt/keyrings/githubcli.gpg] https://cli.github.com/packages stable main" \
  > /etc/apt/sources.list.d/github-cli.list
apt-get update
apt-get install -y --no-install-recommends \
  "claude-code=${CLAUDE_CODE_VERSION}-1" "gh=${GH_VERSION}"
rm -rf /var/lib/apt/lists/*
EOF
ARG LINEWRIGHT_UID=1000 LINEWRIGHT_GID=1000
RUN (getent group "$LINEWRIGHT_GID" >/dev/null || groupadd --gid "$LINEWRIGHT_GID" linewright) \
    && useradd --uid "$LINEWRIGHT_UID" --gid "$LINEWRIGHT_GID" \
        --create-home --shell /bin/bash linewright
USER linewright
WORKDIR /home/linewright
ENTRYPOINT ["tini", "--", "lw"]
CMD ["serve"]

FROM runtime AS base
COPY --link --from=build /usr/local/bin/lw /usr/local/bin/lw
LABEL org.opencontainers.image.source="https://github.com/tibbe/linewright" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.description="Linewright, Claude Code, git, gh, and python3"
