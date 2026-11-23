pub const COMMAND: &str = "lw";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The version of Linewright and `commit`, the commit it is built from, as
/// `--version` prints them.
pub fn version_and_commit(commit: &str) -> String {
    format!("{VERSION} ({commit})")
}

pub mod access;
pub mod actions;
pub mod api;
pub mod apply;
pub mod attachments;
pub mod automation;
mod bash_command;
pub mod benchmark_runs;
pub mod benchmarks;
pub mod board;
pub mod briefs;
mod cadence;
pub mod child;
pub mod cli;
pub mod clock;
mod clones;
pub mod config;
pub mod contain;
pub mod cost;
pub mod database;
pub mod domain;
pub mod drain;
pub mod ending;
pub mod environment;
pub mod event_stream;
pub mod explain;
pub mod export;
pub mod factory;
#[cfg(feature = "test-utils")]
pub mod fake_claude;
pub mod git;
pub mod harness;
pub mod holds;
pub mod host;
pub mod html;
pub mod http;
pub mod input;
pub mod item_facts;
pub mod layer;
pub mod lifecycle;
pub mod listening;
pub mod logs;
pub mod managed;
pub mod name;
pub mod party;
pub mod phrase;
pub mod prompts;
pub mod pull_records;
pub mod pushed_branches;
pub mod reconcile;
mod records;
pub mod redaction;
pub mod repo_files;
pub mod results;
pub mod run;
pub mod runs;
pub mod secrets;
pub mod serving;
pub mod session;
pub mod session_env;
pub mod spec_gap;
pub mod spec_glob;
pub mod status;
pub mod store;
pub mod task;
#[cfg(feature = "test-utils")]
pub mod testing;
pub mod tracking;
pub mod transcript;
pub mod tree;
pub mod trial;
pub mod variable_names;
pub mod webhook;
pub mod winding_up;
pub mod work_settings;
pub mod workflow_run;
pub mod working_pull;
