//! `linewright status`, end to end on its data model: a snapshot, the
//! workflow files, their last runs and the clock in, the report out
//! (`CODING_STANDARDS.md`, "Where tests go").

use chrono::{DateTime, TimeDelta, Utc};
use linewright::domain::*;
use linewright::reconcile::Policy;
use linewright::spec_glob::SpecGlob;
use linewright::status::report;
use linewright::workflow::{LastRuns, Unparsed, WorkflowFiles};

/// 2026-10-01 is a Thursday.
fn at(time: &str) -> Timestamp {
    DateTime::parse_from_rfc3339(time)
        .unwrap()
        .with_timezone(&Utc)
}

fn policy() -> Policy {
    Policy {
        machine_user: Login::new("factory"),
        spec_glob: SpecGlob::default(),
        spec_lookback: TimeDelta::days(7),
        max_workers: 4,
        ci_grace: TimeDelta::minutes(5),
        ci_timeout: TimeDelta::hours(6),
        ci_attempts: 2,
        auto_merge: None,
        update_rule: UpdateRule::RebaseOnConflict,
        allowed_merge_methods: MergeMethods::all(),
        require_label: None,
    }
}

fn snapshot(issues: Vec<Issue>) -> Snapshot {
    Snapshot {
        default_branch: "main".into(),
        issues,
        pulls: vec![],
        merged: vec![],
    }
}

fn ready_issue() -> Issue {
    Issue {
        number: Number(7),
        title: "Do the thing".into(),
        body: String::new(),
        author: Login::new("maintainer"),
        open: true,
        created_at: at("2026-09-30T09:00:00Z"),
        labels: vec!["ready-for-agent".into()],
        assignees: vec![],
        comments: vec![],
        blocked_by: vec![],
        sub_issues: vec![],
        parent: None,
        events: vec![],
        filed: None,
    }
}

/// The workflows, each a name and a cron expression, and the files that do
/// not parse.
fn files(workflows: &[(&str, &str)], unparsed: &[(&str, &str)]) -> WorkflowFiles {
    WorkflowFiles {
        parsed: workflows
            .iter()
            .map(|&(name, cron)| {
                linewright::workflow::parse(
                    &name.parse().unwrap(),
                    &format!("---\non:\n  schedule: \"{cron}\"\n---\nLook.\n"),
                )
                .unwrap()
            })
            .collect(),
        unparsed: unparsed
            .iter()
            .map(|&(path, reason)| Unparsed {
                path: path.into(),
                reason: reason.into(),
            })
            .collect(),
    }
}

fn last_runs(runs: &[(&str, &str)]) -> LastRuns {
    runs.iter()
        .map(|&(name, time)| (name.parse().unwrap(), at(time)))
        .collect()
}

#[test]
fn a_workflow_is_listed_with_its_last_run_and_its_next_tick_in_utc() {
    let workflows = files(&[("weekly", "0 6 * * 1")], &[]);
    let runs = last_runs(&[("weekly", "2026-09-28T06:00:02Z")]);

    assert_eq!(
        report(
            &snapshot(vec![]),
            &workflows,
            &runs,
            at("2026-10-01T12:00:00Z"),
            &policy()
        ),
        vec!["workflow weekly: last run 2026-09-28 06:00 UTC, next run 2026-10-05 06:00 UTC"]
    );
}

#[test]
fn a_workflow_whose_schedule_ticked_since_its_last_run_is_due_now() {
    let workflows = files(&[("hourly", "0 * * * *")], &[]);
    let runs = last_runs(&[("hourly", "2026-10-01T11:30:00Z")]);

    assert_eq!(
        report(
            &snapshot(vec![]),
            &workflows,
            &runs,
            at("2026-10-01T12:10:00Z"),
            &policy()
        ),
        vec!["workflow hourly: last run 2026-10-01 11:30 UTC, next run due now"]
    );
}

#[test]
fn a_workflow_whose_schedule_never_ticks_again_has_no_next_run() {
    let workflows = files(&[("leap", "0 0 30 2 *")], &[]);
    let runs = last_runs(&[("leap", "2026-09-28T06:00:00Z")]);

    assert_eq!(
        report(
            &snapshot(vec![]),
            &workflows,
            &runs,
            at("2026-10-01T12:00:00Z"),
            &policy()
        ),
        vec![
            "workflow leap: last run 2026-09-28 06:00 UTC, no next run, its schedule ticks no more"
        ]
    );
}

#[test]
fn a_workflow_with_no_recorded_run_is_shown_as_not_run_yet() {
    let workflows = files(&[("hourly", "0 * * * *")], &[]);

    assert_eq!(
        report(
            &snapshot(vec![]),
            &workflows,
            &LastRuns::default(),
            at("2026-10-01T12:10:00Z"),
            &policy()
        ),
        vec!["workflow hourly: not run yet, its schedule starts when a poll first sees it"]
    );
}

#[test]
fn a_workflow_file_that_does_not_parse_is_listed_with_the_reason() {
    let workflows = files(
        &[],
        &[(
            ".linewright/workflows/broken.md",
            "`on.schedule` is \"every hour\", not a cron expression",
        )],
    );

    assert_eq!(
        report(
            &snapshot(vec![]),
            &workflows,
            &LastRuns::default(),
            at("2026-10-01T12:10:00Z"),
            &policy()
        ),
        vec![
            ".linewright/workflows/broken.md: does not parse: `on.schedule` is \"every hour\", not a cron expression"
        ]
    );
}

#[test]
fn the_work_in_flight_comes_first_then_the_workflows_by_name_then_the_files_that_do_not_parse() {
    let workflows = files(
        &[("weekly", "0 6 * * 1"), ("hourly", "0 * * * *")],
        &[(".linewright/workflows/broken.md", "no front matter")],
    );
    let runs = last_runs(&[
        ("weekly", "2026-09-28T06:00:00Z"),
        ("hourly", "2026-10-01T12:00:00Z"),
    ]);

    assert_eq!(
        report(
            &snapshot(vec![ready_issue()]),
            &workflows,
            &runs,
            at("2026-10-01T12:10:00Z"),
            &policy()
        ),
        vec![
            "#7 Do the thing: ready to claim",
            "workflow hourly: last run 2026-10-01 12:00 UTC, next run 2026-10-01 13:00 UTC",
            "workflow weekly: last run 2026-09-28 06:00 UTC, next run 2026-10-05 06:00 UTC",
            ".linewright/workflows/broken.md: does not parse: no front matter",
        ]
    );
}

#[test]
fn nothing_in_flight_shows_when_there_is_neither_work_nor_a_workflow() {
    assert_eq!(
        report(
            &snapshot(vec![]),
            &files(&[], &[]),
            &LastRuns::default(),
            at("2026-10-01T12:10:00Z"),
            &policy()
        ),
        vec!["Nothing in flight."]
    );
}
