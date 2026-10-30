use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};
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

/// A GitHub repository, as `owner/name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    full: String,
    /// Where `full`'s `/` is.
    slash: usize,
}

impl Repo {
    pub fn owner(&self) -> &str {
        &self.full[..self.slash]
    }

    pub fn name(&self) -> &str {
        &self.full[self.slash + 1..]
    }
}

impl std::str::FromStr for Repo {
    type Err = anyhow::Error;

    fn from_str(full: &str) -> anyhow::Result<Repo> {
        match full.split_once('/') {
            Some((owner, name)) if !owner.is_empty() && !name.is_empty() => Ok(Repo {
                full: full.to_string(),
                slash: owner.len(),
            }),
            _ => anyhow::bail!("the repository {full:?} is not owner/name"),
        }
    }
}

/// `owner/name`, as the commands and the pages name the repository.
impl std::ops::Deref for Repo {
    type Target = str;

    fn deref(&self) -> &str {
        &self.full
    }
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.full)
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
    /// See [`super::HandBackKind::Stall`].
    pub const STALLED: &str = "linewright:stalled";
    /// What [`DESIGNED`] was called before this release: read as it, for
    /// this release, so that no spec change processed before the upgrade is
    /// designed again; never set.
    pub const SPEC_PROCESSED: &str = "linewright:spec-processed";

    pub const ANSWERS: [(&str, &str); 2] = [
        (READY_FOR_AGENT, "to implement it as decided"),
        (NEEDS_TRIAGE, "to triage it again"),
    ];

    /// One of Linewright's labels, with what it means.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Label {
        pub name: &'static str,
        pub description: &'static str,
    }

    const fn label(name: &'static str, description: &'static str) -> Label {
        Label { name, description }
    }

    pub const ALL: [Label; 8] = [
        label(NEEDS_TRIAGE, "Not yet assessed"),
        label(
            NEEDS_INFO,
            "Its assessment waits on a fact only a person knows",
        ),
        label(READY_FOR_AGENT, "Decided: an agent could build it"),
        label(READY_FOR_HUMAN, "Needs a person's decision or action"),
        label(BUG, "Something is broken"),
        label(ENHANCEMENT, "New feature or improvement"),
        label(
            DESIGNED,
            "Designed, and cut into tickets where it needed cutting",
        ),
        label(STALLED, "Handed back in a stall"),
    ];

    /// A label in the repository, named as the repository spells it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct RepoLabel {
        pub name: String,
        pub description: Option<String>,
    }

    /// What names the labels only Linewright uses.
    const PREFIX: &str = "linewright:";

    /// A change Linewright makes to the repository's labels at startup.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum LabelChange {
        Create(Label),
        /// Sets the description of the label the repository spells `name`.
        Describe {
            name: String,
            description: &'static str,
        },
    }

    /// The changes that give the repository the labels in `wanted` it lacks,
    /// matching names case-insensitively as GitHub does, and set the
    /// description of each `linewright:` label it has to its meaning, leaving
    /// every other label as it is (`docs/specs/repository.md`, Labels).
    pub fn label_changes(existing: &[RepoLabel], wanted: &[Label]) -> Vec<LabelChange> {
        wanted
            .iter()
            .filter_map(|wanted| {
                let found = existing
                    .iter()
                    .find(|label| label.name.to_lowercase() == wanted.name.to_lowercase());
                match found {
                    None => Some(LabelChange::Create(*wanted)),
                    Some(label)
                        if wanted.name.starts_with(PREFIX)
                            && label.description.as_deref() != Some(wanted.description) =>
                    {
                        Some(LabelChange::Describe {
                            name: label.name.clone(),
                            description: wanted.description,
                        })
                    }
                    Some(_) => None,
                }
            })
            .collect()
    }
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

    /// Whose work it asks for (`docs/specs/repository.md`, Labels).
    pub fn asks(self) -> Asks {
        match self {
            StatusLabel::NeedsTriage => Asks::Linewright(Asked::Triage),
            StatusLabel::ReadyForAgent => Asks::Linewright(Asked::DesignAndImplementation),
            StatusLabel::NeedsInfo | StatusLabel::ReadyForHuman => Asks::Person,
        }
    }
}

/// Whose work a status label asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asks {
    Linewright(Asked),
    Person,
}

/// The work a status label asks of Linewright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    Triage,
    DesignAndImplementation,
}

