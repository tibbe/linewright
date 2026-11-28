//! What [`Store::completed_attempts`] promises, checked on the GitHub store
//! against the workflow runs recorded in `fixtures/workflow-run-*.json` and
//! on the local store holding the same attempts.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use crate::domain::Timestamp;
use crate::store::Store;
use crate::workflow_run::{
    AttemptConclusion, AttemptId, CompletedAttempt, Conclusion, WorkflowRunId,
};

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

fn id(run: u64, attempt: u32) -> AttemptId {
    AttemptId {
        run: WorkflowRunId(run),
        attempt: NonZeroU32::new(attempt).unwrap(),
    }
}

/// The repository of [`re_run_three_times`].
pub const LINEWRIGHT: &str = "tibbe/linewright";

/// The repository of [`fork_run_on_main`].
pub const REGISTER: &str = "is-a-dev/register";

/// When GitHub created the run of [`re_run_three_times`].
pub fn re_run_created() -> Timestamp {
    at("2026-11-13T16:30:39Z")
}

/// The attempts of tibbe/linewright's CI run 43463419555, which failed
/// three times and was re-run three times:
/// `fixtures/workflow-run-re-run-three-times.json` and its `-attempt-1`,
/// `-attempt-2` and `-attempt-3`.
pub fn re_run_three_times() -> Vec<CompletedAttempt> {
    [
        (1, "2026-11-13T16:45:52Z", Conclusion::Failure),
        (2, "2026-11-13T17:09:39Z", Conclusion::Failure),
        (3, "2026-11-13T17:25:43Z", Conclusion::Failure),
        (4, "2026-11-13T17:36:21Z", Conclusion::Success),
    ]
    .into_iter()
    .map(|(attempt, completed_at, conclusion)| CompletedAttempt {
        id: id(43_463_419_555, attempt),
        workflow: "CI".into(),
        run_created_at: re_run_created(),
        completed_at: at(completed_at),
        conclusion: AttemptConclusion::Known(conclusion),
        branch: Some("1177-spec-change-redact-what-linewright-produ".into()),
        commit: "5ff00348cb5a392276a772be09ca518186fc4e3e".into(),
        link: format!(
            "https://github.com/tibbe/linewright/actions/runs/43463419555/attempts/{attempt}"
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

/// `store` holds [`re_run_three_times`]'s run, and perhaps others in progress:
/// every attempt of it is read, and those known need not be.
pub async fn every_attempt_of_a_run_re_run_three_times_is_read(store: &dyn Store) {
    let none = BTreeSet::new();
    let all = read(store, re_run_created(), &none).await;
    assert_eq!(unknown(all, &none), re_run_three_times());

    let known = BTreeSet::from([
        id(43_463_419_555, 1),
        id(43_463_419_555, 2),
        id(43_463_419_555, 3),
    ]);
    let latest = read(store, re_run_created(), &known).await;
    assert_eq!(unknown(latest, &known), re_run_three_times()[3..]);
}

/// `store` holds `attempt` alone: it is read as it is held.
pub async fn the_attempt_is_read(store: &dyn Store, attempt: CompletedAttempt) {
    let read = read(store, attempt.run_created_at, &BTreeSet::new()).await;
    assert_eq!(read, [attempt]);
}

/// `store` holds [`re_run_three_times`]'s run: a poll after one that read it,
/// with nothing new, reads the same.
pub async fn a_poll_with_nothing_new_reads_the_same(store: &dyn Store) {
    let none = BTreeSet::new();
    for _ in 0..2 {
        let read = read(store, re_run_created(), &none).await;
        assert_eq!(unknown(read, &none), re_run_three_times());
    }
}

/// What `store` reads, failing the test when it fails.
async fn read(
    store: &dyn Store,
    created_since: Timestamp,
    known: &BTreeSet<AttemptId>,
) -> Vec<CompletedAttempt> {
    store
        .completed_attempts(created_since, known)
        .await
        .unwrap_or_else(|e| panic!("reading the completed attempts: {e:#}"))
}
