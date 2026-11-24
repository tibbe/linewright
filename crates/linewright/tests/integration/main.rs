#![allow(
    clippy::items_after_statements,
    reason = "`test_log::test` puts its setup before each test's body, so a `use` at the top of a test comes after a statement the test never wrote"
)]

mod benchmark_run_runs;
mod benchmark_runs;
mod benchmarks;
mod capture;
mod environment_shown;
mod export;
mod fake_serve;
mod harness;
mod local_api;
mod logs;
mod mcp;
mod programs;
mod runs;
mod serve_fake;
mod session_clone;
mod startup;
