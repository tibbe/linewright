# shellcheck shell=bash
# https://doc.rust-lang.org/cargo/reference/config.html#buildwarnings
export CARGO_BUILD_WARNINGS=deny

rumdl_version=0.2.79
compose_version=5.6.0
nextest_version=0.9.148
tailwind_version=4.3.3

apalache_version=0.62.4
quint_evaluator_version=v0.7.1
temurin_version=21.0.13+9

has_sha256() {
  local actual
  actual="$({ sha256sum "$1" 2>/dev/null || shasum -a 256 "$1"; } | cut -d' ' -f1)"
  if [ "$actual" != "$2" ]; then
    echo "$1 has sha256 $actual, not $2"
    return 1
  fi
}

# Installs the download at `url`, if it has sha256 `sha`, as `dir/name`: the
# file `member` in it when it is a tar.gz, or the download itself.
install_tool() {
  local dir=$1 name=$2 url=$3 sha=$4 member=${5:-} tmp
  tmp="$(mktemp -d)"
  if curl -fsSL -o "$tmp/download" "$url" &&
    has_sha256 "$tmp/download" "$sha" &&
    if [ -n "$member" ]; then
      tar -xzf "$tmp/download" -C "$tmp" && mv "$tmp/$member" "$tmp/download"
    fi &&
    chmod +x "$tmp/download" &&
    mkdir -p "$dir" &&
    mv "$tmp/download" "$dir/$name.$$" &&
    mv -f "$dir/$name.$$" "$dir/$name"; then
    rm -rf "$tmp"
  else
    rm -rf "$tmp"
    return 1
  fi
}

no_release() {
  echo "$1 has no release for $(uname -sm)"
  return 1
}

# Installs the directory `member` of the tar.gz at `url`, if it has sha256
# `sha`, as `dir/member`.
install_tree() {
  local dir=$1 member=$2 url=$3 sha=$4 tmp
  tmp="$(mktemp -d)"
  if curl -fsSL -o "$tmp/download" "$url" &&
    has_sha256 "$tmp/download" "$sha" &&
    mkdir "$tmp/tree" && tar -xzf "$tmp/download" -C "$tmp/tree" &&
    mkdir -p "$dir" &&
    { [ -e "$dir/$member" ] || mv "$tmp/tree/$member" "$dir/$member"; }; then
    rm -rf "$tmp"
  else
    rm -rf "$tmp"
    return 1
  fi
}

rumdl() {
  local dir="${XDG_CACHE_HOME:-$HOME/.cache}/linewright/rumdl-$rumdl_version" target sha archive
  if [ ! -x "$dir/rumdl" ]; then
    case "$(uname -sm)" in
    "Darwin arm64") target=aarch64-apple-darwin sha=8c0ecf93a61bf612df5f6c6f22813d066a6f51a5680f3c12da49b93ed892aebb ;;
    "Darwin x86_64") target=x86_64-apple-darwin sha=09cca63968f9a3f35faab71ac8d09bda1a96ba4e0e38cac54bfb7b544523075f ;;
    "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-musl sha=f2ccb236a6b62b9980711a2f54c1e0d8d91ceaea5d7313d0b1a615237b3130bf ;;
    "Linux x86_64") target=x86_64-unknown-linux-musl sha=a85ae77c6880616118e10dda1e36292df59815bcfbf767201736ff6971213cf3 ;;
    *) no_release "rumdl $rumdl_version" || return 1 ;;
    esac
    archive="rumdl-v$rumdl_version-$target.tar.gz"
    install_tool "$dir" rumdl \
      "https://github.com/rvben/rumdl/releases/download/v$rumdl_version/$archive" \
      "$sha" rumdl || return 1
  fi
  "$dir/rumdl" "$@"
}

