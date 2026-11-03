pub const COMMAND: &str = "lw";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The commit Linewright is built from, abbreviated, or `unknown`.
pub const COMMIT: &str = env!("LINEWRIGHT_COMMIT");
pub const VERSION_AND_COMMIT: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("LINEWRIGHT_COMMIT"),
    ")"
);

pub mod api;
pub mod apply;
pub mod attachments;
pub mod automation;
mod bash_command;
pub mod board;
pub mod briefs;
pub mod case;
pub mod child;
pub mod clock;
pub mod config;
pub mod contain;
pub mod domain;
pub mod drain;
pub mod ending;
pub mod environment;
pub mod event_stream;
pub mod explain;
pub mod factory;
pub mod git;
pub mod harness;
pub mod holds;
pub mod host;
pub mod item_facts;
pub mod layer;
pub mod lifecycle;
pub mod listening;
pub mod logs;
pub mod managed;
pub mod phrase;
pub mod prompts;
pub mod pull_records;
pub mod reconcile;
mod records;
pub mod redaction;
pub mod repo_files;
pub mod results;
pub mod run;
pub mod runs;
pub mod secrets;
pub mod session_env;
pub mod spec_gap;
pub mod spec_glob;
pub mod status;
pub mod store;
pub mod tracking;
pub mod transcript;
pub mod variable_names;
pub mod web;
pub mod webhook;
pub mod work_settings;
pub mod working_pull;
