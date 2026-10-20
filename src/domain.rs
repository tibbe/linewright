use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::phrase::Phrase;

pub type Timestamp = DateTime<Utc>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Number(pub u64);

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// A GitHub login, which GitHub matches regardless of case
/// (<https://docs.github.com/en/get-started/learning-about-github/types-of-github-accounts>):
/// two logins differing only in case are equal.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Login(pub String);

impl PartialEq for Login {
    fn eq(&self, other: &Login) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl Eq for Login {}

impl std::hash::Hash for Login {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for b in self.folded() {
            state.write_u8(b);
        }
        state.write_u8(0xff);
    }
}

impl PartialOrd for Login {
    fn partial_cmp(&self, other: &Login) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Login {
    fn cmp(&self, other: &Login) -> std::cmp::Ordering {
        self.folded().cmp(other.folded())
    }
}

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

    /// `login` if it is one GitHub allows: up to 39 letters, digits and
    /// single hyphens, neither first nor last.
    pub fn parse(login: &str) -> Option<Login> {
        let valid = (1..=39).contains(&login.len())
            && login.chars().all(is_login_char)
            && !login.starts_with('-')
            && !login.ends_with('-')
            && !login.contains("--");
        valid.then(|| Login::new(login))
    }

    /// Whether `text` holds an @-mention of the login.
    pub fn is_mentioned_in(&self, text: &str) -> bool {
        text.match_indices('@').any(|(at, _)| {
            let after = &text[at + 1..];
            let name = &after[..after.find(|c| !is_login_char(c)).unwrap_or(after.len())];
            !text[..at].ends_with(is_login_char) && Login::new(name) == *self
        })
    }

    fn folded(&self) -> impl Iterator<Item = u8> + '_ {
        self.0.bytes().map(|b| b.to_ascii_lowercase())
    }
}

/// Whether `c` may appear in a login.
fn is_login_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// `comment`, opening with a mention of each of `people` it does not mention.
pub fn addressed_to(people: &[Login], comment: &str) -> String {
    let unnamed: Vec<String> = people
        .iter()
        .filter(|person| !person.is_mentioned_in(comment))
        .map(|person| format!("@{person}"))
        .collect();
    if unnamed.is_empty() {
        comment.to_string()
    } else {
        format!("{}\n\n{comment}", unnamed.join(" "))
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
    pub const DESIGNED: &str = "linewright:designed";
    /// What [`DESIGNED`] was called before this release: read as it, for
    /// this release, so that no spec change processed before the upgrade is
    /// designed again; never set.
    pub const SPEC_PROCESSED: &str = "linewright:spec-processed";

    /// The labels that bring an issue into the snapshot: those asking for
    /// Linewright's work, and the label a tracking issue cut from an issue
    /// keeps when its status label goes.
    pub const IN_VIEW: [&str; 4] = [NEEDS_TRIAGE, NEEDS_INFO, READY_FOR_AGENT, DESIGNED];

    pub const ANSWERS: [(&str, &str); 2] = [
        (READY_FOR_AGENT, "to implement it as decided"),
        (NEEDS_TRIAGE, "to triage it again"),
    ];

    pub const ALL: [(&str, &str); 7] = [
        (NEEDS_TRIAGE, "Not yet assessed"),
        (
            NEEDS_INFO,
            "Its assessment waits on a fact from the reporter",
        ),
        (READY_FOR_AGENT, "Decided: an agent could build it"),
        (READY_FOR_HUMAN, "Needs a person's decision or action"),
        (BUG, "Something is broken"),
        (ENHANCEMENT, "New feature or improvement"),
        (
            DESIGNED,
            "Designed, and cut into tickets where it needed cutting",
        ),
    ];
}

/// A label that says a merged pull request was designed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesignedLabel {
    #[serde(rename = "linewright:designed")]
    Designed,
    /// See [`label::SPEC_PROCESSED`].
    #[serde(rename = "linewright:spec-processed")]
    SpecProcessed,
}

impl DesignedLabel {
    pub fn name(self) -> &'static str {
        match self {
            DesignedLabel::Designed => label::DESIGNED,
            DesignedLabel::SpecProcessed => label::SPEC_PROCESSED,
        }
    }
}

