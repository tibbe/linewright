//! The facts Linewright decides on: a snapshot of the store (see
//! `docs/specs/architecture.md`, "The store is an interface"). Every machine state
//! the spec names (claimed, in review) is derived from these facts, never
//! stored.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Timestamp = DateTime<Utc>;

/// An issue or pull request number. GitHub numbers both from one sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Number(pub u64);

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// A GitHub account name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Login(pub String);

impl Login {
    pub fn new(login: impl Into<String>) -> Self {
        Login(login.into())
    }

    /// GitHub Apps act as `<app>[bot]`.
    pub fn is_bot(&self) -> bool {
        self.0.ends_with("[bot]")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A person other than the machine user: someone to ask for a review.
    pub fn is_person_besides(&self, machine_user: &Login) -> bool {
        self != machine_user && !self.is_bot()
    }
}

impl fmt::Display for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The labels of `docs/specs/product.md`, "Repository".
pub mod label {
    pub const NEEDS_TRIAGE: &str = "needs-triage";
    pub const NEEDS_INFO: &str = "needs-info";
    pub const READY_FOR_AGENT: &str = "ready-for-agent";
    pub const READY_FOR_HUMAN: &str = "ready-for-human";
    pub const BUG: &str = "bug";
    pub const ENHANCEMENT: &str = "enhancement";
    pub const SPEC_PROCESSED: &str = "linewright:spec-processed";

    /// The labels that say where an issue is in its lifecycle. An issue
    /// handed from one state to another loses all the others.
    pub const STATES: [&str; 4] = [NEEDS_TRIAGE, NEEDS_INFO, READY_FOR_AGENT, READY_FOR_HUMAN];

    /// Every label Linewright creates at startup, with its description.
    pub const ALL: [(&str, &str); 7] = [
        (NEEDS_TRIAGE, "New, not yet assessed"),
        (NEEDS_INFO, "Waiting on the reporter"),
        (READY_FOR_AGENT, "Actionable: an agent may claim it"),
        (READY_FOR_HUMAN, "The maintainer must act or decide"),
        (BUG, "Something is broken"),
        (ENHANCEMENT, "New feature or improvement"),
        (
            SPEC_PROCESSED,
            "This merged PR's spec changes have been ticketed",
        ),
    ];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub author: Login,
    pub body: String,
    pub created_at: Timestamp,
}

/// Another issue an issue points at (a blocker or a sub-issue), with whether
/// it is still open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Linked {
    pub number: Number,
    pub open: bool,
}

/// The issue timeline events the reconcile step reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum IssueEvent {
    Labeled {
        label: String,
        at: Timestamp,
    },
    Unassigned {
        assignee: Login,
        actor: Login,
        at: Timestamp,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub number: Number,
    pub title: String,
    pub body: String,
    pub author: Login,
    pub open: bool,
    pub created_at: Timestamp,
    pub labels: Vec<String>,
    pub assignees: Vec<Login>,
    pub comments: Vec<Comment>,
    pub blocked_by: Vec<Linked>,
    pub sub_issues: Vec<Linked>,
    pub events: Vec<IssueEvent>,
    /// What the issue is for, if Linewright filed it.
    #[serde(default)]
    pub filed: Option<Filed>,
}

/// What an issue Linewright filed is for. A store reads it from the issue's
/// title and body once, when it builds the snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Filed {
    /// The tracking issue of a spec change.
    Tracking { spec_change: Number },
    /// A spec gap found while working on a ticket, which did not block it.
    SpecGap { ticket: Number },
}

impl Filed {
    pub fn read(author: &Login, title: &str, body: &str, machine_user: &Login) -> Option<Filed> {
        if author != machine_user {
            return None;
        }
        if let Some(spec_change) = crate::tracking::spec_change_of(title, body) {
            return Some(Filed::Tracking { spec_change });
        }
        crate::spec_gap::ticket_of(body).map(|ticket| Filed::SpecGap { ticket })
    }
}

impl Issue {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    pub fn is_assigned(&self, login: &Login) -> bool {
        self.assignees.contains(login)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum PullState {
    Open { draft: bool },
    Merged { at: Timestamp },
    Closed { at: Timestamp },
}

/// Whether the pull request's branch can merge into the default branch, as
/// the store reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mergeability {
    Clean,
    /// The default branch has commits the branch lacks.
    Behind,
    Conflicting,
    /// The branch merges cleanly, but the default branch's rules hold the
    /// merge until what they require is met: `requires`, as far as the store
    /// can name it.
    Blocked {
        requires: Vec<Requirement>,
    },
    /// Not computed yet. GitHub computes none for a draft.
    Unknown,
}

/// Something the default branch's rules require before a pull request
/// merges
/// (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches>).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Requirement {
    /// An approving review.
    Approval,
    /// An approving review from a code owner of the files changed: one of
    /// `owners`, those GitHub requested review from.
    CodeOwnerReview { owners: Vec<String> },
    /// The required check `name`, passing.
    Check { name: String },
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Requirement::Approval => f.write_str("an approving review"),
            Requirement::CodeOwnerReview { owners } if owners.is_empty() => {
                f.write_str("a code owner's review")
            }
            Requirement::CodeOwnerReview { owners } => {
                let owners: Vec<String> = owners.iter().map(|o| format!("@{o}")).collect();
                write!(f, "a code owner's review ({})", owners.join(" or "))
            }
            Requirement::Check { name } => write!(f, "the required check `{name}` passing"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckState {
    Pending,
    Passed,
    Failed,
}

/// A check run or commit status on the pull request's head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub author: Login,
    pub state: ReviewState,
    pub body: String,
    pub submitted_at: Timestamp,
}

/// A review thread on the diff. The store's id is what replies and
/// resolutions address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub path: String,
    pub resolved: bool,
    pub comments: Vec<Comment>,
}

