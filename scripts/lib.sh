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

# shellcheck disable=SC2034 # the attribute of a test that is not #[test_log::test], read by scripts/check and scripts/fix
test_attribute='^[ \t]*#\[(tokio::)?test(\(.*\))?\][ \t]*$'