impl Asked {
    /// The status label that asks for it, as [`StatusLabel::asks`] tells.
    pub fn label(self) -> StatusLabel {
        StatusLabel::ALL
            .into_iter()
            .find(|status| status.asks() == Asks::Linewright(self))
            .expect("a status label asks for each work")
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

impl Comment {
    /// Its writing, as an event.
    pub fn event(&self) -> IssueEvent {
        IssueEvent::Commented {
            by: self.author.clone(),
            at: self.created_at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Linked {
    pub number: Number,
    pub open: bool,
}

/// A sub-issue, with who opened it: Linewright opens the tickets it cuts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SubIssueRecord", into = "SubIssueRecord")]
pub struct SubIssue {
    pub number: Number,
    pub author: Login,
    pub state: SubIssueState,
}

/// Whether a sub-issue is open, or closed and when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubIssueState {
    Open,
    /// `at` is unknown in a snapshot written before sub-issues held it, and
    /// until a full read after a refresh found it closed without its close
    /// time.
    Closed {
        at: Option<Timestamp>,
    },
}

impl SubIssueState {
    /// Open, or closed at an unknown time.
    pub fn open(open: bool) -> SubIssueState {
        if open {
            SubIssueState::Open
        } else {
            SubIssueState::Closed { at: None }
        }
    }
}

impl SubIssue {
    pub fn is_open(&self) -> bool {
        self.state == SubIssueState::Open
    }

    pub fn linked(&self) -> Linked {
        Linked {
            number: self.number,
            open: self.is_open(),
        }
    }
}

/// A [`SubIssue`] as a snapshot holds it.
#[derive(Serialize, Deserialize)]
struct SubIssueRecord {
    #[serde(flatten)]
    linked: Linked,
    author: Login,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    closed_at: Option<Timestamp>,
}

impl From<SubIssueRecord> for SubIssue {
    fn from(record: SubIssueRecord) -> SubIssue {
        SubIssue {
            number: record.linked.number,
            author: record.author,
            state: if record.linked.open {
                SubIssueState::Open
            } else {
                SubIssueState::Closed {
                    at: record.closed_at,
                }
            },
        }
    }
}

impl From<SubIssue> for SubIssueRecord {
    fn from(sub_issue: SubIssue) -> SubIssueRecord {
        SubIssueRecord {
            linked: sub_issue.linked(),
            author: sub_issue.author,
            closed_at: match sub_issue.state {
                SubIssueState::Open => None,
                SubIssueState::Closed { at } => at,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum IssueEvent {
    Labeled {
        label: String,
        /// Who applied it: `None` where GitHub gives no actor, as for a
        /// deleted account, or in a snapshot written before Linewright read
        /// it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<Login>,
        at: Timestamp,
    },
    Unlabeled {
        label: String,
        at: Timestamp,
    },
    Reopened {
        at: Timestamp,
    },
    /// A comment written on it.
    Commented {
        by: Login,
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
            | IssueEvent::Reopened { at }
            | IssueEvent::Commented { at, .. } => *at,
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
    /// When GitHub last updated it; the epoch in a snapshot written before
    /// Linewright read it.
    #[serde(default)]
    pub updated_at: Timestamp,
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
    /// As [`Pull::updated_at`].
    #[serde(default)]
    pub updated_at: Timestamp,
    /// As [`Pull::base_ref`].
    #[serde(default)]
    pub base_ref: String,
}

impl ClosingPull {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PullState::Open { .. })
    }
}

/// The merge of a pull request that closes an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Merge {
    pub pull_request: Number,
    pub at: Timestamp,
}

/// Of `closing`, the pull requests that close an issue with `events`, the
/// first to merge into `default_branch` since the issue was last opened
/// (`docs/specs/design_system.md`, The board), by number where two merged
/// at once. A pull request whose base is not known merged into no branch it
/// can tell.
fn merged_since_opened(
    closing: &[ClosingPull],
    events: &[IssueEvent],
    default_branch: &str,
) -> Option<Merge> {
    let reopened = last_reopened(events);
    closing
        .iter()
        .filter(|pull| !pull.base_ref.is_empty() && pull.base_ref == default_branch)
        .filter_map(|pull| match pull.state {
            PullState::Merged { at } => Some(Merge {
                pull_request: pull.number,
                at,
            }),
            PullState::Open { .. } | PullState::Closed { .. } => None,
        })
        .filter(|merge| reopened.is_none_or(|reopened| merge.at > reopened))
        .min_by_key(|merge| (merge.at, merge.pull_request))
}

fn last_reopened(events: &[IssueEvent]) -> Option<Timestamp> {
    events
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

impl From<&Pull> for ClosingPull {
    fn from(pull: &Pull) -> ClosingPull {
        ClosingPull {
            number: pull.number,
            author: pull.author.clone(),
            state: pull.state,
            updated_at: pull.updated_at,
            base_ref: pull.base_ref.clone(),
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

/// Whether an item is marked for Linewright (`GLOSSARY.md`, **Marked for
/// Linewright**).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marking {
    Marked,
    Unmarked,
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

/// What an issue filed as `filed`, labeled `labels`, with `sub_issues`,
/// tracks, as [`Issue::tracks`] tells.
fn tracks(
    filed: Option<Filed>,
    labels: &[String],
    sub_issues: &[SubIssue],
    linewright: &Login,
) -> Option<Tracks> {
    match filed {
        Some(Filed::Tracking { spec_change }) => Some(Tracks::SpecChange(spec_change)),
        _ if labels.iter().any(|l| l == label::DESIGNED)
            && tickets_cut(sub_issues, linewright).next().is_some() =>
        {
            Some(Tracks::Cut)
        }
        _ => None,
    }
}

impl Tracks {
    /// Those of `sub_issues` that are its tickets: for a cut issue, those
    /// `linewright` opened.
    pub fn tickets<'a>(
        self,
        sub_issues: &'a [SubIssue],
        linewright: &'a Login,
    ) -> impl Iterator<Item = &'a SubIssue> {
        sub_issues.iter().filter(move |s| match self {
            Tracks::SpecChange(_) => true,
            Tracks::Cut => is_cut(s, linewright),
        })
    }
}

/// When `tickets` finished: once every one is closed, when the last of them
/// closed. `None` while one is open, or when one's close time is unknown.
pub fn tickets_finished(tickets: &[&SubIssue]) -> Option<Timestamp> {
    tickets
        .iter()
        .map(|ticket| match ticket.state {
            SubIssueState::Open => None,
            SubIssueState::Closed { at } => at,
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()
}

/// Those of `sub_issues` `linewright` opened: the tickets it cut.
fn tickets_cut<'a>(
    sub_issues: &'a [SubIssue],
    linewright: &'a Login,
) -> impl Iterator<Item = &'a SubIssue> {
    sub_issues.iter().filter(move |s| is_cut(s, linewright))
}

/// Whether an issue under `parent`, opened by `author`, is a ticket cut
/// from its parent's item, as [`Issue::was_cut`] tells.
fn was_cut(parent: Option<Parent>, author: Option<&Login>, linewright: &Login) -> bool {
    parent.is_some_and(|parent| parent.spec_change.is_some() || author == Some(linewright))
}

fn is_cut(sub_issue: &SubIssue, linewright: &Login) -> bool {
    sub_issue.author == *linewright
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

    /// The open issues that block it.
    pub fn open_blockers(&self) -> Vec<Number> {
        self.blocked_by
            .iter()
            .filter(|l| l.open)
            .map(|l| l.number)
            .collect()
    }

    /// Whether it was handed back in a stall, and Linewright has not worked
    /// it since (`docs/specs/repository.md`, "Stalls").
    pub fn is_stalled(&self) -> bool {
        self.has_label(label::STALLED)
    }

    /// The status label the issue holds: of those it carries, the one applied
    /// most recently (docs/specs/repository.md, "status labels"). One whose
    /// application fell out of the events read counts as older than the rest.
    pub fn status_label(&self) -> Option<StatusLabel> {
        status_label(&self.labels, &self.events)
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
                IssueEvent::Reopened { .. }
                | IssueEvent::Commented { .. }
                | IssueEvent::Assignment(_) => {}
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
        was_cut(self.parent, Some(&self.author), linewright)
    }

    /// Its sub-issues `linewright` opened: the tickets it cut from the issue.
    pub fn tickets_cut<'a>(&'a self, linewright: &'a Login) -> impl Iterator<Item = &'a SubIssue> {
        tickets_cut(&self.sub_issues, linewright)
    }

    /// What it tracks, if it is a tracking issue (docs/specs/design.md,
    /// "Cutting"). An issue cut into tickets is labeled designed last, so one
    /// with tickets but no label is a cut that stopped part way, and tracks
    /// nothing yet.
    pub fn tracks(&self, linewright: &Login) -> Option<Tracks> {
        tracks(self.filed, &self.labels, &self.sub_issues, linewright)
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

    /// Its events with its comments among them, as GitHub's timeline holds
    /// them.
    fn timeline(&self) -> Vec<IssueEvent> {
        let comments = self.comments.iter().map(Comment::event);
        self.events.iter().cloned().chain(comments).collect()
    }

    /// What tells whether `linewright` manages it, with `closing` the pull
    /// requests that close it and `timeline` its [`Issue::timeline`].
    fn signs<'a>(
        &'a self,
        linewright: &Login,
        closing: &'a [ClosingPull],
        timeline: &'a [IssueEvent],
    ) -> ManagedSigns<'a> {
        ManagedSigns {
            open: self.open,
            labels: &self.labels,
            assignees: &self.assignees,
            work: WorkSigns {
                author: Some(&self.author),
                events: timeline,
                closed_by: closing,
                tracks: self.tracks(linewright).is_some(),
            },
        }
    }

    /// Whether `linewright` has worked it (`GLOSSARY.md`, **Worked**), with
    /// `closing` the pull requests that close it, such as
    /// [`Snapshot::pulls_closing`] gives.
    pub fn worked_by(&self, linewright: &Login, closing: &[ClosingPull]) -> bool {
        let timeline = self.timeline();
        self.signs(linewright, closing, &timeline)
            .work
            .worked_by(linewright)
    }

    /// Whether it asks for `linewright`'s work (`docs/specs/repository.md`,
    /// Managed items): it is open, and assigned to Linewright, such as by
    /// hand, or, with `marking` [`Marking::Marked`], labeled with a status
    /// label that asks for Linewright's work. A spec change waiting for
    /// design is a merged pull request, which `reconcile::spec_change`
    /// tells.
    pub fn asks_for_work(&self, linewright: &Login, marking: Marking) -> bool {
        let timeline = self.timeline();
        self.signs(linewright, &self.closed_by, &timeline)
            .asks_for_work(linewright, marking)
    }

    /// Whether `linewright` may manage it (`docs/specs/repository.md`,
    /// Managed items): it has worked it, as [`Issue::worked_by`] tells with
    /// `closing`, or it asks for Linewright's work, as
    /// [`Issue::asks_for_work`] tells with `marking`. Whether it is managed
    /// is `managed::classify`'s to decide.
    pub fn candidate(&self, linewright: &Login, closing: &[ClosingPull], marking: Marking) -> bool {
        let timeline = self.timeline();
        self.signs(linewright, closing, &timeline)
            .candidate(linewright, marking)
    }

    pub fn last_reopened(&self) -> Option<Timestamp> {
        last_reopened(&self.events)
    }

    /// Of `closing`, the pull requests that close it, such as
    /// [`Snapshot::pulls_closing`] gives, the first to merge into
    /// `default_branch` since it was last opened.
    pub fn merged_since_opened(
        &self,
        closing: &[ClosingPull],
        default_branch: &str,
    ) -> Option<Merge> {
        merged_since_opened(closing, &self.events, default_branch)
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

    /// Its assignees but `login`, in GitHub's order.
    pub fn assignees_but(&self, login: &Login) -> Vec<Login> {
        self.assignees
            .iter()
            .filter(|a| *a != login)
            .cloned()
            .collect()
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
    /// When GitHub last updated it; the epoch in a snapshot written before
    /// Linewright read it.
    #[serde(default)]
    pub updated_at: Timestamp,
    /// The branch it merges into; empty where it is not known, as in a
    /// snapshot written before Linewright read it.
    #[serde(default)]
    pub base_ref: String,
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
    /// Oldest first; none known in a snapshot written before Linewright
    /// read them.
    #[serde(default)]
    pub labelings: Vec<Labeling>,
    pub assignees: Vec<Login>,
    /// Oldest first.
    pub assignments: Vec<Assignment>,
    /// `None` where no review decides whether it merges.
    #[serde(default)]
    pub review_decision: Option<ReviewDecision>,
}

/// A label applied to a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Labeling {
    pub label: String,
    /// As [`IssueEvent::Labeled`]'s.
    pub by: Option<Login>,
    pub at: Timestamp,
}

/// Where a pull request stands with its reviews, as GitHub decides it
/// (<https://docs.github.com/en/graphql/reference/enums#pullrequestreviewdecision>).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewDecision {
    ChangesRequested,
    ReviewRequired,
    Approved,
}

impl Pull {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PullState::Open { .. })
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    /// Whether it was handed back in a stall, and Linewright has not worked
    /// it since (`docs/specs/repository.md`, "Stalls").
    pub fn is_stalled(&self) -> bool {
        self.has_label(label::STALLED)
    }

    pub fn is_assigned(&self, login: &Login) -> bool {
        self.assignees.contains(login)
    }

    /// Whether `linewright` has worked it, as [`PullSigns::worked_by`]
    /// tells.
    pub fn worked_by(&self, linewright: &Login) -> bool {
        let events = pull_events(&self.labelings, &self.assignments, &self.comments);
        self.signs(&events).worked_by(linewright)
    }

    /// Among which pull requests a snapshot holds it: open, as
    /// [`PullSigns::held`] tells; closed, among Linewright's when it is
    /// Linewright's.
    pub fn held(&self, linewright: &Login) -> Option<PullHeld> {
        let events = pull_events(&self.labelings, &self.assignments, &self.comments);
        match self.signs(&events).held(linewright) {
            Some(PullHeld::Peoples) if !self.is_open() => None,
            held => held,
        }
    }

    fn signs<'a>(&'a self, events: &'a [IssueEvent]) -> PullSigns<'a> {
        PullSigns {
            author: &self.author,
            assignees: &self.assignees,
            events,
        }
    }

    /// Whether GitHub requires a review before it merges, such as a code
    /// owner's, that it has not had (`docs/specs/design_system.md`, Cards).
    /// GitHub decides no review when its rules require a code owner's review
    /// but no approvals, so its blocked merge says so too.
    pub fn review_required(&self) -> bool {
        self.review_decision == Some(ReviewDecision::ReviewRequired)
            || matches!(&self.mergeability, Mergeability::Blocked { requires }
            if requires.iter().any(|requirement| matches!(
                requirement,
                Requirement::Approval | Requirement::CodeOwnerReview { .. }
            )))
    }
}

/// A pull request's `labelings`, `assignments` and `comments` as events,
/// oldest first.
pub fn pull_events(
    labelings: &[Labeling],
    assignments: &[Assignment],
    comments: &[Comment],
) -> Vec<IssueEvent> {
    let labelings = labelings.iter().map(|l| IssueEvent::Labeled {
        label: l.label.clone(),
        by: l.by.clone(),
        at: l.at,
    });
    let assignments = assignments.iter().cloned().map(IssueEvent::Assignment);
    let mut events: Vec<IssueEvent> = labelings
        .chain(assignments)
        .chain(comments.iter().map(Comment::event))
        .collect();
    events.sort_by_key(IssueEvent::at);
    events
}

/// What tells whether Linewright worked a pull request, or it asks for
/// Linewright's work (`docs/specs/repository.md`, Managed items): who
/// opened it, who it is assigned to and its labelings, assignments and
/// comments as `events`.
pub struct PullSigns<'a> {
    pub author: &'a Login,
    pub assignees: &'a [Login],
    pub events: &'a [IssueEvent],
}

/// Among which pull requests a snapshot holds an open one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullHeld {
    /// Linewright's own, which reconcile acts on ([`Snapshot::pulls`]).
    Linewrights,
    /// A person's that Linewright may manage ([`Snapshot::peoples_pulls`]).
    Peoples,
}

impl PullSigns<'_> {
    /// Whether `linewright` worked it, as [`Issue::worked_by`] tells: it
    /// opened it, assigned itself to it, labeled it or commented on it.
    pub fn worked_by(&self, linewright: &Login) -> bool {
        WorkSigns {
            author: Some(self.author),
            events: self.events,
            closed_by: &[],
            tracks: false,
        }
        .worked_by(linewright)
    }

    /// Among which pull requests a snapshot holds it while it is open: among
    /// Linewright's when Linewright opened it; among the people's when
    /// Linewright is assigned to it, which asks for its work, or worked it;
    /// otherwise among none.
    pub fn held(&self, linewright: &Login) -> Option<PullHeld> {
        if self.author == linewright {
            Some(PullHeld::Linewrights)
        } else if self.assignees.contains(linewright) || self.worked_by(linewright) {
            Some(PullHeld::Peoples)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Item {
    Issue(Number),
    Pull(Number),
}

/// Whether a hand-back is a stall (`GLOSSARY.md`, **Stall**).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandBackKind {
    /// Labels the item [`label::STALLED`] (`docs/specs/repository.md`,
    /// "Stalls").
    Stall,
    Decision,
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
    /// When GitHub last updated it; the epoch in a snapshot written before
    /// Linewright read it.
    #[serde(default)]
    pub updated_at: Timestamp,
    pub base_ref: String,
    pub labels: Vec<String>,
    pub closes: Vec<Number>,
    pub files: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<Login>,
    /// Its assignments, labelings and comments.
    #[serde(default)]
    pub events: Vec<IssueEvent>,
}

impl MergedPull {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    /// Whether it was handed back in a stall, and Linewright has not worked
    /// it since (`docs/specs/repository.md`, "Stalls").
    pub fn is_stalled(&self) -> bool {
        self.has_label(label::STALLED)
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

    /// Whether `linewright` has worked it, as [`Pull::worked_by`] tells.
    pub fn worked_by(&self, linewright: &Login) -> bool {
        PullSigns {
            author: &self.author,
            assignees: &self.assignees,
            events: &self.events,
        }
        .worked_by(linewright)
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
            if let IssueEvent::Labeled { label, at, .. } = e
                && label == name
            {
                Some(EventOrder::new(*at, i))
            } else {
                None
            }
        })
        .max()
}

/// The status label an issue labeled `labels`, with `events`, holds, as
/// [`Issue::status_label`] tells.
fn status_label(labels: &[String], events: &[IssueEvent]) -> Option<StatusLabel> {
    StatusLabel::ALL
        .into_iter()
        .filter_map(|status| {
            let carried = labels.iter().position(|l| l == status.label())?;
            Some((status, (last_labeling(events, status.label()), carried)))
        })
        .max_by_key(|(_, order)| *order)
        .map(|(status, _)| status)
}

/// What an issue, open or closed, holds that tells whether Linewright
/// manages it (`GLOSSARY.md`, **Managed**) besides whether it is open, as
/// [`Issue`]'s fields of the same names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedFacts {
    pub filed: Option<Filed>,
    pub sub_issues: Vec<SubIssue>,
    pub labels: Vec<String>,
    /// None of it known in a snapshot taken before Linewright read it.
    #[serde(default)]
    pub work: WorkFacts,
}

impl ManagedFacts {
    /// What it tracks, if it is a tracking issue, as [`Issue::tracks`] tells.
    pub fn tracks(&self, linewright: &Login) -> Option<Tracks> {
        tracks(self.filed, &self.labels, &self.sub_issues, linewright)
    }

    /// Whether the issue, under `parent`, is a ticket cut from its parent's
    /// item, as [`Issue::was_cut`] tells.
    pub fn was_cut(&self, parent: Option<Parent>, linewright: &Login) -> bool {
        was_cut(parent, self.work.author.as_ref(), linewright)
    }

    fn signs(&self, open: bool, linewright: &Login) -> ManagedSigns<'_> {
        let work = &self.work;
        ManagedSigns {
            open,
            labels: &self.labels,
            assignees: &work.assignees,
            work: WorkSigns {
                author: work.author.as_ref(),
                events: &work.events,
                closed_by: &work.closed_by,
                tracks: self.tracks(linewright).is_some(),
            },
        }
    }

    /// Whether `linewright` has worked it, as [`Issue::worked_by`] tells.
    pub fn worked_by(&self, linewright: &Login) -> bool {
        self.signs(false, linewright).work.worked_by(linewright)
    }

    /// Whether `linewright` may manage it, `open` or not, as
    /// [`Issue::candidate`] tells of an issue marked for Linewright with the
    /// pull requests closing it.
    pub fn candidate(&self, open: bool, linewright: &Login) -> bool {
        self.signs(open, linewright)
            .candidate(linewright, Marking::Marked)
    }
}

/// What tells whether Linewright may manage an issue
/// (`docs/specs/repository.md`, Managed items).
struct ManagedSigns<'a> {
    open: bool,
    labels: &'a [String],
    assignees: &'a [Login],
    work: WorkSigns<'a>,
}

impl ManagedSigns<'_> {
    /// As [`Issue::asks_for_work`].
    fn asks_for_work(&self, linewright: &Login, marking: Marking) -> bool {
        self.open
            && (self.assignees.contains(linewright)
                || marking == Marking::Marked
                    && status_label(self.labels, self.work.events)
                        .is_some_and(|status| matches!(status.asks(), Asks::Linewright(_))))
    }

    /// As [`Issue::candidate`].
    fn candidate(&self, linewright: &Login, marking: Marking) -> bool {
        self.work.worked_by(linewright) || self.asks_for_work(linewright, marking)
    }
}

/// What tells whether Linewright has worked an item, open or closed
/// (`GLOSSARY.md`, **Worked**).
struct WorkSigns<'a> {
    author: Option<&'a Login>,
    /// Its comments among them.
    events: &'a [IssueEvent],
    closed_by: &'a [ClosingPull],
    /// Whether it is a tracking issue.
    tracks: bool,
}

