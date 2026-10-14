/// The command Linewright runs as (`docs/specs/operation.md`), which names
/// the program in its help and starts the errors it prints itself.
pub const COMMAND: &str = "lw";
/// The version Linewright is built as.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The commit Linewright is built from, abbreviated, or `unknown` (see
/// `build.rs`).
pub const COMMIT: &str = env!("LINEWRIGHT_COMMIT");
/// The version and the commit, as `--version` names them.
pub const VERSION_AND_COMMIT: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("LINEWRIGHT_COMMIT"),
    ")"
);

pub mod api;
pub mod briefs;
pub mod case;
pub mod clock;
pub mod config;
pub mod domain;
pub mod explain;
pub mod factory;
pub mod git;
pub mod logs;
pub mod marker;
pub mod prompts;
pub mod reconcile;
mod records;
pub mod results;
pub mod run;
pub mod runs;
pub mod runtime;
pub mod spec_gap;
pub mod spec_glob;
pub mod status;
pub mod store;
pub mod tracking;
pub mod transcript;
pub mod webhook;
pub mod workflow;
