//! The helpers the integration tests share, in one module per topic and all
//! re-exported here, so a test brings them in with `use common::*` or names
//! one as `common::X`. A shared helper's name is unique across the topics.
//!
//! # `world`: worlds and servers
//!
//! - `LINEWRIGHT`: the login Linewright works as.
//! - `REQUIRED`: the label a factory requires when set to require one.
//! - `credentials_of`: the credentials of an environment holding some variables.
//! - `redaction_of`: the redaction of an environment holding some variables.
//! - `World`: a factory over a local store, a scripted harness and a fake clock.
//!   - `new`, `serving`, `with`, `with_repository`, `start`: start one, as
//!     configured.
//!   - `restart`: starts its factory again, reconfigured.
//!   - `poll`: one poll, with every run it started run to its end.
//!   - `issue`: an issue as the store holds it.
//!   - `ticket_in_review`: a ticket implemented and its pull request opened.
//!   - `persons_pull_closing`: a person's pull request that closes an issue.
//!   - `rework_asked`: a maintainer asks for rework and the agent does it.
//! - `Repository`: the repository settings a `World` starts with; `allowing`
//!   some merge methods.
//! - `config`: the factory config a `World` starts with.
//! - `labels`: an issue's labels, sorted.
//! - `last_comment`: the body of an issue's last comment.
//! - `last_pull_comment`: the body of a pull request's last comment.
//! - `check`: a check record with its name, state and log, for the store.
//! - `implement`: scripts each implement run to add to its ticket's branch.
//! - `design_issues`: scripts each design to answer that the issue is designed.
//! - `with_issue_for_triage`: a world, its work dir `REPO`'s, holding an issue
//!   needing triage that each triage answers is ready for an agent.
//! - `open_ticket`: opens a ticket, ready for an agent and designed.
//! - `HOURLY`: an automation file that runs on the hour; the fake clock starts
//!   at 12:00.
//! - `Serving`: a factory serving in the background.
//!   - `start`: starts serving.
//!   - `drain`, `stop`, `kill`: drains, stops or aborts it.
//!   - `has_ended`, `ended`, `finished`: whether it has ended, and waits for
//!     it to end of its own accord, or however long that takes.
//! - `briefs_with`: a copy of the shipped briefs with one prompt edited.
//!
//! # `harness`: the scripted agent
//!
//! - `Call`: what a script is called with: the session spec, session and follow-up.
//! - `ScriptedHarness`: a harness whose turns run the script set per kind of work.
//!   - `on`, `on_automation`: set the script for a kind of work or automations.
//!   - `takes`: makes the next turn take some time.
//!   - `hold`: holds the next turn open, as a `HeldTurn`.
//!   - `prompts`, `automation_prompts`: the prompts its turns were given.
//! - `HeldTurn`: the agent of a turn held open, producing output when told to
//!   (`says`, `writes`) and ending its turn when dropped.
//! - `subject_file`: the file name of the subject a call works on.
//! - `usage_limit_resets_at`: when the usage limit the refusals hit resets.
//! - `refused_for_the_usage_limit`: a turn refused for the usage limit before it began.
//! - `refused_part_way`: a turn refused for the usage limit part way through.
//! - `refused_until`: a turn refused for a usage limit that resets at a given time.
//! - `rejected`: git's output for a push the remote rejected.
//! - `fake_claude`: a stand-in `claude` program whose turns answer a message.
//! - `fake_claude_streaming`: a stand-in `claude` program that streams events.
//! - `fake_claude_saying`: a stand-in `claude` that streams, keeps and writes errors.
//! - `write_program`: writes an executable script safely while other tests run.
//! - `nothing_needs_doing`: an automation's answer that it found nothing to do.
//!
//! # `clock`: clocks and times
//!
//! - `NOW`: the time the functional core is asked at.
//! - `at`: the `Timestamp` an RFC 3339 string names.
//! - `FakeClock`: a clock that moves only when advanced, recording its sleeps;
//!   `new`, `advance`, `next_sleep`.
//!
//! # `items`: item and view builders for the functional core
//!
//! - `policy`: the policy the functional core decides with.
//! - `sub_issue`: a sub-issue, open or closed, by an author.
//! - `ticket`: an open ticket, ready for an agent and designed.
//! - `relabeled`: an issue labeled twice, only the label events telling which last.
//! - `closed_by`: an issue an open pull request closes.
//! - `implementing`: the live run implementing an issue.
//! - `in_flight`: a run in flight, started at a given time and last heard from
//!   at one, if ever.
//! - `limited_until`: memory of a usage limit that resets at a given time.
//! - `draining_since`: memory of a drain since a given time.
//! - `view`: the status view of some issues and pull requests.
//!
//! # `webhooks`: deliveries
//!
//! - `SECRET`: the webhook secret.
//! - `webhooks`: the webhooks config listening at an address.
//! - `signature`: the `X-Hub-Signature-256` of a body.
//! - `Request`: a delivery as it reaches the listener: `signed_json` or
//!   `signed_form`, and `send` to a listener.
//! - `Response`: the listener's answer to a delivery.
//! - `signed`: sends a signed JSON delivery of an event.
//! - `signed_form`: sends a signed form-encoded delivery of an event.
//!
//! # `files`: git and files
//!
//! - `commit_in`: writes files in a checkout and commits them.
//! - `git_in`: runs git in a directory, failing the test if git fails.
//! - `files_named`: the files with a name under a directory.
//!
//! # `cli`: running `lw`
//!
//! - `REPO`: the repository `lw` is run on.
//! - `world_in_work_dir`: a world whose work dir is `REPO`'s in a temporary
//!   directory.
//! - `Output`: the exit code, stdout and stderr of an `lw`.
//! - `lw_in`: runs `lw` on `REPO` with a work dir.
//!
//! # `assert`: assertions and waiting
//!
//! - `Log`: what a thread logs, from `capture`, as `text`, `lines` by level or
//!   `usage_limit_warnings`.
//! - `eventually`: waits until a condition holds.
//! - `eventually_async`: waits until an async condition holds.
//! - `run_ids`: the run ids in `lw runs` output.

mod assert;
mod cli;
mod clock;
mod files;
mod harness;
mod items;
mod webhooks;
mod world;

pub use assert::*;
pub use cli::*;
pub use clock::*;
pub use files::*;
pub use harness::*;
pub use items::*;
pub use webhooks::*;
pub use world::*;
