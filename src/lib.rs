pub const COMMAND: &str = "lw";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The commit Linewright is built from, abbreviated, or `unknown` (see
/// `build.rs`).
pub const COMMIT: &str = env!("LINEWRIGHT_COMMIT");
pub const VERSION_AND_COMMIT: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("LINEWRIGHT_COMMIT"),
    ")"
);

pub mod api;
pub mod automation;
pub mod briefs;
pub mod case;
pub mod clock;
pub mod config;
pub mod credentials;
pub mod domain;
pub mod explain;
pub mod factory;
pub mod git;
pub mod holds;
pub mod logs;
pub mod prompts;
pub mod reconcile;
mod records;
pub mod redaction;
pub mod results;
pub mod run;
pub mod runs;
pub mod runtime;
pub mod session_env;
pub mod spec_gap;
pub mod spec_glob;
pub mod status;
pub mod store;
pub mod tracking;
pub mod transcript;
pub mod webhook;
