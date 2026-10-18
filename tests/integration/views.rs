use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use linewright::domain::*;
use linewright::reconcile::{LiveRun, Memory, Policy, Subject, WorkKind};
use linewright::runs::InFlight;
use linewright::runtime::UsageLimit;
use linewright::spec_glob::SpecGlob;
use linewright::status::{Polls, View};
use linewright::workflow::{LastRuns, WorkflowFiles};

/// 2026-10-01 is a Thursday.
pub fn at(time: &str) -> Timestamp {
    DateTime::parse_from_rfc3339(time)
        .unwrap()
        .with_timezone(&Utc)
}

pub fn policy() -> Policy {
    Policy {
        linewright: Login::new("factory"),
        spec_glob: SpecGlob::default(),
        spec_lookback: TimeDelta::days(7),
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

pub fn ticket(number: u64, title: &str) -> Issue {
    Issue {
        number: Number(number),
        title: title.into(),
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
        closed_by: vec![],
    }
}

/// `issue` labeled `older`, then `newer`, listed newest first so that only
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
        }],
        ..issue
    }
}

pub fn implementing(issue: u64) -> LiveRun {
    LiveRun {
        subject: Subject::Issue(Number(issue)),
        kind: WorkKind::Implement,
    }
}

/// The run `id`, started at `started`, whose agent last produced output at
/// `heard`, or that has started no session.
pub fn in_flight(id: &str, started: &str, heard: Option<&str>) -> InFlight {
    InFlight {
        id: id.parse().unwrap(),
        started_at: at(started),
        last_output: heard.map(at),
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

pub fn view(issues: Vec<Issue>, pulls: Vec<Pull>) -> View {
    View {
        snapshot: Arc::new(Snapshot {
            default_branch: "main".into(),
            issues,
            pulls,
            merged: vec![],
            ticket_branches: vec![],
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
        workflows: WorkflowFiles::default(),
        last_runs: LastRuns::default(),
    }
}
