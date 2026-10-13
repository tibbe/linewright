//! `linewright status`: the work in flight and the workflows, derived from a
//! snapshot, the workflow files and their last runs the way the reconcile
//! step derives them.

use crate::domain::{CheckState, Filed, PullState, Snapshot, Timestamp, label};
use crate::reconcile::{NextRun, Policy, next_run, pending_spec_changes};
use crate::workflow::{LastRuns, WorkflowFiles};

/// The report's lines: the work in flight, then each workflow by name with
/// its last and next run, then each workflow file that does not parse.
pub fn report(
    snapshot: &Snapshot,
    workflows: &WorkflowFiles,
    last_runs: &LastRuns,
    now: Timestamp,
    policy: &Policy,
) -> Vec<String> {
    let mut lines = work_in_flight(snapshot, now, policy);
    let mut parsed: Vec<_> = workflows.parsed.iter().collect();
    parsed.sort_by(|a, b| a.name.cmp(&b.name));
    for workflow in parsed {
        let runs = match last_runs.get(&workflow.name) {
            None => "not run yet, its schedule starts when a poll first sees it".into(),
            Some(last) => {
                let next = match next_run(workflow, last, now) {
                    NextRun::Due => "next run due now".into(),
                    NextRun::At(tick) => format!("next run {}", utc(tick)),
                    NextRun::Never => "no next run, its schedule ticks no more".into(),
                };
                format!("last run {}, {next}", utc(last))
            }
        };
        lines.push(format!("workflow {}: {runs}", workflow.name));
    }
    let mut unparsed: Vec<_> = workflows.unparsed.iter().collect();
    unparsed.sort_by(|a, b| a.path.cmp(&b.path));
    for file in unparsed {
        lines.push(format!("{}: does not parse: {}", file.path, file.reason));
    }
    if lines.is_empty() {
        lines.push("Nothing in flight.".into());
    }
    lines
}

fn utc(time: Timestamp) -> String {
    time.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn work_in_flight(snapshot: &Snapshot, now: Timestamp, policy: &Policy) -> Vec<String> {
    let linewright = &policy.linewright;
    let mut lines = vec![];
    for pr in pending_spec_changes(snapshot, now, policy) {
        lines.push(format!(
            "{} {}: spec change waiting for tickets",
            pr.number, pr.title
        ));
    }
    for issue in snapshot.issues.iter().filter(|i| i.open) {
        let state = if matches!(issue.filed, Some(Filed::Tracking { .. })) {
            let closed = issue.sub_issues.iter().filter(|t| !t.open).count();
            format!(
                "tracking issue, {closed}/{} tickets closed",
                issue.sub_issues.len()
            )
        } else if issue.has_label(label::NEEDS_TRIAGE) {
            "waiting for triage".into()
        } else if issue.has_label(label::NEEDS_INFO) {
            "waiting on the reporter".into()
        } else if issue.has_label(label::READY_FOR_HUMAN) {
            "with the maintainer".into()
        } else if issue.has_label(label::READY_FOR_AGENT) {
            let pr = snapshot.pulls.iter().find(|p| {
                p.is_open() && &p.author == linewright && p.closes.contains(&issue.number)
            });
            let blockers: Vec<String> = issue
                .blocked_by
                .iter()
                .filter(|b| b.open)
                .map(|b| b.number.to_string())
                .collect();
            match pr {
                Some(pr) => {
                    let ci = if pr.checks.is_empty() {
                        "no checks"
                    } else if pr.checks.iter().any(|c| c.state == CheckState::Failed) {
                        "CI failing"
                    } else if pr.checks.iter().any(|c| c.state == CheckState::Pending) {
                        "CI running"
                    } else {
                        "CI green"
                    };
                    let stage = match pr.state {
                        PullState::Open { draft: true } => "draft",
                        _ => "ready for review",
                    };
                    format!("in review: {} ({stage}, {ci})", pr.number)
                }
                None if !blockers.is_empty() => format!("blocked by {}", blockers.join(", ")),
                None if issue.is_assigned(linewright) => "claimed: being implemented".into(),
                None => "ready to claim".into(),
            }
        } else {
            continue;
        };
        lines.push(format!("{} {}: {state}", issue.number, issue.title));
    }
    lines
}
