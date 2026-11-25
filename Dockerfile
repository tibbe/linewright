# syntax=docker/dockerfile:1

ARG DEBIAN_RELEASE=trixie
# The sha256s in the tools stages are gh's from
# https://github.com/cli/cli/releases/download/v<version>/gh_<version>_checksums.txt
# and Claude Code's from
# https://downloads.claude.ai/claude-code-releases/<version>/manifest.json
# (platforms.linux-x64 and linux-arm64, .checksum).
ARG CLAUDE_CODE_VERSION=2.1.323 GH_VERSION=2.105.0

FROM rust:1-slim-${DEBIAN_RELEASE} AS build
WORKDIR /src
COPY rust-toolchain.toml ./
RUN rustup toolchain install
ARG LINEWRIGHT_COMMIT
RUN --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=crates,target=crates \
    --mount=type=bind,source=briefs,target=briefs \
    --mount=type=bind,source=docs/specs/assets,target=docs/specs/assets \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
    && cp target/release/lw /usr/local/bin/lw

FROM scratch AS tools-amd64
ARG GH_VERSION CLAUDE_CODE_VERSION
ADD --checksum=sha256:1dac939af3e84fb3861927c984b357531341be9403b889e4aa86e8822145a09d \
    https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_amd64.deb /gh.deb
ADD --chmod=755 --checksum=sha256:604d29a6869409106b6a1110bdadcc8b72e8654a5a865d23258f938a696e348e \
    https://downloads.claude.ai/claude-code-releases/${CLAUDE_CODE_VERSION}/linux-x64/claude /claude

FROM scratch AS tools-arm64
ARG GH_VERSION CLAUDE_CODE_VERSION
ADD --checksum=sha256:d17e9b089ce0356679c9fa1898f3a7d38dccf943d529acaa94c24e8a4584e0ae \
    https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_arm64.deb /gh.deb
ADD --chmod=755 --checksum=sha256:452dddff140b5caf00a5410333a3d6db82c73ea2dec74b1541fae18e97d47dca \
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
