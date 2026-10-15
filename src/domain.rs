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

    /// A person other than Linewright: someone to ask for a review.
    pub fn is_person_besides(&self, linewright: &Login) -> bool {
        self != linewright && !self.is_bot()
    }
}

impl fmt::Display for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The labels of `docs/specs/repository.md`.
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

    /// The labels that bring an open issue into the store's snapshot,
    /// besides being assigned to or opened by Linewright.
    pub const IN_VIEW: [&str; 3] = [NEEDS_TRIAGE, NEEDS_INFO, READY_FOR_AGENT];

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
    /// The issue this one is a sub-issue of, if any.
    #[serde(default)]
    pub parent: Option<Parent>,
    pub events: Vec<IssueEvent>,
    /// What the issue is for, if Linewright filed it.
    #[serde(default)]
    pub filed: Option<Filed>,
    /// The open pull requests that close the issue when merged, whoever
    /// opened them.
    #[serde(default)]
    pub closed_by: Vec<ClosingPull>,
}

/// An open pull request that closes an issue when merged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosingPull {
    pub number: Number,
    pub author: Login,
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
    pub fn read(author: &Login, title: &str, body: &str, linewright: &Login) -> Option<Filed> {
        if author != linewright {
            return None;
        }
        if let Some(spec_change) = crate::tracking::spec_change_of(title, body) {
            return Some(Filed::Tracking { spec_change });
        }
        crate::spec_gap::ticket_of(body).map(|ticket| Filed::SpecGap { ticket })
    }
}

/// An issue's parent issue, with the spec change it tracks when it is a
/// tracking issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent {
    pub number: Number,
    pub spec_change: Option<Number>,
}

impl Parent {
    pub fn new(number: Number, filed: Option<Filed>) -> Parent {
        Parent {
            number,
            spec_change: match filed {
                Some(Filed::Tracking { spec_change }) => Some(spec_change),
                _ => None,
            },
        }
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

/// Whether the store can rebase the pull request's commits onto the default
/// branch, as it reports it: what merging it by rebase needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rebaseability {
    Rebaseable,
    NotRebaseable,
    /// Not computed yet.
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
    /// `owners`, those GitHub requested review from. Empty when they can't
    /// all be named.
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
    pub rebaseability: Rebaseability,
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

/// An issue or pull request the store holds. GitHub numbers both from
/// one sequence, but reads them differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Item {
    Issue(Number),
    Pull(Number),
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Issue(number) => write!(f, "issue {number}"),
            Item::Pull(number) => write!(f, "pull request {number}"),
        }
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
    /// The issues the pull request closes when merged (`Closes #N`).
    pub closes: Vec<Number>,
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
    /// Pull requests by Linewright: open ones, and recently closed ones.
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

    /// Puts `issue`, read again, in place of the one the snapshot holds, or
    /// beside the others.
    pub fn put_issue(&mut self, issue: Issue) {
        put(&mut self.issues, issue, |i| i.number);
    }

    pub fn remove_issue(&mut self, number: Number) {
        self.issues.retain(|i| i.number != number);
    }

    /// Records whether the issue `number` is `open`, read again, where the
    /// issues the snapshot holds link to it as a blocker or a sub-issue.
    pub fn relink(&mut self, number: Number, open: bool) {
        for issue in &mut self.issues {
            for linked in issue.blocked_by.iter_mut().chain(&mut issue.sub_issues) {
                if linked.number == number {
                    linked.open = open;
                }
            }
        }
    }

    /// Drops the links to the issue `number`, which the store no longer has,
    /// from the issues the snapshot holds.
    pub fn unlink(&mut self, number: Number) {
        for issue in &mut self.issues {
            issue.blocked_by.retain(|linked| linked.number != number);
            issue.sub_issues.retain(|linked| linked.number != number);
        }
    }

    /// The closed and merged pull requests the snapshot holds that close the
    /// issue `number`.
    pub fn ended_pulls_closing(&self, number: Number) -> Vec<Number> {
        self.pulls
            .iter()
            .filter(|p| !p.is_open() && p.closes.contains(&number))
            .map(|p| p.number)
            .collect()
    }

    /// The issues the snapshot holds that the open pull request `number`
    /// closes, as they were last read.
    pub fn issues_closed_by(&self, number: Number) -> Vec<Number> {
        self.issues
            .iter()
            .filter(|i| i.closed_by.iter().any(|p| p.number == number))
            .map(|i| i.number)
            .collect()
    }

    /// Puts `pull`, read again, in place of the one the snapshot holds, or
    /// beside the others.
    pub fn put_pull(&mut self, pull: Pull) {
        put(&mut self.pulls, pull, |p| p.number);
    }

    pub fn remove_pull(&mut self, number: Number) {
        self.pulls.retain(|p| p.number != number);
    }

    /// Puts `pull`, read again, in place of the merged one the snapshot
    /// holds, or beside the others.
    pub fn put_merged(&mut self, pull: MergedPull) {
        put(&mut self.merged, pull, |p| p.number);
    }

    pub fn remove_merged(&mut self, number: Number) {
        self.merged.retain(|p| p.number != number);
    }
}

/// Puts `item` in place of the one in `items` with its number, or before the
/// first with a higher number, so that items in number order stay so.
fn put<T>(items: &mut Vec<T>, item: T, number: impl Fn(&T) -> Number) {
    if let Some(held) = items.iter_mut().find(|i| number(i) == number(&item)) {
        *held = item;
    } else {
        let at = items.iter().position(|i| number(i) > number(&item));
        items.insert(at.unwrap_or(items.len()), item);
    }
}

/// How a branch is brought up to date with the default branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateMethod {
    Rebase,
    Merge,
}

/// Which method brings a branch up to date with the default branch
/// (`docs/specs/pull_requests.md`, "Keeping up to date").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateRule {
    Always(UpdateMethod),
    /// Merge, but rebase where the merge has a conflict to resolve: a
    /// conflict resolved in a merge commit leaves the branch impossible to
    /// rebase-merge.
    RebaseOnConflict,
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
    /// Every merge method, in the order Linewright prefers them
    /// (`docs/specs/pull_requests.md`, "Auto-merge").
    pub const ALL: [MergeMethod; 3] =
        [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase];
}

impl fmt::Display for MergeMethod {
    /// As the `merge-method` setting names it.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Its log, where it has one: a commit status has none.
    pub log: Option<CheckLog>,
}

/// What a fix session is given of a failing check's log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckLog {
    /// The end of the output of the check's failed step.
    pub step_output: String,
    /// Where to read the whole log, where the store can tell.
    pub full: Option<JobLog>,
}

/// The log of a GitHub Actions job, which `gh run view --job <job> --log`
/// reads (<https://cli.github.com/manual/gh_run_view>).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLog {
    /// The job's repository, as `owner/name`.
    pub repo: String,
    pub job: u64,
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
