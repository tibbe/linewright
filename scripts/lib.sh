# shellcheck shell=bash
# https://doc.rust-lang.org/cargo/reference/config.html#buildwarnings
export CARGO_BUILD_WARNINGS=deny

# Fails, saying how to install it, when `tool`, one scripts/install installs,
# is not on the PATH.
need() {
  command -v "$1" >/dev/null || {
    echo "$1 is not on the PATH: run \`scripts/install $1\`"
    return 1
  }
}
