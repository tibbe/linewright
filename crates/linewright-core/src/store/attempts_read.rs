//! What [`Store::completed_attempts`] promises, checked on the GitHub store
//! against the workflow runs recorded in `fixtures/workflow-run-*.json` and
//! on the local store holding the same attempts.

use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::panic::Location;

use crate::domain::Timestamp;
use crate::store::Store;
use crate::workflow_run::{
    AttemptConclusion, AttemptId, CompletedAttempt, Conclusion, WorkflowRunId,
};

#[track_caller]
fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

#[track_caller]
fn id(run: u64, attempt: u32) -> AttemptId {
    AttemptId {
        run: WorkflowRunId(run),
        attempt: NonZeroU32::new(attempt).unwrap(),
    }
}

/// The repository of [`re_run_twice`].
pub const LINEWRIGHT: &str = "tibbe/linewright";

/// The repository of [`fork_run_on_main`].
pub const REGISTER: &str = "is-a-dev/register";

/// When GitHub created the run of [`re_run_twice`].
pub fn re_run_created() -> Timestamp {
    at("2026-10-29T07:18:43Z")
}

/// The attempts of tibbe/linewright's CI run 40834616225, which failed
/// twice and was re-run twice: `fixtures/workflow-run-re-run-twice.json`
/// and its `-attempt-1` and `-attempt-2`.
pub fn re_run_twice() -> Vec<CompletedAttempt> {
    [
        (1, "2026-10-29T07:22:19Z", Conclusion::Failure),
        (2, "2026-10-29T07:24:25Z", Conclusion::Failure),
        (3, "2026-10-29T07:27:13Z", Conclusion::Success),
    ]
    .into_iter()
    .map(|(attempt, completed_at, conclusion)| CompletedAttempt {
        id: id(40_834_616_225, attempt),
        workflow: "CI".into(),
        run_created_at: re_run_created(),
        completed_at: at(completed_at),
        conclusion: AttemptConclusion::Known(conclusion),
        branch: Some("22665-design".into()),
        commit: "b46bed43544054fdaaa9aea073de0eed7f3e1663".into(),
        link: format!(
            "https://github.com/tibbe/linewright/actions/runs/40834616225/attempts/{attempt}"
        ),
    })
    .collect()
}

/// is-a-dev/register's CI run 44999410949, of a pull request from the
/// branch `main` of the fork hostdomain708-sudo/register:
/// `fixtures/workflow-run-fork-main.json`.
pub fn fork_run_on_main() -> CompletedAttempt {
    CompletedAttempt {
        id: id(44_999_410_949, 1),
        workflow: "CI".into(),
        run_created_at: at("2026-11-22T07:42:02Z"),
        completed_at: at("2026-11-22T07:42:22Z"),
        conclusion: AttemptConclusion::Known(Conclusion::Failure),
        branch: None,
        commit: "0025172d638509aa6924db959818ee5c834639a0".into(),
        link: "https://github.com/is-a-dev/register/actions/runs/44999410949/attempts/1".into(),
    }
}

/// A conclusion none of GitHub's nine, as GitHub might add one.
pub const UNKNOWN_CONCLUSION: &str = "deferred";

/// [`fork_run_on_main`], concluded [`UNKNOWN_CONCLUSION`].
pub fn concluded_outside_the_nine() -> CompletedAttempt {
    CompletedAttempt {
        conclusion: AttemptConclusion::Other(UNKNOWN_CONCLUSION.into()),
        ..fork_run_on_main()
    }
}

/// Those of `attempts` not in `known`, in order.
fn unknown(attempts: Vec<CompletedAttempt>, known: &BTreeSet<AttemptId>) -> Vec<CompletedAttempt> {
    let mut attempts: Vec<_> = attempts
        .into_iter()
        .filter(|attempt| !known.contains(&attempt.id))
        .collect();
    attempts.sort_by_key(|attempt| attempt.id);
    attempts
}

/// `store` holds [`re_run_twice`]'s run, and perhaps others in progress:
/// every attempt of it is read, and those known need not be.
#[track_caller]
pub fn every_attempt_of_a_run_re_run_twice_is_read(store: &dyn Store) -> impl Future<Output = ()> {
    let caller = Location::caller();
    async move {
        let none = BTreeSet::new();
        let all = read(store, re_run_created(), &none, caller).await;
        assert_eq!(unknown(all, &none), re_run_twice(), "at {caller}");

        let known = BTreeSet::from([id(40_834_616_225, 1), id(40_834_616_225, 2)]);
        let latest = read(store, re_run_created(), &known, caller).await;
        assert_eq!(unknown(latest, &known), re_run_twice()[2..], "at {caller}");
    }
}

/// `store` holds `attempt` alone: it is read as it is held.
#[track_caller]
pub fn the_attempt_is_read(
    store: &dyn Store,
    attempt: CompletedAttempt,
) -> impl Future<Output = ()> {
    let caller = Location::caller();
    async move {
        let read = read(store, attempt.run_created_at, &BTreeSet::new(), caller).await;
        assert_eq!(read, [attempt], "at {caller}");
    }
}

/// `store` holds [`re_run_twice`]'s run: a poll after one that read it,
/// with nothing new, reads the same.
#[track_caller]
pub fn a_poll_with_nothing_new_reads_the_same(store: &dyn Store) -> impl Future<Output = ()> {
    let caller = Location::caller();
    async move {
        let none = BTreeSet::new();
        for _ in 0..2 {
            let read = read(store, re_run_created(), &none, caller).await;
            assert_eq!(unknown(read, &none), re_run_twice(), "at {caller}");
        }
    }
}

/// What `store` reads, failing the test called at `caller` when it fails.
async fn read(
    store: &dyn Store,
    created_since: Timestamp,
    known: &BTreeSet<AttemptId>,
    caller: &Location<'_>,
) -> Vec<CompletedAttempt> {
    store
        .completed_attempts(created_since, known)
        .await
        .unwrap_or_else(|e| panic!("reading the completed attempts at {caller}: {e:#}"))
}