compose() {
  local dir="${XDG_CACHE_HOME:-$HOME/.cache}/linewright/compose-$compose_version" target sha
  if [ ! -x "$dir/docker-compose" ]; then
    case "$(uname -sm)" in
    "Darwin arm64") target=darwin-aarch64 sha=bd714a42b46e51757fc1121085b3096b9064e3f80d3ca5a991e33f44df4f43c9 ;;
    "Darwin x86_64") target=darwin-x86_64 sha=c76e3abf5fde1aeb3bd7c7306ff15bef2246afc1f9e54832a0a050dd7cdfb2f8 ;;
    "Linux aarch64" | "Linux arm64") target=linux-aarch64 sha=733ec76717ceb59052a9609b9dadfb523b2df8eab57a54212872d10a58078ea2 ;;
    "Linux x86_64") target=linux-x86_64 sha=40343e21ca777173e69cff5dbafeb37c6f81f3b0d57d9e597f036e95eb63e76a ;;
    *) no_release "Docker Compose $compose_version" || return 1 ;;
    esac
    install_tool "$dir" docker-compose \
      "https://github.com/docker/compose/releases/download/v$compose_version/docker-compose-$target" \
      "$sha" || return 1
  fi
  "$dir/docker-compose" "$@"
}

nextest() {
  local dir="${XDG_CACHE_HOME:-$HOME/.cache}/linewright/nextest-$nextest_version" target sha archive
  if [ ! -x "$dir/cargo-nextest" ]; then
    case "$(uname -sm)" in
    "Darwin arm64" | "Darwin x86_64") target=universal-apple-darwin sha=6c23b7fb4ca82c571fc8dfedbab45bfa773d45918f933170c85d21cd7f4b852b ;;
    "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-musl sha=94676506ceaed01746d78cfe9ed124ee841b2932fd18d9d10f438a347bd9ba5e ;;
    "Linux x86_64") target=x86_64-unknown-linux-musl sha=bf92cb81450872d960ffa13857a83084d557ec0cdc141663d263bb23ef756f97 ;;
    *) no_release "cargo-nextest $nextest_version" || return 1 ;;
    esac
    archive="cargo-nextest-$nextest_version-$target.tar.gz"
    install_tool "$dir" cargo-nextest \
      "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$nextest_version/$archive" \
      "$sha" cargo-nextest || return 1
  fi
  "$dir/cargo-nextest" nextest "$@"
}

tailwind() {
  local dir="${XDG_CACHE_HOME:-$HOME/.cache}/linewright/tailwind-$tailwind_version" target sha
  if [ ! -x "$dir/tailwindcss" ]; then
    case "$(uname -sm)" in
    "Darwin arm64") target=macos-arm64 sha=cdf646702987a743464dff4d9c60fd4480d1c1e73dd819a9a67f1078815dce9d ;;
    "Darwin x86_64") target=macos-x64 sha=7922e0953f2110c05976e3bf58f14e643d90427575e766b7d433f5f80cbee7e1 ;;
    "Linux aarch64" | "Linux arm64") target=linux-arm64 sha=55fd0b241214eff3de1e8ee4f22796662f2d2e7a49bcfca7477cfd0bac398195 ;;
    "Linux x86_64") target=linux-x64 sha=dc61b3ac6b8c9ca874c0cc4c57b2409791a64c5540404ca5f5367360babc313a ;;
    *) no_release "Tailwind CSS $tailwind_version" || return 1 ;;
    esac
    install_tool "$dir" tailwindcss \
      "https://github.com/tailwindlabs/tailwindcss/releases/download/v$tailwind_version/tailwindcss-$target" \
      "$sha" || return 1
  fi
  "$dir/tailwindcss" "$@"
}

