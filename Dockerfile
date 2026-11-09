# syntax=docker/dockerfile:1

ARG DEBIAN_RELEASE=trixie
# The sha256s in the tools stages are gh's from
# https://github.com/cli/cli/releases/download/v<version>/gh_<version>_checksums.txt
# and Claude Code's from
# https://downloads.claude.ai/claude-code-releases/<version>/manifest.json
# (platforms.linux-x64 and linux-arm64, .checksum).
ARG CLAUDE_CODE_VERSION=2.1.311 GH_VERSION=2.103.3

FROM rust:1-slim-${DEBIAN_RELEASE} AS build
WORKDIR /src
COPY rust-toolchain.toml ./
RUN rustup toolchain install
ARG LINEWRIGHT_COMMIT
RUN --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=build.rs,target=build.rs \
    --mount=type=bind,source=src,target=src \
    --mount=type=bind,source=briefs,target=briefs \
    --mount=type=bind,source=templates,target=templates \
    --mount=type=bind,source=assets,target=assets \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
    && cp target/release/lw /usr/local/bin/lw

FROM scratch AS tools-amd64
ARG GH_VERSION CLAUDE_CODE_VERSION
ADD --checksum=sha256:e3bafef96328d1a67de3a968bdfe1cc2ffd58cf6aba8b5e201356897ed06bb3d \
    https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_amd64.deb /gh.deb
ADD --chmod=755 --checksum=sha256:86dc1fcc69498159eaf15f446613e847010f9036bfbbdffd4c49350be8489ada \
    https://downloads.claude.ai/claude-code-releases/${CLAUDE_CODE_VERSION}/linux-x64/claude /claude

FROM scratch AS tools-arm64
ARG GH_VERSION CLAUDE_CODE_VERSION
ADD --checksum=sha256:cac3ff84b60b68e64cca69f31f303210c26320361cc3286b773655d4226b7b06 \
    https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_arm64.deb /gh.deb
ADD --chmod=755 --checksum=sha256:2e2496c9a8065df93acb0fca2d341f59a6b8777649b2e025ab8c9bcd5eef61f3 \
    https://downloads.claude.ai/claude-code-releases/${CLAUDE_CODE_VERSION}/linux-arm64/claude /claude

FROM tools-${TARGETARCH} AS tools

FROM debian:${DEBIAN_RELEASE}-slim AS runtime
RUN --mount=type=bind,from=tools,source=/gh.deb,target=/tmp/gh.deb \
    apt-get update \
    && apt-get install -y --no-install-recommends \
        bash ca-certificates git jq python3 ripgrep tini /tmp/gh.deb \
    && rm -rf /var/lib/apt/lists/*
COPY --link --from=tools /claude /usr/local/bin/claude
ARG GH_VERSION CLAUDE_CODE_VERSION
RUN gh --version | grep -qF "gh version ${GH_VERSION} " \
    && claude --version | grep -qF "${CLAUDE_CODE_VERSION} "
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
