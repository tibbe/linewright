use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Timestamp = DateTime<Utc>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Number(pub u64);

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Login(pub String);

impl Login {
    pub fn new(login: impl Into<String>) -> Self {
        Login(login.into())
    }

    pub fn is_bot(&self) -> bool {
        self.0.ends_with("[bot]")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_person_besides(&self, linewright: &Login) -> bool {
        self != linewright && !self.is_bot()
    }
}

impl fmt::Display for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub mod label {
    pub const NEEDS_TRIAGE: &str = "needs-triage";
    pub const NEEDS_INFO: &str = "needs-info";
    pub const READY_FOR_AGENT: &str = "ready-for-agent";
    pub const READY_FOR_HUMAN: &str = "ready-for-human";
    pub const BUG: &str = "bug";
    pub const ENHANCEMENT: &str = "enhancement";
    pub const SPEC_PROCESSED: &str = "linewright:spec-processed";

    pub const STATES: [&str; 4] = [NEEDS_TRIAGE, NEEDS_INFO, READY_FOR_AGENT, READY_FOR_HUMAN];

    pub const IN_VIEW: [&str; 3] = [NEEDS_TRIAGE, NEEDS_INFO, READY_FOR_AGENT];

    pub const ANSWERS: [(&str, &str); 2] = [
        (READY_FOR_AGENT, "to implement it as decided"),
        (NEEDS_TRIAGE, "to triage it again"),
    ];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Linked {
    pub number: Number,
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum IssueEvent {
    Labeled {
        label: String,
        at: Timestamp,
    },
    Assigned {
        assignee: Login,
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
    #[serde(default)]
    pub parent: Option<Parent>,
    pub events: Vec<IssueEvent>,
    #[serde(default)]
    pub filed: Option<Filed>,
    #[serde(default)]
    pub closed_by: Vec<ClosingPull>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosingPull {
    pub number: Number,
    pub author: Login,
    #[serde(flatten)]
    pub state: PullState,
}

impl ClosingPull {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PullState::Open { .. })
    }
}

impl From<&Pull> for ClosingPull {
    fn from(pull: &Pull) -> ClosingPull {
        ClosingPull {
            number: pull.number,
            author: pull.author.clone(),
            state: pull.state,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Filed {
    Tracking { spec_change: Number },
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

    pub fn cut_from(&self) -> Option<Number> {
        self.parent.and_then(|parent| parent.spec_change)
    }

    pub fn branch(&self) -> String {
        let mut slug = String::new();
        for c in self.title.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
            if slug.len() >= 40 {
                break;
            }
        }
        let slug = slug.trim_matches('-');
        if slug.is_empty() {
            format!("{}", self.number.0)
        } else {
            format!("{}-{slug}", self.number.0)
        }
    }

    pub fn closed_with_no_open_pull(&self) -> bool {
        !self.open && !self.closed_by.iter().any(ClosingPull::is_open)
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mergeability {
    Clean,
    Behind,
    Conflicting,
    Blocked {
        requires: Vec<Requirement>,
    },
    /// GitHub computes none for a draft.
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rebaseability {
    Rebaseable,
    NotRebaseable,
    Unknown,
}

/// <https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Requirement {
    Approval,
    /// An approving review from a code owner of the files changed: one of
    /// `owners`, those GitHub requested review from. Empty when they can't
    /// all be named.
    CodeOwnerReview {
        owners: Vec<String>,
    },
    Check {
        name: String,
    },
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub path: String,
    pub resolved: bool,
    pub comments: Vec<Comment>,
}

impl Thread {
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
pub struct LabelEvent {
    pub label: String,
    pub change: LabelChange,
    pub at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LabelChange {
    Applied,
    Removed,
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
    pub closes: Vec<Number>,
    /// Oldest first.
    pub pushes: Vec<Push>,
    pub mergeability: Mergeability,
    pub rebaseability: Rebaseability,
    pub checks: Vec<Check>,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Review>,
    pub threads: Vec<Thread>,
    pub labels: Vec<String>,
    /// Oldest first.
    pub label_events: Vec<LabelEvent>,
}

impl Pull {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PullState::Open { .. })
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedPull {
    pub number: Number,
    pub title: String,
    pub body: String,
    pub author: Login,
    pub merged_at: Timestamp,
    pub base_ref: String,
    pub labels: Vec<String>,
    pub closes: Vec<Number>,
    pub files: Vec<String>,
}

impl MergedPull {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
}

/// The name of a branch a run on a ticket could have pushed: one
/// [`Issue::branch`] gives under any title the issue has had.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TicketBranchName {
    name: String,
    issue: Number,
}

impl TicketBranchName {
    pub fn parse(name: &str) -> Option<Self> {
        let (number, slug) = name.split_once('-').unwrap_or((name, ""));
        let digits = |word: &str| !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit());
        let word = |word: &str| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        };
        let named = digits(number)
            && !number.starts_with('0')
            && (slug.is_empty() || slug.split('-').all(word));
        let issue = named.then(|| number.parse().ok()).flatten()?;
        Some(Self {
            name: name.to_string(),
            issue: Number(issue),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.name
    }

    pub fn issue(&self) -> Number {
        self.issue
    }
}

impl TryFrom<String> for TicketBranchName {
    type Error = String;

    fn try_from(name: String) -> Result<Self, String> {
        Self::parse(&name).ok_or_else(|| format!("{name} names no ticket's branch"))
    }
}

impl From<TicketBranchName> for String {
    fn from(name: TicketBranchName) -> String {
        name.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketBranch {
    pub name: TicketBranchName,
    pub issue: BranchIssue,
    pub open_pull_from_branch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchIssue {
    Held,
    Read(Box<Issue>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub default_branch: String,
    pub issues: Vec<Issue>,
    pub pulls: Vec<Pull>,
    pub merged: Vec<MergedPull>,
    #[serde(default)]
    pub ticket_branches: Vec<TicketBranch>,
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

    pub fn pulls_closing(&self, issue: &Issue) -> Vec<ClosingPull> {
        let held = self
            .pulls
            .iter()
            .filter(|p| p.closes.contains(&issue.number))
            .map(ClosingPull::from);
        let others = issue
            .closed_by
            .iter()
            .filter(|p| self.pull(p.number).is_none())
            .cloned();
        let mut pulls: Vec<ClosingPull> = held.chain(others).collect();
        pulls.sort_by_key(|p| p.number);
        pulls
    }

    pub fn ticket_branch(&self, issue: &Issue) -> String {
        let base = issue.branch();
        let taken = |name: &str| self.pulls.iter().any(|p| p.is_open() && p.head_ref == name);
        let mut name = base.clone();
        let mut n = 1;
        while taken(&name) {
            n += 1;
            name = format!("{base}-{n}");
        }
        name
    }

    pub fn put_issue(&mut self, issue: Issue) {
        put(&mut self.issues, issue, |i| i.number);
    }

    pub fn remove_issue(&mut self, number: Number) {
        self.issues.retain(|i| i.number != number);
    }

    pub fn relink(&mut self, number: Number, open: bool) {
        for issue in &mut self.issues {
            for linked in issue.blocked_by.iter_mut().chain(&mut issue.sub_issues) {
                if linked.number == number {
                    linked.open = open;
                }
            }
        }
    }

    pub fn unlink(&mut self, number: Number) {
        for issue in &mut self.issues {
            issue.blocked_by.retain(|linked| linked.number != number);
            issue.sub_issues.retain(|linked| linked.number != number);
        }
    }

    pub fn ended_pulls_closing(&self, number: Number) -> Vec<Number> {
        self.pulls
            .iter()
            .filter(|p| !p.is_open() && p.closes.contains(&number))
            .map(|p| p.number)
            .collect()
    }

    pub fn issues_closing(&self, number: Number, closes: &[Number]) -> Vec<Number> {
        self.issues
            .iter()
            .filter(|i| {
                closes.contains(&i.number)
                    || i.closed_by
                        .iter()
                        .any(|p| p.is_open() && p.number == number)
            })
            .map(|i| i.number)
            .collect()
    }

    pub fn put_pull(&mut self, pull: Pull) {
        put(&mut self.pulls, pull, |p| p.number);
    }

    pub fn remove_pull(&mut self, number: Number) {
        self.pulls.retain(|p| p.number != number);
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateMethod {
    Rebase,
    Merge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateRule {
    Always(UpdateMethod),
    /// Merge, but rebase where the merge has a conflict to resolve.
    RebaseOnConflict,
}

/// <https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request>
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// In the order Linewright prefers them.
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFailure {
    pub name: String,
    pub summary: String,
    /// A commit status has none.
    pub log: Option<CheckLog>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckLog {
    pub step_output: String,
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

/// <https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue>
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
