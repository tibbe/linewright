//! The helpers the tests of every crate share, in one module per topic and all
//! re-exported here, so a test brings them in with
//! `use linewright_core::testing::*` or names one as `testing::X`. A shared
//! helper's name is unique across the topics.
//!
//! The helpers that the tests of one area share between crates, the ones
//! that run `lw` being in `linewright`, are in a module of the area that is
//! not re-exported: `benchmark_runs`, `captures`, `exports`, `logs`, `oauth`,
//! `runs`, `scores`, `session_clones` and `trial_runs`.
//!
//! # `world`: worlds and servers
//!
//! - `LINEWRIGHT`: the login Linewright works as.
//! - `MODEL`: the model the operator's file sets; `model`: a model by name.
//! - `COMMIT`: the commit a `World`'s Linewright says it is built from.
//! - `REQUIRED`: the label a factory requires when set to require one.
//! - `AUTO_MERGE`: auto-merge on, by the repository's merge method.
//! - `secrets_of`: the secrets of an environment holding some variables.
//! - `secrets_unnamed`: the secrets of an environment holding some variables,
//!   with no `secrets` setting.
//! - `redaction_of`: the redaction of an environment holding some variables.
//! - `World`: a factory over a local store, a scripted harness and a fake
//!   clock, and the `work_dir` its factory keeps its runs in.
//!   - `new`, `serving`, `with`, `with_repository`, `start`: start one, as
//!     configured.
//!   - `restart`: starts its factory again, reconfigured.
//!   - `poll`: one poll, with every run it started run to its end.
//!   - `issue`: an issue as the store holds it.
//!   - `implement_a_ticket`: runs the implement stage on a ticket.
//!   - `ticket_in_review`: a ticket implemented and its pull request opened,
//!     written straight into the store; `ticket_in_review_closing`, its pull
//!     request also closing another issue.
//!   - `persons_pull_closing`: a person's pull request that closes an issue.
//!   - `rework_asked`: a maintainer asks for rework and the agent does it.
//! - `Repository`: the repository settings a `World` starts with; `allowing`
//!   some merge methods.
//! - `local_store`: an empty local store of `REPO`, with a `Repository`'s
//!   settings, as a `World` starts with.
//! - `config`: the factory config a `World` starts with, its briefs over
//!   the markers.
//! - `ended_runs`: how each of a world's runs ended.
//! - `labels`: an issue's labels, sorted.
//! - `last_comment`: the body of an issue's last comment.
//! - `last_pull_comment`: the body of a pull request's last comment.
//! - `check`: a check record with its name, state and log, for the store.
//! - `implement`: scripts each implement run to add to its ticket's branch;
//!   `implement_closing`, describing its pull request as also closing another
//!   issue.
//! - `design_issues`: scripts each design to answer that the issue is designed.
//! - `with_issue_for_triage`: a world, its work dir `REPO`'s, holding an issue
//!   needing triage that each triage answers is ready for an agent.
//! - `open_ticket`: opens a ticket, ready for an agent and designed.
//! - `relabel_ready_for_agent`: a maintainer relabels a handed-back issue
//!   `ready-for-agent`.
//! - `triage_paused_after_removing_needs_triage`: an issue needing triage,
//!   its triage paused once it removes `needs-triage`, and `TRIAGE_BRIEF`,
//!   the comment it posts.
//! - `gated`: holds each session of a kind of work open until its `Gate`
//!   releases it; the `Gate` tells when one has `begun`.
//! - `gate`: a `Gate` and the `Gatepost` a session's script `wait`s at.
//! - `store_holding`: a store whose default branch holds only the files
//!   given.
//! - `HOURLY`: an automation file that runs on the hour; the fake clock starts
//!   at 12:00.
//! - `Serving`: a factory serving in the background.
//!   - `start`: starts serving.
//!   - `drain`, `stop`, `kill`: drains, stops or aborts it.
//!   - `has_ended`, `ended`, `finished`: whether it has ended, and waits for
//!     it to end of its own accord, or however long that takes.
//! - `briefs_with`: a `briefs` directory with one shipped file edited.
//!
//! # `back_door`: Creation Methods
//!
//! Each writes into the fakes the state a stage leaves, with the scripts its
//! sessions answered with.
//!
//! - `World`:
//!   - `issue_triaged_for_the_maintainer`, `issue_asking_its_reporter`: an
//!     issue triage handed to the maintainer, or asked its reporter about.
//!   - `spec_change_cut_into_tickets`: a spec change taken up, its issue cut
//!     into two tickets, as a `SpecCut`; `spec_change_ticket_in_review`, its
//!     first ticket implemented and its pull request opened;
//!     `old_spec_change_cut`: one cut under a tracking issue, as an earlier
//!     release left it.
//!   - `issue_cut_into_tickets`: an issue cut into two tickets filed under it.
//!   - `designed_on_the_issue`: an issue designed on the issue.
//!   - `run_recorded`: a triage, design or implement run ended, with one
//!     session and the input it recorded, read from the store as it is.
//!   - `ticket_handed_back_with_its_branch`, `ticket_timed_out_with_edits`,
//!     `ticket_blocked_filing_a_spec_gap`: a ticket the implement stage handed
//!     back.
//!   - `ready_for_review`, `ready_for_review_on_green_checks`,
//!     `held_for_a_code_owners_review`: a pull request marked ready once CI's
//!     grace has passed or on green checks, then, for the last, commented on
//!     by auto-merge naming the code owner's review GitHub requires.
//!   - `reworked`: a pull request the maintainer asked to rework, reworked.
//!   - `rework_timed_out_with_edits`: a ticket in review whose rework timed
//!     out.
//! - `moved_past_with`: a world whose ticket's pull request, ready for
//!   review, the default branch moved on past, in conflict with it or not.
//! - `resolve_in`: writes files in a clone and takes its rebase or merge to
//!   its end.
//! - `triage_for_the_maintainer`, `triage_asking`: script each triage to ask
//!   the maintainer, or to ask people which version.
//! - `tickets_script`: scripts each design to cut two tickets.
//! - `implement_gadgets`: scripts each implement run to commit gadgets to its
//!   ticket's file.
//! - `implement_giving_up_with_earlier_work`, `implement_timing_out_with_edits`,
//!   `implement_blocked_filing_a_spec_gap`, `rework_timing_out_with_edits`:
//!   script each session of a stage to end that way.
//! - `start_and_leave_edits`, `rename_and_leave_edits`: commit, then leave
//!   edits uncommitted, in a clone.
//! - `merge_spec_change`, `merge_spec_and_code_change`: merge a spec change
//!   of the spec, or of the spec and code.
//! - `open_undesigned_gadgets`, `open_designed_gadgets`: open an issue ready
//!   for an agent, not designed or designed.
//! - `UNFINISHED`, `TIMED_OUT`, `kept_on`: the unfinished commit's subject,
//!   and how a hand-back says the session timed out and where the work is.
//! - `timed_out`: a session's answer that it timed out.
//! - `EARLIER_WORK`, `SPEC_GAP`: the file a handed-back ticket's branch keeps,
//!   and the spec gap sessions file.
//!
//! # `harness`: the scripted agent
//!
//! - `VARIABLES`: the variables of the scripted harness's program.
//! - `Call`: what a script is called with: the session spec, session and follow-up.
//! - `ScriptedHarness`: a harness whose turns run the script set per kind of work.
//!   - `on`, `on_automation`: set the script for a kind of work or automations.
//!   - `takes`: makes the next turn take some time.
//!   - `hold`: holds the next turn open, as a `HeldTurn`.
//!   - `prompts`, `automation_prompts`: the prompts its turns were given.
//!   - `stopped`: the update each session found its clone stopped in.
//! - `Stopped`: an update stopped on conflicts, a rebase or a merge.
//! - `HeldTurn`: the agent of a turn held open, producing output when told to
//!   (`says`, `writes`) and ending its turn when dropped.
//! - `subject_file`: the file name of the subject a call works on.
//! - `usage_limit_resets_at`: when the usage limit the refusals hit resets.
//! - `refused_for_the_usage_limit`: a turn refused for the usage limit.
//! - `failed_session`: a session that ended unexpectedly, for a reason.
//! - `refused_until`: a turn refused for a usage limit that resets at a given time.
//! - `assert_in_a_process_group_of_its_own`: asserts a process group is not
//!   the tests' own.
//! - `rejected`: git's output for a push the remote rejected.
//! - `Scenario`, `Step`: what the fake `claude` does on `--version` and on a
//!   turn.
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
//! - `linewright`: the login Linewright works as, as a `Login`.
//! - `logins`: the `Login`s of some names.
//! - `sub_issue`: a sub-issue, open or closed, by an author.
//! - `ticket`: an open ticket, ready for an agent and designed.
//! - `relabeled`: an issue labeled twice, only the label events telling which last.
//! - `closed_by`: an issue an open pull request closes.
//! - `implementing`: the live run implementing an issue.
//! - `in_flight`: a run in flight, started at a given time.
//! - `limited_until`: memory of a usage limit that resets at a given time.
//! - `draining_since`: memory of a drain since a given time.
//! - `snapshot_of`: a snapshot holding some issues and nothing else.
//! - `view`: the status view of some issues and pull requests.
//! - `card`: a card with a given line, entered its column at a given time.
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
//! # `http`: requests over a connection
//!
//! - `send_over`: sends a request over a stream, with a `Host` or none.
//! - `routed`: a router's answer, in-process, to a request with a `Host` or
//!   none.
//! - `status_and_body`: an answer's status and its whole body.
//! - `UI_HOST`: the host of `public_url`.
//! - `public_url`: the web UI's public URL in tests.
//! - `ui_on`: the web UI on an address, at `public_url`, signing people in
//!   with Google.
//!
//! # `files`: git and files
//!
//! - `repository`: the root of the checkout of Linewright the tests are built
//!   from.
//! - `commit_in`: writes files in a checkout and commits them.
//! - `git_in`: runs git in a directory, failing the test if git fails, and
//!   returns what it printed.
//! - `files_named`: the files with a name under a directory.
//!
//! # `cli`: what `lw` is run on
//!
//! - `REPO`: the repository `lw` is run on.
//! - `socket_dir_in`: the directory of `serve`'s socket under a runtime
//!   directory, as `lw` finds it there.
//! - `world_in_work_dir`: a world whose work dir is `REPO`'s in a temporary
//!   directory.
//! - `json_objects`: the objects of JSON lines, as `logs --json` prints them.
//! - `link_from_path`: puts a tool on the tests' path in a directory.
//!
//! # `assert`: assertions and waiting
//!
//! - `Log`: what a thread logs, from `capture`, as `text`, `lines` by level or
//!   `usage_limit_warnings`.
//! - `eventually`: waits until a condition holds.
//! - `eventually_async`: waits until an async condition holds.
//! - `run_ids`: the run ids in `lw runs` output.
//! - `shared`: the keys two JSON objects have in common, each checked to
//!   hold the same value in both.
//!
//! # `markers`: the briefs as markers
//!
//! - `markers`: Linewright's own layer of the briefs, each file a marker
//!   naming its path and its placeholders.
//! - `marker_briefs`: the briefs with no layer over the markers.
//! - `marker`: the marker of a file filled with some values, as expected.
//! - `marker_text`: the marker layer's text of one file, for a layer of a
//!   test's own.
//! - `filled_in`: the value a marker in a text fills a placeholder with.
//! - `could_not_be_used`, `not_finished`: the follow-up a session is sent
//!   back with for a reason, or a problem with its work.
//!
//! # `same_state`: comparing stores, databases and directories
//!
//! Each asserts a stage left the same state as a back door wrote, naming
//! the first field that differs.
//!
//! - `assert_same_state`: in two stores.
//! - `assert_same_rows`: in the rows a query gives in two work directories'
//!   databases.
//! - `assert_same_input`: in the input a run recorded in two worlds.
//! - `assert_same_tree`: in two directories, their files and repositories.
//!
//! # `environments`: setup commands
//!
//! - `setup_config`: the repository's file with a setup command.
//! - `set_up_with`: ignores `node_modules` and sets a setup command.
//! - `Marks`: a directory outside every clone, where setup commands leave
//!   marks; `installing`, a setup command that notes each build, and
//!   `builds`, how many it noted.
//!
//! # `transcripts`: a recorded transcript
//!
//! - `TURN_1`, `TURN_2`: its two turns, as Claude Code streamed them.
//! - `Turn1`, `turn_1`: its first turn, in the pieces a test writes it in.
//!
//! # `trials`: tasks and the outputs of trials
//!
//! - `triaged`: what a triage answers: a bug, ready for an agent.
//! - `triage_task`: a triage run of a world, captured as a task.
//! - `task`: a run of a world, captured as a task.
//! - `triage_output`: a triage task, and the output of a trial of it.

pub mod benchmark_runs;
pub mod captures;
pub mod exports;
pub mod logs;
pub mod oauth;
pub mod runs;
pub mod scores;
pub mod session_clones;
pub mod trial_runs;

mod assert;
mod back_door;
mod cli;
mod clock;
mod environments;
mod files;
mod harness;
mod http;
mod items;
mod markers;
mod records;
mod same_state;
mod transcripts;
mod trials;
mod webhooks;
mod world;

pub use assert::*;
pub use back_door::*;
pub use cli::*;
pub use clock::*;
pub use environments::*;
pub use files::*;
pub use harness::*;
pub use http::*;
pub use items::*;
pub use markers::*;
pub use records::*;
pub use same_state::*;
pub use transcripts::*;
pub use trials::*;
pub use webhooks::*;
pub use world::*;
