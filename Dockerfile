# syntax=docker/dockerfile:1

ARG DEBIAN_RELEASE=trixie
# To bump a pin, change its version here and its two sha256s in the tools
# stages, gh's from
# https://github.com/cli/cli/releases/download/v<version>/gh_<version>_checksums.txt
# and Claude Code's from
# https://downloads.claude.ai/claude-code-releases/<version>/manifest.json
# (platforms.linux-x64 and linux-arm64, .checksum).
ARG CLAUDE_CODE_VERSION=2.1.287 GH_VERSION=2.102.1

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
    --mount=type=bind,source=templates,target=templates \
    --mount=type=bind,source=assets,target=assets \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
    && cp target/release/lw /usr/local/bin/lw

# gh's apt repository keeps only its newest release and Claude Code's only
# its newest few, so a pin there breaks once newer ones come out. Their
# releases stay downloadable, each architecture's checked by its sha256.
FROM scratch AS tools-amd64
ARG GH_VERSION CLAUDE_CODE_VERSION
ADD --checksum=sha256:5b91c75d971509e96f673fa9f46e5d8ea294b6bbfedbc8ff458aedd88775d94c \
    https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_amd64.deb /gh.deb
ADD --chmod=755 --checksum=sha256:3920489a5109cff5786a1a392c25277408ff22bc796d5edb9c16a60e5a1718f0 \
    https://downloads.claude.ai/claude-code-releases/${CLAUDE_CODE_VERSION}/linux-x64/claude /claude

FROM scratch AS tools-arm64
ARG GH_VERSION CLAUDE_CODE_VERSION
ADD --checksum=sha256:69a7b86119e9e21f8f4fd6a605386d53fd41625100f3676d93ea9b5a5b176cee \
    https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_arm64.deb /gh.deb
ADD --chmod=755 --checksum=sha256:e4daf793d1e74fb0d9874dd09e98690bbfd7be515f78a87fd05b9e2b4bb33b03 \
    https://downloads.claude.ai/claude-code-releases/${CLAUDE_CODE_VERSION}/linux-arm64/claude /claude

FROM tools-${TARGETARCH} AS tools

FROM debian:${DEBIAN_RELEASE}-slim AS runtime
RUN --mount=type=bind,from=tools,source=/gh.deb,target=/tmp/gh.deb \
    apt-get update \
    && apt-get install -y --no-install-recommends \
        bash ca-certificates git jq python3 ripgrep tini /tmp/gh.deb \
    && rm -rf /var/lib/apt/lists/*
COPY --link --from=tools /claude /usr/local/bin/claude
# Fails the build when a pin did not take effect.
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