/// What an issue needs next, as its status label says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatusLabel {
    NeedsTriage,
    NeedsInfo,
    ReadyForAgent,
    ReadyForHuman,
}

impl StatusLabel {
    pub const ALL: [StatusLabel; 4] = [
        StatusLabel::NeedsTriage,
        StatusLabel::NeedsInfo,
        StatusLabel::ReadyForAgent,
        StatusLabel::ReadyForHuman,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusLabel::NeedsTriage => label::NEEDS_TRIAGE,
            StatusLabel::NeedsInfo => label::NEEDS_INFO,
            StatusLabel::ReadyForAgent => label::READY_FOR_AGENT,
            StatusLabel::ReadyForHuman => label::READY_FOR_HUMAN,
        }
    }
}

impl fmt::Display for StatusLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
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

/// A sub-issue, with who opened it: Linewright opens the tickets it cuts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubIssue {
    #[serde(flatten)]
    pub linked: Linked,
    pub author: Login,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum IssueEvent {
    Labeled {
        label: String,
        at: Timestamp,
    },
    Unlabeled {
        label: String,
        at: Timestamp,
    },
    Reopened {
        at: Timestamp,
    },
    #[serde(untagged)]
    Assignment(Assignment),
}

/// A change to an item's assignees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Assignment {
    Assigned {
        assignee: Login,
        actor: Login,
        at: Timestamp,
    },
    Unassigned {
        assignee: Login,
        actor: Login,
        at: Timestamp,
    },
}

impl IssueEvent {
    pub fn at(&self) -> Timestamp {
        match self {
            IssueEvent::Labeled { at, .. }
            | IssueEvent::Unlabeled { at, .. }
            | IssueEvent::Reopened { at } => *at,
            IssueEvent::Assignment(
                Assignment::Assigned { at, .. } | Assignment::Unassigned { at, .. },
            ) => *at,
        }
    }
}

impl Assignment {
    pub fn assignee(&self) -> &Login {
        match self {
            Assignment::Assigned { assignee, .. } | Assignment::Unassigned { assignee, .. } => {
                assignee
            }
        }
    }
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
    pub sub_issues: Vec<SubIssue>,
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

/// What a tracking issue tracks: the tickets cut from one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracks {
    /// Those cut from this spec change, under an issue Linewright opened for
    /// them.
    SpecChange(Number),
    /// Those cut from the issue itself.
    Cut,
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

    /// The status label the issue holds: of those it carries, the one applied
    /// most recently (docs/specs/repository.md, "status labels"). One whose
    /// application fell out of the events read counts as older than the rest.
    pub fn status_label(&self) -> Option<StatusLabel> {
        StatusLabel::ALL
            .into_iter()
            .filter_map(|status| {
                let carried = self.labels.iter().position(|l| l == status.label())?;
                Some((status, (self.last_labeling(status), carried)))
            })
            .max_by_key(|(_, order)| *order)
            .map(|(status, _)| status)
    }

    /// Whether `status` is the status label the issue holds, rather than one it
    /// merely carries.
    pub fn holds(&self, status: StatusLabel) -> bool {
        self.status_label() == Some(status)
    }

    /// The status labels the issue carries besides the one it holds.
    pub fn stale_status_labels(&self) -> Vec<StatusLabel> {
        let held = self.status_label();
        StatusLabel::ALL
            .into_iter()
            .filter(|s| Some(*s) != held && self.has_label(s.label()))
            .collect()
    }

    pub fn last_labeled(&self, status: StatusLabel) -> Option<Timestamp> {
        self.last_labeling(status).map(|order| order.at)
    }

    /// The status label the issue held just after the event at `when`: the
    /// one it holds, with each label applied or removed since undone.
    pub fn status_label_as_of(&self, when: EventOrder) -> Option<StatusLabel> {
        let (before, mut since): (Vec<_>, Vec<_>) = self
            .events
            .iter()
            .enumerate()
            .map(|(i, e)| (EventOrder::of(i, e), e))
            .partition(|(order, _)| *order <= when);
        since.sort_by_key(|(order, _)| *order);
        let mut then = self.clone();
        for (_, event) in since.into_iter().rev() {
            match event {
                IssueEvent::Labeled { label, .. } => then.labels.retain(|l| l != label),
                IssueEvent::Unlabeled { label, .. } => then.labels.push(label.clone()),
                IssueEvent::Reopened { .. } | IssueEvent::Assignment(_) => {}
            }
        }
        then.events = before.into_iter().map(|(_, e)| e.clone()).collect();
        then.status_label()
    }