impl WorkSigns<'_> {
    /// Whether `linewright` worked it (`docs/specs/repository.md`, Managed
    /// items):
    ///
    /// - it filed it, or tracks tickets with it: it opened it;
    /// - it assigned itself to it once;
    /// - it applied a label to it;
    /// - it wrote a comment on it;
    /// - it wrote a pull request closing it: it opened that pull request.
    ///
    /// Being assigned alone is not working it: that asks for its work.
    fn worked_by(&self, linewright: &Login) -> bool {
        self.author == Some(linewright)
            || self.events.iter().any(|event| match event {
                IssueEvent::Labeled { by, .. } => by.as_ref() == Some(linewright),
                IssueEvent::Commented { by, .. } => by == linewright,
                IssueEvent::Assignment(Assignment::Assigned {
                    assignee, actor, ..
                }) => assignee == linewright && actor == linewright,
                IssueEvent::Unlabeled { .. }
                | IssueEvent::Reopened { .. }
                | IssueEvent::Assignment(Assignment::Unassigned { .. }) => false,
            })
            || self.closed_by.iter().any(|pr| pr.author == *linewright)
            || self.tracks
    }
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
    /// Linewright's open pull requests, and those of its closed ones that
    /// close an issue it holds. Reconcile acts on these as Linewright's own.
    pub pulls: Vec<Pull>,
    /// Other people's open pull requests that Linewright may manage
    /// ([`Pull::held`]). Reconcile does not act on them.
    #[serde(default)]
    pub peoples_pulls: Vec<Pull>,
    pub merged: Vec<MergedPull>,
    #[serde(default)]
    pub ticket_branches: Vec<TicketBranch>,
    /// The issues closed within the Done window that Linewright manages
    /// ([`ClosedItem::candidate`]), every closed ticket of a tracking issue
    /// it holds and Linewright's pull requests closed without merging that
    /// close one, whenever they closed, and the pull requests Linewright
    /// worked closed without merging within the Done window, whoever opened
    /// them, for the board's Done column (`docs/specs/design_system.md`,
    /// "The board"), which keeps only those worked. The reconcile ignores
    /// them.
    #[serde(default)]
    pub closed: Vec<ClosedItem>,
}

