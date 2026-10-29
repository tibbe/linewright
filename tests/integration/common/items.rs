use std::collections::HashMap;
use std::sync::Arc;

use chrono::TimeDelta;
use linewright::automation::{AutomationFiles, LastScheduledRuns};
use linewright::domain::*;
use linewright::harness::UsageLimit;
use linewright::reconcile::{Drain, LiveRun, Memory, Policy, Subject, WorkKind};
use linewright::runs::InFlight;
use linewright::spec_glob::SpecGlob;
use linewright::status::{Polls, View};

use super::clock::at;

pub fn policy() -> Policy {
    Policy {
        linewright: Login::new("factory"),
        spec_glob: SpecGlob::default(),
        spec_lookback: TimeDelta::days(7),
        done_window: TimeDelta::days(7),
        max_runs: 4,
        ci_grace: TimeDelta::minutes(5),
        ci_timeout: TimeDelta::hours(6),
        ci_attempts: 2,
        auto_merge: None,
        update_rule: UpdateRule::RebaseOnConflict,
        allowed_merge_methods: MergeMethods::all(),
        require_label: None,
    }
}

pub fn sub_issue(number: u64, open: bool, author: &str) -> SubIssue {
    SubIssue {
        number: Number(number),
        author: Login::new(author),
        state: SubIssueState::open(open),
    }
}

pub fn ticket(number: u64, title: &str) -> Issue {
    Issue {
        number: Number(number),
        title: title.into(),
        body: String::new(),
        author: Login::new("maintainer"),
        open: true,
        created_at: at("2026-09-30T09:00:00Z"),
        updated_at: at("2026-09-30T09:00:00Z"),
        labels: vec!["ready-for-agent".into(), label::DESIGNED.into()],
        assignees: vec![],
        comments: vec![],
        blocked_by: vec![],
        sub_issues: vec![],
        parent: None,
        events: vec![],
        filed: None,
        closed_by: vec![],
    }
}

/// `issue`, with each of its labels applied by Linewright when it was
/// opened, as Linewright labels an issue it triages or hands back: it has
/// worked it.
pub fn labeled_by_linewright(issue: Issue) -> Issue {
    let labelings = issue.labels.iter().map(|label| IssueEvent::Labeled {
        label: label.clone(),
        by: Some(Login::new("factory")),
        at: issue.created_at,
    });
    Issue {
        events: labelings.chain(issue.events.iter().cloned()).collect(),
        ..issue
    }
}

/// `issue` labeled `older`, then `newer`, by Linewright, listed newest first so that only
/// the order they were applied in tells them apart.
pub fn relabeled(issue: Issue, older: &str, newer: &str) -> Issue {
    Issue {
        labels: vec![newer.into(), older.into()],
        events: [
            (older, "2026-10-01T09:00:00Z"),
            (newer, "2026-10-01T10:00:00Z"),
        ]
        .map(|(label, time)| IssueEvent::Labeled {
            label: label.into(),
            by: Some(Login::new("factory")),
            at: at(time),
        })
        .to_vec(),
        ..issue
    }
}

pub fn closed_by(issue: Issue, pr: u64, author: &str) -> Issue {
    Issue {
        closed_by: vec![ClosingPull {
            number: Number(pr),
            author: Login::new(author),
            state: PullState::Open { draft: false },
            updated_at: at("2026-09-30T09:00:00Z"),
        }],
        ..issue
    }
}

/// Linewright assigned to an item by itself at `when`, as a claim does.
pub fn assigning_linewright(when: &str) -> IssueEvent {
    IssueEvent::Assignment(Assignment::Assigned {
        assignee: Login::new("factory"),
        actor: Login::new("factory"),
        at: at(when),
    })
}

/// Linewright unassigning itself from an item at `when`, as a hand-back
/// does.
pub fn linewright_letting_go(when: &str) -> IssueEvent {
    IssueEvent::Assignment(Assignment::Unassigned {
        assignee: Login::new("factory"),
        actor: Login::new("factory"),
        at: at(when),
    })
}

/// Linewright unassigned from an item by the maintainer at `when`.
pub fn unassigning_linewright(when: &str) -> IssueEvent {
    IssueEvent::Assignment(Assignment::Unassigned {
        assignee: Login::new("factory"),
        actor: Login::new("maintainer"),
        at: at(when),
    })
}

pub fn implementing(issue: u64) -> LiveRun {
    LiveRun {
        subject: Subject::Issue(Number(issue)),
        kind: WorkKind::Implement,
    }
}

/// The run `id`, started at `started`.
pub fn in_flight(id: &str, started: &str) -> InFlight {
    InFlight {
        id: id.parse().unwrap(),
        started_at: at(started),
    }
}

pub fn limited_until(resets_at: &str) -> Memory {
    Memory {
        usage_limit: Some(UsageLimit {
            resets_at: at(resets_at),
        }),
        ..Memory::default()
    }
}

pub fn draining_since(since: &str) -> Memory {
    Memory {
        drain: Some(Drain { since: at(since) }),
        ..Memory::default()
    }
}

pub fn view(issues: Vec<Issue>, pulls: Vec<Pull>) -> View {
    View {
        snapshot: Arc::new(Snapshot {
            default_branch: "main".into(),
            issues,
            pulls,
            merged: vec![],
            ticket_branches: vec![],
            closed: vec![],
        }),
        read_at: HashMap::default(),
        policy: policy(),
        polls: Polls {
            last: at("2026-10-01T12:00:00Z"),
            next: at("2026-10-01T12:01:00Z"),
        },
        memory: Memory::default(),
        live: vec![],
        in_flight: vec![],
        automations: AutomationFiles::default(),
        last_scheduled_runs: LastScheduledRuns::default(),
        unusable: vec![],
    }
}