    /// When `status` was last applied, with the event's place among the events
    /// to order labels applied at the same time.
    fn last_labeling(&self, status: StatusLabel) -> Option<EventOrder> {
        last_labeling(&self.events, status.label())
    }

    pub fn cut_from(&self) -> Option<Number> {
        self.parent.and_then(|parent| parent.spec_change)
    }

    /// Whether it is a ticket cut from its parent's item: a sub-issue of a
    /// spec change's tracking issue, or one `linewright` opened under the
    /// issue it cut. Its design is then that item's, not its own
    /// (docs/specs/repository.md, "Handing back").
    pub fn was_cut(&self, linewright: &Login) -> bool {
        self.cut_from().is_some() || (self.parent.is_some() && self.author == *linewright)
    }

    /// Its sub-issues `linewright` opened: the tickets it cut from the issue.
    pub fn tickets_cut<'a>(&'a self, linewright: &'a Login) -> impl Iterator<Item = &'a SubIssue> {
        self.sub_issues
            .iter()
            .filter(move |sub_issue| sub_issue.author == *linewright)
    }

    /// What it tracks, if it is a tracking issue (docs/specs/design.md,
    /// "Cutting"). An issue cut into tickets is labeled designed last, so one
    /// with tickets but no label is a cut that stopped part way, and tracks
    /// nothing yet.
    pub fn tracks(&self, linewright: &Login) -> Option<Tracks> {
        match self.filed {
            Some(Filed::Tracking { spec_change }) => Some(Tracks::SpecChange(spec_change)),
            _ if self.has_label(label::DESIGNED)
                && self.tickets_cut(linewright).next().is_some() =>
            {
                Some(Tracks::Cut)
            }
            _ => None,
        }
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

    pub fn last_reopened(&self) -> Option<Timestamp> {
        self.events
            .iter()
            .filter_map(|e| {
                if let IssueEvent::Reopened { at } = e {
                    Some(*at)
                } else {
                    None
                }
            })
            .max()
    }

    /// The people a `needs-info` question asks: those `named`, or the
    /// reporter when it names none, `linewright` never.
    pub fn people_asked(&self, named: &[Login], linewright: &Login) -> Vec<Login> {
        let named: Vec<Login> = named.iter().filter(|p| *p != linewright).cloned().collect();
        if !named.is_empty() {
            named
        } else if &self.author != linewright {
            vec![self.author.clone()]
        } else {
            vec![]
        }
    }

    /// The other assignees `actor` assigned: those whose last assignment
    /// among the events `actor` made. One whose assignment fell out of the
    /// events read counts as assigned by someone else.
    pub fn assigned_by(&self, actor: &Login) -> Vec<Login> {
        self.assignees
            .iter()
            .filter(|a| {
                *a != actor && last_assignment(&self.events, a).is_some_and(|(_, by)| by == actor)
            })
            .cloned()
            .collect()
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

impl Requirement {
    pub(crate) fn describe(&self) -> Phrase {
        match self {
            Requirement::Approval => "an approving review".into(),
            Requirement::CodeOwnerReview { owners } if owners.is_empty() => {
                "a code owner's review".into()
            }
            Requirement::CodeOwnerReview { owners } => {
                let owners: Vec<String> = owners.iter().map(|o| format!("@{o}")).collect();
                format!("a code owner's review ({})", owners.join(" or ")).into()
            }
            Requirement::Check { name } => Phrase::from("the required check ")
                .code(name)
                .text(" passing"),
        }
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.describe().fmt(f)
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
    pub assignees: Vec<Login>,
    /// Oldest first.
    pub assignments: Vec<Assignment>,
}

impl Pull {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PullState::Open { .. })
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    pub fn is_assigned(&self, login: &Login) -> bool {
        self.assignees.contains(login)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Item {
    Issue(Number),
    Pull(Number),
}

impl Item {
    pub fn number(self) -> Number {
        match self {
            Item::Issue(number) | Item::Pull(number) => number,
        }
    }
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
    #[serde(default)]
    pub assignees: Vec<Login>,
    /// Its assignments and labelings.
    #[serde(default)]
    pub events: Vec<IssueEvent>,
}

impl MergedPull {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    /// The label that says it was designed, if it carries one.
    pub fn designed_label(&self) -> Option<DesignedLabel> {
        [DesignedLabel::Designed, DesignedLabel::SpecProcessed]
            .into_iter()
            .find(|designed| self.has_label(designed.name()))
    }

    pub fn is_assigned(&self, login: &Login) -> bool {
        self.assignees.contains(login)
    }

    /// When `linewright` was last unassigned from it, by anyone, if
    /// `required_label`, when one is set, was not applied since.
    pub fn unassigned_since_labeled(
        &self,
        linewright: &Login,
        required_label: Option<&str>,
    ) -> Option<Timestamp> {
        let unassigned = last_unassigned(&self.events, linewright)?;
        let labeled = required_label
            .and_then(|required| last_labeling(&self.events, required))
            .map(|order| order.at);
        labeled
            .is_none_or(|labeled| labeled < unassigned)
            .then_some(unassigned)
    }
}

/// When an event happened, with its place among the events read to order it
/// against events at the same time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventOrder {
    pub at: Timestamp,
    pub index: usize,
}

impl EventOrder {
    pub fn new(at: Timestamp, index: usize) -> Self {
        EventOrder { at, index }
    }

    /// The order of `event`, the `index`th of the events read.
    pub fn of(index: usize, event: &IssueEvent) -> Self {
        EventOrder::new(event.at(), index)
    }
}

/// When `name` was last applied among `events`, with the event's place among
/// them to order labels applied at the same time.
pub fn last_labeling(events: &[IssueEvent], name: &str) -> Option<EventOrder> {
    events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if let IssueEvent::Labeled { label, at } = e
                && label == name
            {
                Some(EventOrder::new(*at, i))
            } else {
                None
            }
        })
        .max()
}

/// When `assignee` was last assigned among `events`, by anyone.
pub fn last_assigned(events: &[IssueEvent], assignee: &Login) -> Option<Timestamp> {
    last_assignment(events, assignee).map(|(order, _)| order.at)
}

/// [`last_assigned`], with the event's place among `events` to order it
/// against events at the same time, and who made the assignment.
pub fn last_assignment<'a>(
    events: &'a [IssueEvent],
    assignee: &Login,
) -> Option<(EventOrder, &'a Login)> {
    events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if let IssueEvent::Assignment(Assignment::Assigned {
                assignee: assigned,
                actor,
                at,
            }) = e
                && assigned == assignee
            {
                Some((EventOrder::new(*at, i), actor))
            } else {
                None
            }
        })
        .max_by_key(|(order, _)| *order)
}

/// When `assignee` was last unassigned among `events`, by anyone.
pub fn last_unassigned(events: &[IssueEvent], assignee: &Login) -> Option<Timestamp> {
    last_unassignment(events, assignee).map(|order| order.at)
}

/// [`last_unassigned`], with the event's place among `events` to order it
/// against events at the same time.
pub fn last_unassignment(events: &[IssueEvent], assignee: &Login) -> Option<EventOrder> {
    events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if let IssueEvent::Assignment(Assignment::Unassigned {
                assignee: unassigned,
                at,
                ..
            }) = e
                && unassigned == assignee
            {
                Some(EventOrder::new(*at, i))
            } else {
                None
            }
        })
        .max()
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
            let sub_issues = issue.sub_issues.iter_mut().map(|s| &mut s.linked);
            for linked in issue.blocked_by.iter_mut().chain(sub_issues) {
                if linked.number == number {
                    linked.open = open;
                }
            }
        }
    }

    pub fn unlink(&mut self, number: Number) {
        for issue in &mut self.issues {
            issue.blocked_by.retain(|linked| linked.number != number);
            issue.sub_issues.retain(|s| s.linked.number != number);
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