/// An issue's closing: when, the pull request whose merge closed it where it
/// is known, and GitHub's reason, where it is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IssueClosing {
    pub at: Timestamp,
    pub merged_by: Option<Number>,
    pub reason: Option<IssueCloseReason>,
}

/// An issue or pull request a snapshot holds closed ([`Snapshot::closed`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedItem {
    pub number: Number,
    pub title: String,
    pub closed_at: Timestamp,
    /// When GitHub last updated it; the epoch in a snapshot written before
    /// Linewright read it.
    #[serde(default)]
    pub updated_at: Timestamp,
    #[serde(flatten)]
    pub kind: ClosedKind,
}

impl ClosedItem {
    /// Closed `issue`, as a snapshot holds it with its `closing`.
    pub fn of_issue(issue: &Issue, closing: IssueClosing) -> ClosedItem {
        let mut timeline = issue.timeline();
        timeline.sort_by_key(IssueEvent::at);
        ClosedItem {
            number: issue.number,
            title: issue.title.clone(),
            closed_at: closing.at,
            updated_at: issue.updated_at,
            kind: ClosedKind::Issue {
                parent: issue.parent,
                facts: ManagedFacts {
                    filed: issue.filed,
                    sub_issues: issue.sub_issues.clone(),
                    labels: issue.labels.clone(),
                    work: WorkFacts {
                        author: Some(issue.author.clone()),
                        assignees: issue.assignees.clone(),
                        events: timeline,
                        closed_by: issue.closed_by.clone(),
                    },
                },
                merged_by: closing.merged_by,
                reason: closing.reason,
            },
        }
    }

