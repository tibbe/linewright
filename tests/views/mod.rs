//! `serve`'s view of the repository, as the answers to the monitoring
//! commands are derived from it, for the tests of those answers.

#![allow(dead_code)]

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use linewright::domain::*;
use linewright::reconcile::{LiveRun, Memory, OpenedFor, Policy, Subject, WorkKind};
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

/// The maintainer's issue #`number`, labeled `ready-for-agent`.
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

/// `issue`, which an open pull request #`pr` by `author` closes.
pub fn closed_by(issue: Issue, pr: u64, author: &str) -> Issue {
    Issue {
        closed_by: vec![ClosingPull {
            number: Number(pr),
            author: Login::new(author),
            open: true,
        }],
        ..issue
    }
}

/// An implement run in flight on #`issue`.
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

/// What the factory remembers with the usage limit in force until
/// `resets_at`.
pub fn limited_until(resets_at: &str) -> Memory {
    Memory {
        usage_limit: Some(UsageLimit {
            resets_at: at(resets_at),
        }),
        ..Memory::default()
    }
}

/// The view of a poll at 12:00 that saw `issues` and `pulls`, with no run
/// in flight, no usage limit and no workflows.
pub fn view(issues: Vec<Issue>, pulls: Vec<Pull>) -> View {
    View {
        snapshot: Arc::new(Snapshot {
            default_branch: "main".into(),
            issues,
            pulls,
            merged: vec![],
            ticket_branches: vec![],
        }),
        opened_for: OpenedFor::default(),
        read_at: Default::default(),
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