impl Thread {
    /// Who opened the thread.
    pub fn opened_by(&self) -> Option<&Login> {
        self.comments.first().map(|c| &c.author)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Push {
    pub by: Login,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pull {
    pub number: Number,
    pub title: String,
    pub body: String,
    pub author: Login,
    pub created_at: Timestamp,
    pub head_ref: String,
    pub head_sha: String,
    pub state: PullState,
    /// The issues the pull request closes when merged (`Closes #N`).
    pub closes: Vec<Number>,
    /// Pushes to the head branch, oldest first.
    pub pushes: Vec<Push>,
    pub mergeability: Mergeability,
    pub checks: Vec<Check>,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Review>,
    pub threads: Vec<Thread>,
}

impl Pull {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PullState::Open { .. })
    }
}

/// A pull request merged into the default branch: a candidate spec change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedPull {
    pub number: Number,
    pub title: String,
    pub body: String,
    pub author: Login,
    pub merged_at: Timestamp,
    pub base_ref: String,
    pub labels: Vec<String>,
    /// The paths the pull request touched.
    pub files: Vec<String>,
}

impl MergedPull {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
}

/// A snapshot of the store: what one poll reconciles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub default_branch: String,
    /// Open issues Linewright may act on, and any others the store returns.
    pub issues: Vec<Issue>,
    /// Pull requests by the machine user: open ones, and recently closed ones.
    pub pulls: Vec<Pull>,
    /// Pull requests merged into the default branch since the lookback cutoff.
    pub merged: Vec<MergedPull>,
}

impl Snapshot {
    pub fn issue(&self, number: Number) -> Option<&Issue> {
        self.issues.iter().find(|i| i.number == number)
    }

    pub fn pull(&self, number: Number) -> Option<&Pull> {
        self.pulls.iter().find(|p| p.number == number)
    }

    pub fn merged_pull(&self, number: Number) -> Option<&MergedPull> {
        self.merged.iter().find(|p| p.number == number)
    }
}

/// How a branch is brought up to date with the default branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateMethod {
    Rebase,
    Merge,
}

/// How a pull request is merged into the default branch, as GitHub's merge
/// methods are named
/// (<https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request>).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub const ALL: [MergeMethod; 3] =
        [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase];
}

impl fmt::Display for MergeMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        })
    }
}

impl std::str::FromStr for MergeMethod {
    type Err = ();

    fn from_str(name: &str) -> Result<MergeMethod, ()> {
        MergeMethod::ALL
            .into_iter()
            .find(|method| method.to_string() == name)
            .ok_or(())
    }
}

/// The merge methods a repository allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeMethods(Vec<MergeMethod>);

impl MergeMethods {
    pub fn all() -> MergeMethods {
        MergeMethod::ALL.into_iter().collect()
    }

    pub fn contains(&self, method: MergeMethod) -> bool {
        self.0.contains(&method)
    }
}

impl FromIterator<MergeMethod> for MergeMethods {
    fn from_iter<I: IntoIterator<Item = MergeMethod>>(methods: I) -> MergeMethods {
        MergeMethods(methods.into_iter().collect())
    }
}

/// As a phrase: "only `squash`", "`merge` and `rebase`".
impl fmt::Display for MergeMethods {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<String> = self.0.iter().map(|m| format!("`{m}`")).collect();
        match names.as_slice() {
            [] => f.write_str("none"),
            [only] => write!(f, "only {only}"),
            [rest @ .., last] => write!(f, "{} and {last}", rest.join(", ")),
        }
    }
}

/// A failing check, with what the store can tell about why it failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFailure {
    pub name: String,
    pub summary: String,
    /// The end of the check's log, where the store has one.
    pub log_tail: String,
}

/// The issues a pull request body closes when merged: GitHub's closing
/// keywords followed by `#N`
/// (<https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue>).
pub fn closing_references(body: &str) -> Vec<Number> {
    const KEYWORDS: [&str; 9] = [
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ];
    let words: Vec<&str> = body.split_whitespace().collect();
    words
        .windows(2)
        .filter(|pair| KEYWORDS.contains(&pair[0].trim_end_matches(':').to_lowercase().as_str()))
        .filter_map(|pair| {
            let digits: String = pair[1]
                .strip_prefix('#')?
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok().map(Number)
        })
        .collect()
}