    /// Its closing, if it is an issue.
    pub fn issue_closing(&self) -> Option<IssueClosing> {
        match &self.kind {
            ClosedKind::Issue {
                merged_by, reason, ..
            } => Some(IssueClosing {
                at: self.closed_at,
                merged_by: *merged_by,
                reason: *reason,
            }),
            ClosedKind::Pull { .. } => None,
        }
    }

    /// The pull requests that close it, in number order, if it is an issue.
    pub fn closing_pulls(&self) -> Vec<ClosingPull> {
        let mut closing = match &self.kind {
            ClosedKind::Issue { facts, .. } => facts.work.closed_by.clone(),
            ClosedKind::Pull { .. } => vec![],
        };
        closing.sort_by_key(|pull| pull.number);
        closing
    }

    /// What it tracks, if it is a tracking issue, as [`Issue::tracks`] tells.
    pub fn tracks(&self, linewright: &Login) -> Option<Tracks> {
        match &self.kind {
            ClosedKind::Issue { facts, .. } => facts.tracks(linewright),
            ClosedKind::Pull { .. } => None,
        }
    }

    /// Whether `linewright` has worked it (`GLOSSARY.md`, **Worked**), as
    /// [`Issue::worked_by`] tells. A pull request is held only when
    /// Linewright worked it.
    pub fn worked_by(&self, linewright: &Login) -> bool {
        match &self.kind {
            ClosedKind::Issue { facts, .. } => facts.worked_by(linewright),
            ClosedKind::Pull { .. } => true,
        }
    }

    /// Whether `linewright` may manage it, as [`Issue::candidate`] tells:
    /// closed, it asks for no work, so it may be managed when Linewright has
    /// worked it.
    pub fn candidate(&self, linewright: &Login) -> bool {
        self.worked_by(linewright)
    }
}

/// The windows a snapshot reads back over (`docs/specs/operation.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Windows {
    /// When the spec lookback window (`spec-lookback`) opened.
    lookback_opened: Timestamp,
    /// When the Done window (`done-window`) opened.
    done_opened: Timestamp,
}

impl Windows {
    /// Windows that opened at the earliest time, so that a snapshot over them
    /// holds every closed and merged item.
    pub const EVER: Windows = Windows {
        lookback_opened: Timestamp::MIN_UTC,
        done_opened: Timestamp::MIN_UTC,
    };

    /// The windows open at `now`, the lookback window `spec_lookback` long
    /// and the Done window `done_window` long.
    pub fn at(now: Timestamp, spec_lookback: TimeDelta, done_window: TimeDelta) -> Windows {
        Windows {
            lookback_opened: now - spec_lookback,
            done_opened: now - done_window,
        }
    }

    /// When the spec lookback window opened.
    pub fn lookback_opened(&self) -> Timestamp {
        self.lookback_opened
    }

    /// How far back, by update time, a snapshot reads closed and merged
    /// items.
    pub fn read_since(&self) -> Timestamp {
        self.lookback_opened.min(self.done_opened)
    }

    /// Whether a snapshot holds a pull request merged at `merged_at` and
    /// last updated at `updated_at` among the merged ones: merged within the
    /// lookback window, or updated within the Done window. A merge or a
    /// designed label updates a pull request, so this holds Linewright's own
    /// merged within the Done window and every spec change designed within
    /// it.
    pub fn holds_merged(&self, merged_at: Timestamp, updated_at: Timestamp) -> bool {
        merged_at >= self.lookback_opened || updated_at >= self.done_opened
    }

    /// Whether a snapshot holds an item closed at `closed_at` among the
    /// closed ones.
    pub fn holds_closed(&self, closed_at: Timestamp) -> bool {
        self.in_done_window(closed_at)
    }

    /// Whether `at` is within the Done window.
    pub fn in_done_window(&self, at: Timestamp) -> bool {
        at >= self.done_opened
    }
}

/// Those of `closed` that `windows` holds, in number order, as a snapshot
/// holds them.
pub fn closed_within(
    closed: impl IntoIterator<Item = ClosedItem>,
    windows: Windows,
) -> Vec<ClosedItem> {
    let mut within: Vec<ClosedItem> = closed
        .into_iter()
        .filter(|item| windows.holds_closed(item.closed_at))
        .collect();
    within.sort_by_key(|item| item.number);
    within
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClosedKind {
    Issue {
        parent: Option<Parent>,
        #[serde(flatten)]
        facts: ManagedFacts,
        /// The pull request whose merge closed it.
        merged_by: Option<Number>,
        /// `None` where it is not known, as in a snapshot taken before
        /// Linewright read it.
        #[serde(default)]
        reason: Option<IssueCloseReason>,
    },
    /// One Linewright worked, closed without merging.
    Pull {
        closes: Vec<Number>,
        /// Who opened it; `None` in a snapshot written before Linewright
        /// read it, which held only Linewright's.
        #[serde(default)]
        author: Option<Login>,
    },
}

/// What a closed issue holds that tells whether Linewright has worked it, as
/// an [`Issue`]'s fields of the same names do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkFacts {
    pub author: Option<Login>,
    pub assignees: Vec<Login>,
    pub events: Vec<IssueEvent>,
    pub closed_by: Vec<ClosingPull>,
}

