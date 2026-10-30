//! Which items Linewright manages (`GLOSSARY.md`, **Managed**), and where
//! each stands or why Linewright leaves it alone: every view of the items
//! asks here, and decides it nowhere else.

use serde::{Deserialize, Serialize};

use crate::domain::{
    CheckState, ClosedItem, ClosedKind, Issue, IssueCloseReason, Login, Marking, Merge, MergedPull,
    Number, Pull, PullState, Snapshot, StatusLabel, Timestamp, Tracks, label, last_labeling,
};
use crate::phrase::Phrase;
use crate::reconcile::{
    NotSpecChange, NotWorked, Policy, SpecChange, Wait, open_pull_for, spec_change, worked_ticket,
};
use crate::status::View;

/// An item the view holds, as the classifier judges it.
#[derive(Clone, Copy, Debug)]
pub enum Held<'a> {
    /// An issue as the view holds an open one. One read closed without its
    /// closing, as `explain` may read it, is judged closed.
    Issue(&'a Issue),
    /// An issue or a pull request closed, with its closing.
    Closed(&'a ClosedItem),
    /// A pull request, open or closed without merging.
    Pull(&'a Pull),
    Merged(&'a MergedPull),
}

impl Held<'_> {
    fn number(self) -> Number {
        match self {
            Held::Issue(issue) => issue.number,
            Held::Closed(item) => item.number,
            Held::Pull(pull) => pull.number,
            Held::Merged(pull) => pull.number,
        }
    }
}

/// A managed item and where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Managed {
    pub number: Number,
    pub title: String,
    /// The issues it closes, if it is a pull request.
    pub closes: Vec<Number>,
    /// Whether it is closed, rather than open or merged.
    pub closed: bool,
    pub state: ItemState,
    pub waiting_for: Vec<Wait>,
}

impl Managed {
    /// Whether it is a pull request: one in review, a spec change, or one
    /// that ended as a pull request.
    pub fn is_pull_request(&self) -> bool {
        use ItemState as S;
        match &self.state {
            S::WaitingForDesign | S::PullRequest { .. } => true,
            S::Ended { ended, .. } => ended.of_pull_request(),
            S::Tracking { .. }
            | S::WaitingForTriage
            | S::BeingTriaged
            | S::WaitingOnThePeopleAsked
            | S::WithTheMaintainer
            | S::InReview(_)
            | S::InReviewElsewhere { .. }
            | S::BeingDesigned
            | S::ReadyToDesign
            | S::Claimed
            | S::ReadyToClaim
            | S::MergedInto { .. } => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ItemState {
    WaitingForDesign,
    Tracking {
        tickets_closed: usize,
        tickets: usize,
    },
    WaitingForTriage,
    BeingTriaged,
    WaitingOnThePeopleAsked,
    WithTheMaintainer,
    InReview(InReview),
    InReviewElsewhere {
        pull_request: Number,
        author: Login,
    },
    /// Claimed, and not designed yet.
    BeingDesigned,
    /// Not claimed, and not designed yet.
    ReadyToDesign,
    Claimed,
    ReadyToClaim,
    /// A pull request of Linewright's in review, for the issue
    /// `pull_request_for`.
    PullRequest {
        pull_request_for: Number,
        #[serde(flatten)]
        review: InReview,
    },
    /// Ended, entering Done `at`. A spec change designed and cut into
    /// tickets has ended as [`Ended::Merged`].
    Ended {
        ended: Ended,
        at: Timestamp,
    },
    /// An open issue `pull_request`, which closes it, merged into, entering
    /// Done `at`: `status` and `explain` show it as `labeled`, the state its
    /// labels give.
    MergedInto {
        pull_request: Number,
        at: Timestamp,
        labeled: Box<ItemState>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InReview {
    pub pull_request: Number,
    pub draft: bool,
    pub ci: Ci,
}

impl InReview {
    pub(crate) fn of(pr: &Pull) -> InReview {
        let has = |state| pr.checks.iter().any(|c| c.state == state);
        InReview {
            pull_request: pr.number,
            draft: matches!(pr.state, PullState::Open { draft: true }),
            ci: if pr.checks.is_empty() {
                Ci::NoChecks
            } else if has(CheckState::Failed) {
                Ci::Failing
            } else if has(CheckState::Pending) {
                Ci::Running
            } else {
                Ci::Green
            },
        }
    }

    pub(crate) fn describe(&self) -> Phrase {
        let stage = if self.draft {
            "draft"
        } else {
            "ready for review"
        };
        Phrase::from("in review: ")
            .item(self.pull_request)
            .text(format!(" ({stage}, {})", self.ci.describe()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ci {
    NoChecks,
    Failing,
    Running,
    Green,
}

impl Ci {
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Ci::NoChecks => "no checks",
            Ci::Failing => "CI failing",
            Ci::Running => "CI running",
            Ci::Green => "CI green",
        }
    }
}

/// How an item in Done ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ended {
    /// An issue the pull request closes, the first to merge since it was
    /// last opened.
    ClosedIn(Number),
    /// An issue closed otherwise, with GitHub's reason where it is known.
    Closed(Option<IssueCloseReason>),
    /// A pull request of Linewright's, merged.
    Merged,
    /// A pull request of Linewright's, closed without merging.
    ClosedUnmerged,
    /// A spec change designed with nothing to build.
    NothingToBuild,
}

impl Ended {
    /// Whether it is how a pull request ended, rather than an issue.
    fn of_pull_request(self) -> bool {
        match self {
            Ended::ClosedIn(_) | Ended::Closed(_) => false,
            Ended::Merged | Ended::ClosedUnmerged | Ended::NothingToBuild => true,
        }
    }

    /// How it ended, as a card in Done says (`docs/specs/design_system.md`,
    /// Cards).
    pub(crate) fn worded(self) -> Phrase {
        match self {
            Ended::ClosedIn(pull) => Phrase::from("closed in ").item(pull),
            Ended::Closed(None) => "closed".into(),
            Ended::Closed(Some(IssueCloseReason::Completed)) => "closed as completed".into(),
            Ended::Closed(Some(IssueCloseReason::NotPlanned)) => "closed as not planned".into(),
            Ended::Closed(Some(IssueCloseReason::Duplicate)) => "closed as a duplicate".into(),
            Ended::Merged => "merged".into(),
            Ended::ClosedUnmerged => "closed without merging".into(),
            Ended::NothingToBuild => "merged · nothing to build".into(),
        }
    }
}

/// Why Linewright leaves an item alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "kebab-case")]
pub enum LeftAloneItem {
    Issue(LeftAloneIssue),
    PullRequest(LeftAlonePull),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum LeftAloneIssue {
    Closed,
    Open { left_alone: IssueExclusion },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum LeftAlonePull {
    Closed,
    Open { left_alone: PullExclusion },
    Merged { left_alone: NotSpecChange },
}

/// Why Linewright leaves an open issue alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum IssueExclusion {
    /// It would wait on Linewright were it marked for Linewright, and
    /// Linewright has not worked it.
    NotMarked {
        label: String,
    },
    /// It waits on a person, and Linewright has not worked it.
    WaitsOnAPerson,
    /// Linewright has worked it, and no column holds it.
    NoPipelineLabel,
    NoTickets,
}

impl IssueExclusion {
    /// What keeps the issue out, as a predicate of it, such as "not marked
    /// for Linewright".
    pub(crate) fn worded(&self) -> Phrase {
        match self {
            IssueExclusion::NotMarked { .. } => "not marked for Linewright".into(),
            IssueExclusion::WaitsOnAPerson => {
                "waits on a person, and Linewright has not worked it".into()
            }
            IssueExclusion::NoPipelineLabel => Phrase::from("labeled none of ").then(Phrase::list(
                StatusLabel::ALL.map(|status| Phrase::default().code(status.label())),
                "or",
            )),
            IssueExclusion::NoTickets => {
                "a tracking issue with no tickets, and none are being cut".into()
            }
        }
    }
}

/// Why Linewright leaves an open pull request alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum PullExclusion {
    NotLinewrights { author: Login },
    ClosesNoIssue,
    Superseded { issue: Number, by: Number },
}

/// Whether Linewright manages `held`, and if so where it stands; otherwise
/// why it leaves it alone.
pub fn classify(view: &View, held: Held<'_>, now: Timestamp) -> Result<Managed, LeftAloneItem> {
    let waiting_for = view
        .decisions(now)
        .waits
        .remove(&held.number())
        .unwrap_or_default();
    judge(view, held, waiting_for, now)
}

/// Every managed item the view holds, open, closed and merged, each once:
/// the merged pull requests first, then the open issues, the open pull
/// requests and the closed items.
pub fn managed(view: &View, now: Timestamp) -> Vec<Managed> {
    let snapshot = &view.snapshot;
    let mut waits = view.decisions(now).waits;
    let held = snapshot
        .merged
        .iter()
        .map(Held::Merged)
        .chain(snapshot.issues.iter().filter(|i| i.open).map(Held::Issue))
        .chain(
            snapshot
                .pulls
                .iter()
                .filter(|p| p.is_open())
                .map(Held::Pull),
        )
        .chain(snapshot.closed.iter().map(Held::Closed));
    held.filter_map(|held| {
        let waiting_for = waits.remove(&held.number()).unwrap_or_default();
        judge(view, held, waiting_for, now).ok()
    })
    .collect()
}

/// [`classify`], with `waiting_for` what the decisions wait for on `held`.
fn judge(
    view: &View,
    held: Held<'_>,
    waiting_for: Vec<Wait>,
    now: Timestamp,
) -> Result<Managed, LeftAloneItem> {
    let snapshot = &view.snapshot;
    let policy = &view.policy;
    let managed = |title: &str, closes: &[Number], state, waits| Managed {
        number: held.number(),
        title: title.to_string(),
        closes: closes.to_vec(),
        closed: matches!(held, Held::Closed(_)),
        state,
        waiting_for: waits,
    };
    match held {
        Held::Issue(issue) if !issue.open => Err(LeftAloneItem::Issue(LeftAloneIssue::Closed)),
        Held::Issue(issue) => match standing(issue, snapshot, policy, &waiting_for) {
            Ok(state) => Ok(managed(&issue.title, &[], state, waiting_for)),
            Err(left_alone) => Err(LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone })),
        },
        Held::Pull(pull) if !pull.is_open() => {
            Err(LeftAloneItem::PullRequest(LeftAlonePull::Closed))
        }
        Held::Pull(pull) => match pull_standing(pull, snapshot, &policy.linewright) {
            Ok(state) => Ok(managed(&pull.title, &pull.closes, state, waiting_for)),
            Err(left_alone) => Err(LeftAloneItem::PullRequest(LeftAlonePull::Open {
                left_alone,
            })),
        },
        Held::Merged(pr) => match merged_standing(pr, snapshot, now, policy) {
            Ok((state, mut waits)) => {
                waits.extend(waiting_for);
                Ok(managed(&pr.title, &pr.closes, state, waits))
            }
            Err(left_alone) => Err(LeftAloneItem::PullRequest(LeftAlonePull::Merged {
                left_alone,
            })),
        },
        Held::Closed(item) => {
            let closes = match &item.kind {
                ClosedKind::Pull { closes } => Some(&closes[..]),
                ClosedKind::Issue { .. } => None,
            };
            match (
                closed_standing(item, snapshot, &policy.linewright, &waiting_for),
                closes,
            ) {
                (Some(state), closes) => Ok(managed(
                    &item.title,
                    closes.unwrap_or_default(),
                    state,
                    waiting_for,
                )),
                (None, Some(_)) => Err(LeftAloneItem::PullRequest(LeftAlonePull::Closed)),
                (None, None) => Err(LeftAloneItem::Issue(LeftAloneIssue::Closed)),
            }
        }
    }
}

/// Where open `issue` stands, or why Linewright leaves it alone: it is
/// managed (`GLOSSARY.md`, **Managed**) or left alone by
/// [`Issue::managed`].
fn standing(
    issue: &Issue,
    snapshot: &Snapshot,
    policy: &Policy,
    waiting_for: &[Wait],
) -> Result<ItemState, IssueExclusion> {
    let linewright = &policy.linewright;
    let marking = policy.marking(&issue.labels);
    let closing = snapshot.pulls_closing(issue);
    if !issue.managed(linewright, &closing, marking) {
        return Err(match &policy.require_label {
            Some(label)
                if marking == Marking::Unmarked
                    && issue.waits_on_linewright(linewright, Marking::Marked) =>
            {
                IssueExclusion::NotMarked {
                    label: label.clone(),
                }
            }
            _ => IssueExclusion::WaitsOnAPerson,
        });
    }
    // An issue Linewright worked that a pull request closing it merged
    // into is in Done, whatever else holds of it.
    let merge = issue
        .merged_since_opened(&closing, &snapshot.default_branch)
        .filter(|_| issue.tracks(linewright).is_none() && issue.worked_by(linewright, &closing));
    let state = match (issue_state(issue, snapshot, policy), merge) {
        (Ok(labeled), Some(merge)) => {
            return Ok(ItemState::MergedInto {
                pull_request: merge.pull_request,
                at: merge.at,
                labeled: Box::new(labeled),
            });
        }
        (Err(_), Some(merge)) => {
            return Ok(ItemState::Ended {
                ended: Ended::ClosedIn(merge.pull_request),
                at: merge.at,
            });
        }
        (state, None) => state?,
    };
    if matches!(state, ItemState::Tracking { tickets: 0, .. }) && waiting_for.is_empty() {
        return Err(IssueExclusion::NoTickets);
    }
    Ok(state)
}

fn issue_state(
    issue: &Issue,
    snapshot: &Snapshot,
    policy: &Policy,
) -> Result<ItemState, IssueExclusion> {
    let open_pull = open_pull_for(snapshot, issue.number, &policy.linewright);
    let closing = snapshot.pulls_closing(issue);
    let open_closing = closing.iter().find(|p| p.is_open());
    let state = if issue.tracks(&policy.linewright).is_some() {
        ItemState::Tracking {
            tickets_closed: issue.sub_issues.iter().filter(|t| !t.is_open()).count(),
            tickets: issue.sub_issues.len(),
        }
    } else if let Some(pr) = open_pull {
        ItemState::InReview(InReview::of(pr))
    } else if let Some(pr) = open_closing {
        ItemState::InReviewElsewhere {
            pull_request: pr.number,
            author: pr.author.clone(),
        }
    } else {
        match issue.status_label() {
            Some(StatusLabel::NeedsTriage) if issue.is_assigned(&policy.linewright) => {
                ItemState::BeingTriaged
            }
            Some(StatusLabel::NeedsTriage) => ItemState::WaitingForTriage,
            Some(StatusLabel::NeedsInfo) => ItemState::WaitingOnThePeopleAsked,
            Some(StatusLabel::ReadyForHuman) => ItemState::WithTheMaintainer,
            Some(StatusLabel::ReadyForAgent) => {
                match (
                    issue.is_assigned(&policy.linewright),
                    issue.has_label(label::DESIGNED),
                ) {
                    (true, true) => ItemState::Claimed,
                    (true, false) => ItemState::BeingDesigned,
                    (false, true) => ItemState::ReadyToClaim,
                    (false, false) => ItemState::ReadyToDesign,
                }
            }
            None => return Err(IssueExclusion::NoPipelineLabel),
        }
    };
    Ok(state)
}

/// Where open `pull` stands: Linewright works on it for the issue it
/// closes, unless a newer one of its pull requests supersedes it.
fn pull_standing(
    pull: &Pull,
    snapshot: &Snapshot,
    linewright: &Login,
) -> Result<ItemState, PullExclusion> {
    let ticket =
        worked_ticket(snapshot, pull, linewright).map_err(|not_worked| match not_worked {
            NotWorked::NotLinewrights => PullExclusion::NotLinewrights {
                author: pull.author.clone(),
            },
            NotWorked::ForNoIssue => PullExclusion::ClosesNoIssue,
            NotWorked::Superseded { ticket, by } => PullExclusion::Superseded { issue: ticket, by },
        })?;
    Ok(ItemState::PullRequest {
        pull_request_for: ticket,
        review: InReview::of(pull),
    })
}

/// Where merged `pr` stands, with the waits its standing implies alone,
/// which the decisions' waits for it follow; or why it is no spec change.
/// A spec change Linewright was unassigned from has been worked
/// (`GLOSSARY.md`, **Managed**), so it stays managed, though Linewright does
/// not cut it. Linewright's own pull request and a spec change designed have
/// ended.
fn merged_standing(
    pr: &MergedPull,
    snapshot: &Snapshot,
    now: Timestamp,
    policy: &Policy,
) -> Result<(ItemState, Vec<Wait>), NotSpecChange> {
    let linewright = &policy.linewright;
    let ended = |ended, at| Ok((ItemState::Ended { ended, at }, vec![]));
    match spec_change(pr, &snapshot.default_branch, now, policy) {
        Ok(SpecChange::Pending) => Ok((ItemState::WaitingForDesign, vec![])),
        Ok(SpecChange::Unassigned { required_label }) => Ok((
            ItemState::WaitingForDesign,
            vec![Wait::Unassigned { required_label }],
        )),
        Err(NotSpecChange::Linewrights) => ended(Ended::Merged, pr.merged_at),
        Err(NotSpecChange::Designed { .. }) if is_cut(snapshot, pr.number, linewright) => {
            ended(Ended::Merged, pr.merged_at)
        }
        Err(NotSpecChange::Designed { label }) => {
            let designed = last_labeling(&pr.events, label.name())
                .map_or(pr.merged_at, |labeling| labeling.at);
            ended(Ended::NothingToBuild, designed)
        }
        Err(because) => Err(because),
    }
}

/// Whether spec change #`spec_change` was cut into tickets: a tracking issue
/// the snapshot holds, open or closed, tracks it.
pub(crate) fn is_cut(snapshot: &Snapshot, spec_change: Number, linewright: &Login) -> bool {
    let tracks = Some(Tracks::SpecChange(spec_change));
    let open = snapshot
        .issues
        .iter()
        .filter(|issue| issue.open)
        .any(|issue| issue.tracks(linewright) == tracks);
    open || snapshot
        .closed
        .iter()
        .any(|item| item.tracks(linewright) == tracks)
}

/// Where closed `item` stands, if Linewright worked it: a tracking issue
/// tracks its tickets still, and the rest have ended.
fn closed_standing(
    item: &ClosedItem,
    snapshot: &Snapshot,
    linewright: &Login,
    waiting_for: &[Wait],
) -> Option<ItemState> {
    if !item.worked_by(linewright) {
        return None;
    }
    match &item.kind {
        ClosedKind::Issue { facts, .. } if item.tracks(linewright).is_some() => {
            let tickets = facts.sub_issues.len();
            (tickets > 0 || !waiting_for.is_empty()).then(|| ItemState::Tracking {
                tickets_closed: facts.sub_issues.iter().filter(|t| !t.is_open()).count(),
                tickets,
            })
        }
        ClosedKind::Issue {
            facts,
            merged_by,
            reason,
            ..
        } => {
            let closing = Closing {
                at: item.closed_at,
                merged_by: *merged_by,
                reason: *reason,
            };
            let merge = facts.work.merged_since_opened(&snapshot.default_branch);
            let (ended, at) = issue_ended(merge, closing);
            Some(ItemState::Ended { ended, at })
        }
        ClosedKind::Pull { .. } => Some(ItemState::Ended {
            ended: Ended::ClosedUnmerged,
            at: item.closed_at,
        }),
    }
}

/// An issue's closing: when, the pull request whose merge closed it where it
/// is known, and GitHub's reason.
#[derive(Clone, Copy)]
struct Closing {
    at: Timestamp,
    merged_by: Option<Number>,
    reason: Option<IssueCloseReason>,
}

/// How a closed issue Linewright worked ended (`docs/specs/design_system.md`,
/// The board, Cards) and when it entered Done, from `merge`, the first pull
/// request that closes it to merge into the default branch since it was last
/// opened, and `closing`: "closed in" names that pull request before the
/// closer.
fn issue_ended(merge: Option<Merge>, closing: Closing) -> (Ended, Timestamp) {
    match merge {
        Some(merge) => (
            Ended::ClosedIn(merge.pull_request),
            merge.at.min(closing.at),
        ),
        None => (
            closing
                .merged_by
                .map_or(Ended::Closed(closing.reason), Ended::ClosedIn),
            closing.at,
        ),
    }
}