# The bin directory of a Java 21 runtime.
java21() {
  local dir="${XDG_CACHE_HOME:-$HOME/.cache}/linewright/temurin-jre-$temurin_version" target sha
  local tree="jdk-$temurin_version-jre" home
  case "$(uname -s)" in
  Darwin) home="$dir/$tree/Contents/Home" ;;
  *) home="$dir/$tree" ;;
  esac
  if [ ! -x "$home/bin/java" ]; then
    case "$(uname -sm)" in
    "Darwin arm64") target=aarch64_mac sha=225bab0a09491ef06835488cd942f5d2befa9a0410a2484f0c8bbcf7506ec09a ;;
    "Darwin x86_64") target=x64_mac sha=c909674b1c10c261b9a3fb7e9c0efebd303e33f88a4fe84be7135b5e2d711f89 ;;
    "Linux aarch64" | "Linux arm64") target=aarch64_linux sha=b1734db7ade49c3f6acc6ae06233b8c2dd90e3e2f00f67000ae1c972aaa64d8c ;;
    "Linux x86_64") target=x64_linux sha=a535a1df9c6284d9618872984e026ae0352d1f7e9e0605425c0e6114f47351dd ;;
    *) no_release "Temurin $temurin_version" || return 1 ;;
    esac
    install_tree "$dir" "$tree" \
      "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-${temurin_version/+/%2B}/OpenJDK21U-jre_${target}_hotspot_${temurin_version/+/_}.tar.gz" \
      "$sha" >&2 || return 1
  fi
  echo "$home/bin"
}

# Quint, from the model's lockfile, with Apalache and the Rust evaluator where
# Quint looks for them (QUINT_HOME), so that it fetches nothing itself.
quint() {
  local lock=docs/model/package-lock.json cache="${XDG_CACHE_HOME:-$HOME/.cache}/linewright"
  local dir home="$cache/quint-home" target sha bin tmp status
  dir="$cache/quint-$({ sha256sum "$lock" 2>/dev/null || shasum -a 256 "$lock"; } | cut -c1-16)"
  if [ ! -x "$dir/node_modules/.bin/quint" ]; then
    tmp="$(mktemp -d)"
    cp docs/model/package.json "$lock" "$tmp/"
    if (cd "$tmp" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error) &&
      mkdir -p "$cache" && { [ -e "$dir" ] || mv "$tmp" "$dir"; }; then
      rm -rf "$tmp"
    else
      rm -rf "$tmp"
      return 1
    fi
  fi
  if [ ! -e "$home/apalache-dist-$apalache_version/apalache/lib/apalache.jar" ]; then
    install_tree "$home/apalache-dist-$apalache_version" apalache \
      "https://github.com/apalache-mc/apalache/releases/download/v$apalache_version/apalache.tgz" \
      8ac8488422c30d552735bae11b92a2d014d8e02e8608702acc08f9a2d8a9abb2 || return 1
  fi
  if [ ! -x "$home/rust-evaluator-$quint_evaluator_version/quint_evaluator" ]; then
    case "$(uname -sm)" in
    "Darwin arm64") target=aarch64-apple-darwin sha=62a7b5226bbc73c738cce94a265b1f87d062e3d1b30a16f7210132f00d8b95b2 ;;
    "Darwin x86_64") target=x86_64-apple-darwin sha=b4c2a706cad360151d08b5ce1577e23859a47b4e16ad11e53dbe8de7e5c450ca ;;
    "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-gnu sha=706819d56eb2153a12b7d3c07b5060917cf5f4431bf5069f998f9ff1e97c4388 ;;
    "Linux x86_64") target=x86_64-unknown-linux-gnu sha=ce0502bdef65dc26c242a9e56379a9bb26de6f3db38f0e3989339d46a542f5eb ;;
    *) no_release "Quint's evaluator $quint_evaluator_version" || return 1 ;;
    esac
    install_tool "$home/rust-evaluator-$quint_evaluator_version" quint_evaluator \
      "https://github.com/quint-co/quint/releases/download/evaluator/$quint_evaluator_version/quint_evaluator-$target.tar.gz" \
      "$sha" quint_evaluator || return 1
  fi
  bin="$(java21)" || return 1
  # Apalache writes its output under the directory it runs in:
  # https://apalache-mc.org/docs/apalache/running.html#4-detailed-output
  tmp="$(mktemp -d)"
  (cd "$tmp" && QUINT_HOME="$home" PATH="$bin:$PATH" "$dir/node_modules/.bin/quint" "$@")
  status=$?
  rm -rf "$tmp"
  return "$status"
}

# shellcheck disable=SC2034 # the attribute of a test that is not #[test_log::test], read by scripts/check and scripts/fix
test_attribute='^[ \t]*#\[(tokio::)?test(\(.*\))?\][ \t]*$'