impl WorkFacts {
    /// As [`Issue::merged_since_opened`], of the pull requests closing it.
    pub fn merged_since_opened(&self, default_branch: &str) -> Option<Merge> {
        merged_since_opened(&self.closed_by, &self.events, default_branch)
    }
}

/// Why an issue was closed, as GitHub records it
/// (<https://docs.github.com/en/graphql/reference/enums#issuestatereason>).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssueCloseReason {
    Completed,
    NotPlanned,
    Duplicate,
}

/// An item as a snapshot holds it, of whichever kind.
enum Held<'a> {
    Issue(&'a Issue),
    Pull(&'a Pull),
    Merged(&'a MergedPull),
    Closed(&'a ClosedItem),
}

impl<'a> Held<'a> {
    fn title(self) -> &'a str {
        match self {
            Held::Issue(issue) => &issue.title,
            Held::Pull(pull) => &pull.title,
            Held::Merged(merged) => &merged.title,
            Held::Closed(closed) => &closed.title,
        }
    }

    fn updated_at(self) -> Timestamp {
        match self {
            Held::Issue(issue) => issue.updated_at,
            Held::Pull(pull) => pull.updated_at,
            Held::Merged(merged) => merged.updated_at,
            Held::Closed(closed) => closed.updated_at,
        }
    }
}

impl Snapshot {
    pub fn issue(&self, number: Number) -> Option<&Issue> {
        self.issues.iter().find(|i| i.number == number)
    }

    pub fn pull(&self, number: Number) -> Option<&Pull> {
        self.pulls.iter().find(|p| p.number == number)
    }

    /// #`number` among Linewright's pull requests or the people's it may
    /// manage.
    pub fn any_pull(&self, number: Number) -> Option<&Pull> {
        self.pull(number)
            .or_else(|| self.peoples_pulls.iter().find(|pull| pull.number == number))
    }

    /// #`number` among the closed items.
    pub fn closed_item(&self, number: Number) -> Option<&ClosedItem> {
        self.closed.iter().find(|item| item.number == number)
    }

    pub fn merged_pull(&self, number: Number) -> Option<&MergedPull> {
        self.merged.iter().find(|p| p.number == number)
    }

    /// As [`Issue::is_stalled`], of #`number`, an issue, a pull request or a
    /// merged one the snapshot holds.
    pub fn is_stalled(&self, number: Number) -> bool {
        self.issue(number).is_some_and(Issue::is_stalled)
            || self.any_pull(number).is_some_and(Pull::is_stalled)
            || self.merged_pull(number).is_some_and(MergedPull::is_stalled)
    }

    /// The title of #`number`, of the issues, pull requests, Linewright's or
    /// the people's, merged ones and closed items the snapshot holds.
    pub fn title(&self, number: Number) -> Option<&str> {
        self.held(number).next().map(Held::title)
    }

    /// When GitHub last updated #`number`, of the issues, pull requests,
    /// merged ones and closed items the snapshot holds.
    pub fn updated_at(&self, number: Number) -> Option<Timestamp> {
        self.held(number).map(Held::updated_at).max()
    }

    /// Each issue, pull request, merged one and closed item the snapshot
    /// holds as #`number`.
    fn held(&self, number: Number) -> impl Iterator<Item = Held<'_>> {
        let issue = self.issue(number).map(Held::Issue);
        let pull = self.any_pull(number).map(Held::Pull);
        let merged = self.merged_pull(number).map(Held::Merged);
        let closed = self
            .closed
            .iter()
            .filter(move |item| item.number == number)
            .map(Held::Closed);
        issue.into_iter().chain(pull).chain(merged).chain(closed)
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

    /// Has the issues linking #`number` see it as `state`.
    pub fn relink(&mut self, number: Number, state: SubIssueState) {
        for issue in &mut self.issues {
            for linked in &mut issue.blocked_by {
                if linked.number == number {
                    linked.open = state == SubIssueState::Open;
                }
            }
            for sub_issue in &mut issue.sub_issues {
                if sub_issue.number == number {
                    sub_issue.state = state;
                }
            }
        }
    }

    pub fn unlink(&mut self, number: Number) {
        for issue in &mut self.issues {
            issue.blocked_by.retain(|linked| linked.number != number);
            issue.sub_issues.retain(|s| s.number != number);
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

    pub fn put_peoples_pull(&mut self, pull: Pull) {
        put(&mut self.peoples_pulls, pull, |p| p.number);
    }

    pub fn remove_peoples_pull(&mut self, number: Number) {
        self.peoples_pulls.retain(|p| p.number != number);
    }

    /// Puts `pull`, read now, among the pull requests a snapshot taken now
    /// holds it among ([`Pull::held`]), and out of the people's otherwise.
    pub fn put_read_pull(&mut self, pull: Pull, linewright: &Login) {
        match pull.held(linewright) {
            Some(PullHeld::Linewrights) => {
                self.remove_peoples_pull(pull.number);
                self.put_pull(pull);
            }
            Some(PullHeld::Peoples) => self.put_peoples_pull(pull),
            None => self.remove_peoples_pull(pull.number),
        }
    }

    pub fn put_merged(&mut self, pull: MergedPull) {
        put(&mut self.merged, pull, |p| p.number);
    }

    pub fn remove_merged(&mut self, number: Number) {
        self.merged.retain(|p| p.number != number);
    }

    pub fn put_closed(&mut self, item: ClosedItem) {
        put(&mut self.closed, item, |i| i.number);
    }

    pub fn remove_closed(&mut self, number: Number) {
        self.closed.retain(|i| i.number != number);
    }

    /// Whether a snapshot holding the rest of what this one holds holds
    /// `item` among the closed ones: closed within the Done window, or,
    /// whenever it closed, a ticket of a tracking issue it holds or a pull
    /// request of Linewright's closing one, as an unfinished lane keeps
    /// every ticket it closed (`docs/specs/design_system.md`, The board,
    /// Finished lanes).
    pub fn keeps_closed(&self, item: &ClosedItem, windows: Windows, linewright: &Login) -> bool {
        let held_ticket = |number: Number| {
            self.held_tickets(linewright)
                .any(|ticket| ticket.number == number)
        };
        windows.holds_closed(item.closed_at)
            || held_ticket(item.number)
            || matches!(&item.kind, ClosedKind::Pull { closes, .. } if closes.iter().any(|&n| held_ticket(n)))
    }

    /// Drops the closed items it would not keep ([`Snapshot::keeps_closed`]),
    /// as a snapshot taken now would not hold them: those of a tracking
    /// issue it no longer holds, or closed before the Done window opened
    /// since it read them.
    pub fn drop_unkept_closed(&mut self, windows: Windows, linewright: &Login) {
        loop {
            let unkept: Vec<Number> = self
                .closed
                .iter()
                .filter(|item| !self.keeps_closed(item, windows, linewright))
                .map(|item| item.number)
                .collect();
            if unkept.is_empty() {
                return;
            }
            // A tracking issue dropped can leave its tickets unkept.
            self.closed.retain(|item| !unkept.contains(&item.number));
        }
    }

    /// The closed tickets of the tracking issues it holds, open or closed,
    /// that it does not yet hold among the closed items, which a store reads
    /// after the rest.
    pub fn unheld_closed_tickets(&self, linewright: &Login) -> BTreeSet<Number> {
        self.held_tickets(linewright)
            .filter(|ticket| !ticket.is_open())
            .map(|ticket| ticket.number)
            .filter(|&number| !self.closed.iter().any(|item| item.number == number))
            .collect()
    }

    /// Linewright's pull requests closed without merging that close the
    /// closed tickets of the tracking issues it holds, that it does not yet
    /// hold among the closed items, which a store reads after those tickets
    /// ([`Snapshot::unheld_closed_tickets`]).
    pub fn unheld_closing_pulls(&self, linewright: &Login) -> BTreeSet<Number> {
        let tickets: BTreeSet<Number> = self
            .held_tickets(linewright)
            .map(|ticket| ticket.number)
            .collect();
        self.closed
            .iter()
            .filter(|item| tickets.contains(&item.number))
            .filter_map(|item| match &item.kind {
                ClosedKind::Issue { facts, .. } => Some(&facts.work.closed_by),
                ClosedKind::Pull { .. } => None,
            })
            .flatten()
            .filter(|pull| {
                pull.author == *linewright && matches!(pull.state, PullState::Closed { .. })
            })
            .map(|pull| pull.number)
            .filter(|&number| !self.closed.iter().any(|item| item.number == number))
            .collect()
    }

    /// The tickets of each tracking issue it holds, open or closed.
    fn held_tickets<'a>(&'a self, linewright: &'a Login) -> impl Iterator<Item = &'a SubIssue> {
        let open = self
            .issues
            .iter()
            .filter(|issue| issue.open)
            .filter_map(|issue| Some((issue.tracks(linewright)?, &issue.sub_issues)));
        let closed = self.closed.iter().filter_map(|item| match &item.kind {
            ClosedKind::Issue { facts, .. } => Some((item.tracks(linewright)?, &facts.sub_issues)),
            ClosedKind::Pull { .. } => None,
        });
        open.chain(closed)
            .flat_map(|(tracks, sub_issues)| tracks.tickets(sub_issues, linewright))
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

/// Merging green pull requests, the `auto-merge` setting turned on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoMerge {
    /// `None` follows the repository's settings.
    pub merge_method: Option<MergeMethod>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// The lookback window opened on 2026-09-29, the Done window on
    /// 2026-09-24.
    fn windows() -> Windows {
        Windows::at(
            at("2026-10-01T12:00:00Z"),
            TimeDelta::days(2),
            TimeDelta::days(7),
        )
    }

    #[test]
    fn a_snapshot_reads_back_to_the_earlier_window() {
        assert_eq!(windows().read_since(), at("2026-09-24T12:00:00Z"));
    }

    #[test]
    fn a_snapshot_holds_a_pull_request_merged_within_the_lookback_or_updated_within_the_done_window()
     {
        let w = windows();
        let held = |merged: &str, updated: &str| w.holds_merged(at(merged), at(updated));

        assert!(held("2026-09-30T12:00:00Z", "2026-09-30T12:00:00Z"));
        assert!(held("2026-09-26T12:00:00Z", "2026-09-26T12:00:00Z"));
        // Merged before both windows, designed within the Done window.
        assert!(held("2026-09-20T12:00:00Z", "2026-09-26T12:00:00Z"));
        assert!(!held("2026-09-20T12:00:00Z", "2026-09-22T12:00:00Z"));
    }

    #[test]
    fn a_snapshot_holds_an_item_closed_within_the_done_window() {
        assert!(windows().holds_closed(at("2026-09-26T12:00:00Z")));
        assert!(!windows().holds_closed(at("2026-09-22T12:00:00Z")));
    }

    /// A snapshot as Linewright wrote it before a sub-issue held when it
    /// closed.
    #[test]
    fn a_snapshot_written_before_sub_issues_held_their_close_time_loads_with_it_unknown() {
        let snapshot: Snapshot = serde_json::from_str(
            r#"{
              "default_branch": "main",
              "issues": [{
                "number": 20, "title": "Tracking 20", "body": "", "author": "factory",
                "open": true, "created_at": "2026-10-01T11:00:00Z", "labels": [],
                "assignees": [], "comments": [], "blocked_by": [],
                "sub_issues": [{"number": 1, "open": false, "author": "factory"}],
                "events": [], "closed_by": []
              }],
              "pulls": [],
              "merged": [],
              "closed": []
            }"#,
        )
        .unwrap();

        assert_eq!(
            snapshot.issues[0].sub_issues[0].state,
            SubIssueState::Closed { at: None }
        );
    }

    /// A snapshot as Linewright wrote it before it held a closed issue's
    /// reason and a pull request's review decision.
    #[test]
    fn a_snapshot_written_before_reasons_and_review_decisions_loads_with_neither_known() {
        let snapshot: Snapshot = serde_json::from_str(
            r#"{
              "default_branch": "main",
              "issues": [],
              "pulls": [{
                "number": 10, "title": "Pull 10", "body": "", "author": "factory",
                "created_at": "2026-10-01T11:58:00Z", "head_ref": "10-pull",
                "head_sha": "abc123", "state": {"state": "open", "draft": false},
                "closes": [1], "pushes": [], "mergeability": "unknown",
                "rebaseability": "unknown", "checks": [], "comments": [], "reviews": [],
                "threads": [], "labels": [], "assignees": ["factory"], "assignments": []
              }],
              "merged": [],
              "closed": [{
                "number": 2, "title": "Ticket 2", "closed_at": "2026-09-28T13:00:00Z",
                "kind": "issue", "parent": null, "filed": null, "sub_issues": [],
                "labels": ["ready-for-agent"], "merged_by": null
              }]
            }"#,
        )
        .unwrap();

        assert_eq!(snapshot.pulls[0].review_decision, None);
        assert!(matches!(
            snapshot.closed[0].kind,
            ClosedKind::Issue { reason: None, .. }
        ));
    }

    /// A snapshot as Linewright wrote it before it held who filed a closed
    /// issue, who was assigned to it and the pull requests closing it: none
    /// of them is known.
    #[test]
    fn a_snapshot_written_before_closed_issues_held_how_they_were_worked_loads_with_none_known() {
        let snapshot: Snapshot = serde_json::from_str(
            r#"{
              "default_branch": "main",
              "issues": [],
              "pulls": [],
              "merged": [],
              "closed": [{
                "number": 2, "title": "Ticket 2", "closed_at": "2026-09-28T13:00:00Z",
                "updated_at": "2026-09-28T13:00:00Z", "kind": "issue", "parent": null,
                "filed": null, "sub_issues": [], "labels": ["ready-for-agent"],
                "merged_by": null, "reason": "completed"
              }]
            }"#,
        )
        .unwrap();

        let ClosedKind::Issue {
            facts: ManagedFacts { work, .. },
            ..
        } = &snapshot.closed[0].kind
        else {
            panic!("an issue");
        };
        assert_eq!(*work, WorkFacts::default());
    }

    /// A snapshot as Linewright wrote it before it held when each item was
    /// last updated: the time is unknown, and counts as the oldest.
    #[test]
    fn a_snapshot_written_before_update_times_loads_with_each_item_updated_at_the_epoch() {
        let snapshot: Snapshot = serde_json::from_str(
            r#"{
              "default_branch": "main",
              "issues": [{
                "number": 1, "title": "Ticket 1", "body": "", "author": "maintainer",
                "open": true, "created_at": "2026-10-01T11:00:00Z", "labels": [],
                "assignees": [], "comments": [], "blocked_by": [], "sub_issues": [],
                "events": [],
                "closed_by": [{"number": 20, "author": "alice", "state": "open", "draft": false}]
              }],
              "pulls": [{
                "number": 10, "title": "Pull 10", "body": "", "author": "factory",
                "created_at": "2026-10-01T11:58:00Z", "head_ref": "10-pull",
                "head_sha": "abc123", "state": {"state": "open", "draft": false},
                "closes": [1], "pushes": [], "mergeability": "unknown",
                "rebaseability": "unknown", "checks": [], "comments": [], "reviews": [],
                "threads": [], "labels": [], "assignees": ["factory"], "assignments": []
              }],
              "merged": [{
                "number": 5, "title": "Spec", "body": "", "author": "maintainer",
                "merged_at": "2026-10-01T09:00:00Z", "base_ref": "main", "labels": [],
                "closes": [], "files": []
              }],
              "closed": [{
                "number": 2, "title": "Ticket 2", "closed_at": "2026-09-28T13:00:00Z",
                "kind": "issue", "parent": null, "filed": null, "sub_issues": [],
                "labels": ["ready-for-agent"], "merged_by": null
              }]
            }"#,
        )
        .unwrap();

        let epoch = Timestamp::UNIX_EPOCH;
        assert_eq!(snapshot.issues[0].updated_at, epoch);
        assert_eq!(snapshot.issues[0].closed_by[0].updated_at, epoch);
        assert_eq!(snapshot.pulls[0].updated_at, epoch);
        assert_eq!(snapshot.merged[0].updated_at, epoch);
        assert_eq!(snapshot.closed[0].updated_at, epoch);
    }

    /// A snapshot as Linewright wrote it before it held people's pull
    /// requests, a pull request's labelings and the author of a pull request
    /// closed unmerged, which was then always Linewright's.
    #[test]
    fn a_snapshot_written_before_peoples_pull_requests_loads_with_none_and_no_labelings_or_author_known()
     {
        let snapshot: Snapshot = serde_json::from_str(
            r#"{
              "default_branch": "main",
              "issues": [],
              "pulls": [{
                "number": 10, "title": "Pull 10", "body": "", "author": "factory",
                "created_at": "2026-10-01T11:58:00Z", "head_ref": "10-pull",
                "head_sha": "abc123", "state": {"state": "open", "draft": false},
                "closes": [1], "pushes": [], "mergeability": "unknown",
                "rebaseability": "unknown", "checks": [], "comments": [], "reviews": [],
                "threads": [], "labels": ["linewright:stalled"], "assignees": ["factory"],
                "assignments": []
              }],
              "merged": [],
              "closed": [{
                "number": 11, "title": "Pull 11", "closed_at": "2026-09-28T13:00:00Z",
                "updated_at": "2026-09-28T13:00:00Z", "kind": "pull", "closes": [2]
              }]
            }"#,
        )
        .unwrap();

        assert_eq!(snapshot.peoples_pulls, []);
        assert_eq!(snapshot.pulls[0].labelings, []);
        assert_eq!(
            snapshot.closed[0].kind,
            ClosedKind::Pull {
                closes: vec![Number(2)],
                author: None,
            }
        );
    }

    fn existing(names: &[&str]) -> Vec<label::RepoLabel> {
        names
            .iter()
            .map(|name| label::RepoLabel {
                name: name.to_string(),
                description: None,
            })
            .collect()
    }

    fn bug() -> label::Label {
        label::Label {
            name: "bug",
            description: "Something is broken",
        }
    }

    fn needs_triage() -> label::Label {
        label::Label {
            name: "needs-triage",
            description: "Not yet assessed",
        }
    }

    #[test]
    fn a_wanted_label_the_repository_lacks_is_created_with_its_description() {
        let changes = label::label_changes(&existing(&["bug"]), &[bug(), needs_triage()]);

        assert_eq!(changes, [label::LabelChange::Create(needs_triage())]);
    }

    #[test]
    fn a_wanted_label_the_repository_has_under_another_case_is_not_created() {
        let changes = label::label_changes(&existing(&["Needs-Triage"]), &[needs_triage()]);

        assert_eq!(changes, []);
    }

    #[test]
    fn a_label_without_the_linewright_prefix_with_another_description_is_left_as_it_is() {
        let changes = label::label_changes(
            &[label::RepoLabel {
                name: "bug".into(),
                description: Some("Broken".into()),
            }],
            &[bug()],
        );

        assert_eq!(changes, []);
    }

    fn stalled() -> label::Label {
        label::Label {
            name: "linewright:stalled",
            description: "Handed back in a stall",
        }
    }

    #[test]
    fn a_linewright_label_with_another_description_is_described_under_the_repositorys_spelling() {
        let changes = label::label_changes(
            &[label::RepoLabel {
                name: "Linewright:Stalled".into(),
                description: Some("Stuck".into()),
            }],
            &[stalled()],
        );

        assert_eq!(
            changes,
            [label::LabelChange::Describe {
                name: "Linewright:Stalled".into(),
                description: "Handed back in a stall",
            }]
        );
    }

    #[test]
    fn a_linewright_label_with_no_description_is_described() {
        let changes = label::label_changes(&existing(&["linewright:stalled"]), &[stalled()]);

        assert_eq!(
            changes,
            [label::LabelChange::Describe {
                name: "linewright:stalled".into(),
                description: "Handed back in a stall",
            }]
        );
    }

    #[test]
    fn a_linewright_label_already_holding_its_meaning_is_left_as_it_is() {
        let changes = label::label_changes(
            &[label::RepoLabel {
                name: "linewright:stalled".into(),
                description: Some("Handed back in a stall".into()),
            }],
            &[stalled()],
        );

        assert_eq!(changes, []);
    }
}
