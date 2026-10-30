//! Which items Linewright manages (`GLOSSARY.md`, **Managed**), and where
//! each stands or why Linewright leaves it alone: every view of the items
//! asks here, and decides it nowhere else.

use serde::{Deserialize, Serialize};

use crate::domain::{
    Asks, CheckState, ClosedItem, ClosedKind, ClosingPull, Issue, IssueCloseReason, IssueClosing,
    Login, Marking, Merge, MergedPull, Number, Pull, PullState, Snapshot, StatusLabel, SubIssue,
    Timestamp, Tracks, label, last_labeling,
};
use crate::phrase::Phrase;
use crate::reconcile::{
    NotSpecChange, Policy, SpecChange, Wait, open_pull_for, spec_change, ticket_of,
};
use crate::status::View;

/// An item the view holds, as the classifier judges it.
#[derive(Clone, Copy, Debug)]
pub enum Held<'a> {
    /// An issue as the view holds an open one. One read closed without its
    /// closing, which a store reads only where GitHub gives no time it
    /// closed, is left alone as closed.
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
            | S::Unlabeled
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
    /// An open issue with no status label, managed through its run in
    /// flight.
    Unlabeled,
    /// A managed pull request in review, for the issue `pull_request_for`,
    /// the first it closes that the snapshot holds, if it closes any.
    PullRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_request_for: Option<Number>,
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
    /// A pull request merged.
    Merged,
    /// A pull request closed without merging.
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
    /// It holds `label`, which asks for a person's work, and Linewright has
    /// not worked it.
    WaitsOnAPerson {
        label: StatusLabel,
    },
    /// It holds no status label, and nothing else makes it managed.
    NoPipelineLabel,
    NoTickets,
    /// A tracking issue with no status label and none of its tickets open.
    NoOpenTickets,
}

impl IssueExclusion {
    /// What keeps the issue out, as a predicate of it, such as "not marked
    /// for Linewright".
    pub(crate) fn worded(&self) -> Phrase {
        match self {
            IssueExclusion::NotMarked { .. } => "not marked for Linewright".into(),
            IssueExclusion::WaitsOnAPerson { label } => Phrase::from("holds ")
                .code(label.label())
                .text(", and Linewright has not worked it"),
            IssueExclusion::NoPipelineLabel => Phrase::from("labeled none of ").then(Phrase::list(
                StatusLabel::ALL.map(|status| Phrase::default().code(status.label())),
                "or",
            )),
            IssueExclusion::NoTickets => {
                "a tracking issue with no tickets, and none are being cut".into()
            }
            IssueExclusion::NoOpenTickets => "a tracking issue with no open ticket".into(),
        }
    }
}

/// Why Linewright leaves an open pull request alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum PullExclusion {
    /// A person's, that Linewright has not worked and is not assigned to.
    NotWorked { author: Login },
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

/// What the view holds as #`number`: a closed item, held with its closing,
/// before the same item held as an open one would be.
pub fn held(snapshot: &Snapshot, number: Number) -> Option<Held<'_>> {
    snapshot
        .closed_item(number)
        .map(Held::Closed)
        .or_else(|| snapshot.issue(number).map(Held::Issue))
        .or_else(|| snapshot.any_pull(number).map(Held::Pull))
        .or_else(|| snapshot.merged_pull(number).map(Held::Merged))
}

/// Every managed item the view holds, open, closed and merged, each once:
/// the merged pull requests first, then the open issues, the open pull
/// requests, Linewright's then the people's, and the closed items.
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
                .chain(&snapshot.peoples_pulls)
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
        closed: match held {
            Held::Issue(issue) => !issue.open,
            Held::Closed(_) => true,
            Held::Pull(pull) => !pull.is_open(),
            Held::Merged(_) => false,
        },
        state,
        waiting_for: waits,
    };
    match held {
        Held::Issue(issue) if !issue.open => Err(LeftAloneItem::Issue(LeftAloneIssue::Closed)),
        Held::Issue(issue) => match standing(issue, view, &waiting_for) {
            Ok(state) => Ok(managed(&issue.title, &[], state, waiting_for)),
            Err(left_alone) => Err(LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone })),
        },
        Held::Pull(pull) if !pull.is_open() => match pull.state {
            PullState::Closed { at } if pull.worked_by(&policy.linewright) => Ok(managed(
                &pull.title,
                &pull.closes,
                ItemState::Ended {
                    ended: Ended::ClosedUnmerged,
                    at,
                },
                waiting_for,
            )),
            PullState::Open { .. } | PullState::Closed { .. } | PullState::Merged { .. } => {
                Err(LeftAloneItem::PullRequest(LeftAlonePull::Closed))
            }
        },
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
                ClosedKind::Pull { closes, .. } => Some(&closes[..]),
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

/// Where open `issue` stands, or why Linewright leaves it alone
/// (`docs/specs/repository.md`, Managed items): of the issues Linewright
/// worked or that ask for its work ([`Issue::candidate`]), it leaves alone
/// one with no status label, no run in flight, no open pull request closing
/// it and no open ticket it tracks, whatever it did on it before.
fn standing(issue: &Issue, view: &View, waiting_for: &[Wait]) -> Result<ItemState, IssueExclusion> {
    let snapshot = &view.snapshot;
    let policy = &view.policy;
    let linewright = &policy.linewright;
    let marking = policy.marking(&issue.labels);
    let closing = snapshot.pulls_closing(issue);
    if !issue.candidate(linewright, &closing, marking) {
        return Err(match (issue.status_label(), &policy.require_label) {
            (Some(status), _) if status.asks() == Asks::Person => {
                IssueExclusion::WaitsOnAPerson { label: status }
            }
            (Some(_), Some(label)) if marking == Marking::Unmarked => IssueExclusion::NotMarked {
                label: label.clone(),
            },
            _ => IssueExclusion::NoPipelineLabel,
        });
    }
    let tracks = issue.tracks(linewright);
    let tickets: Vec<&SubIssue> = tracks
        .map(|tracks| tracks.tickets(&issue.sub_issues, linewright).collect())
        .unwrap_or_default();
    if issue.status_label().is_none()
        && !view.has_live_run(issue.number)
        && !closing.iter().any(ClosingPull::is_open)
        && !tickets.iter().any(|ticket| ticket.is_open())
    {
        return Err(match tracks {
            Some(_) if tickets.is_empty() => IssueExclusion::NoTickets,
            Some(_) => IssueExclusion::NoOpenTickets,
            None => IssueExclusion::NoPipelineLabel,
        });
    }
    // An issue that a pull request closing it merged into is in Done,
    // whatever else holds of it.
    let merge = issue
        .merged_since_opened(&closing, &snapshot.default_branch)
        .filter(|_| tracks.is_none());
    let state = issue_state(issue, snapshot, policy);
    if let Some(merge) = merge {
        // One with no status label has no state its labels give.
        return Ok(if state == ItemState::Unlabeled {
            ItemState::Ended {
                ended: Ended::ClosedIn(merge.pull_request),
                at: merge.at,
            }
        } else {
            ItemState::MergedInto {
                pull_request: merge.pull_request,
                at: merge.at,
                labeled: Box::new(state),
            }
        });
    }
    if matches!(state, ItemState::Tracking { tickets: 0, .. }) && waiting_for.is_empty() {
        return Err(IssueExclusion::NoTickets);
    }
    Ok(state)
}

/// Where open `issue` stands by its tracking, the pull requests closing it
/// and its status label.
fn issue_state(issue: &Issue, snapshot: &Snapshot, policy: &Policy) -> ItemState {
    let open_pull = open_pull_for(snapshot, issue.number, &policy.linewright);
    let closing = snapshot.pulls_closing(issue);
    let open_closing = closing.iter().find(|p| p.is_open());
    if issue.tracks(&policy.linewright).is_some() {
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
            None => ItemState::Unlabeled,
        }
    }
}

/// Where open `pull` stands: in review, if Linewright opened it, worked it
/// or is assigned to it, for the issue it is for, if any.
fn pull_standing(
    pull: &Pull,
    snapshot: &Snapshot,
    linewright: &Login,
) -> Result<ItemState, PullExclusion> {
    if pull.held(linewright).is_none() {
        return Err(PullExclusion::NotWorked {
            author: pull.author.clone(),
        });
    }
    Ok(ItemState::PullRequest {
        pull_request_for: ticket_of(snapshot, pull),
        review: InReview::of(pull),
    })
}

/// Where merged `pr` stands, with the waits its standing implies alone,
/// which the decisions' waits for it follow; or why it is no spec change.
/// A spec change Linewright was unassigned from has been worked
/// (`GLOSSARY.md`, **Managed**), so it stays managed, though Linewright does
/// not cut it. Any other that Linewright worked has ended: a spec change
/// designed with nothing to build as such, the rest as merged
/// (`docs/specs/design_system.md`, Cards).
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
        Err(because) if !pr.worked_by(linewright) => Err(because),
        Err(NotSpecChange::Designed { .. }) if is_cut(snapshot, pr.number, linewright) => {
            ended(Ended::Merged, pr.merged_at)
        }
        Err(NotSpecChange::Designed { label }) => {
            let designed = last_labeling(&pr.events, label.name())
                .map_or(pr.merged_at, |labeling| labeling.at);
            ended(Ended::NothingToBuild, designed)
        }
        Err(_) => ended(Ended::Merged, pr.merged_at),
    }
}

/// Whether spec change #`spec_change` was cut into tickets: a tracking issue
/// the snapshot holds, open or closed, tracks it.
pub(crate) fn is_cut(snapshot: &Snapshot, spec_change: Number, linewright: &Login) -> bool {
    tracking_issues(snapshot, spec_change, linewright)
        .next()
        .is_some()
}

/// The tracking issues the snapshot holds, open or closed, that track spec
/// change #`spec_change`.
pub(crate) fn tracking_issues<'a>(
    snapshot: &'a Snapshot,
    spec_change: Number,
    linewright: &'a Login,
) -> impl Iterator<Item = Number> + 'a {
    let tracks = Some(Tracks::SpecChange(spec_change));
    let open = snapshot
        .issues
        .iter()
        .filter(move |issue| issue.open && issue.tracks(linewright) == tracks)
        .map(|issue| issue.number);
    let closed = snapshot
        .closed
        .iter()
        .filter(move |item| item.tracks(linewright) == tracks)
        .map(|item| item.number);
    open.chain(closed)
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
        ClosedKind::Issue { facts, .. } => {
            let merge = facts.work.merged_since_opened(&snapshot.default_branch);
            let (ended, at) = issue_ended(merge, item.issue_closing()?);
            Some(ItemState::Ended { ended, at })
        }
        ClosedKind::Pull { .. } => Some(ItemState::Ended {
            ended: Ended::ClosedUnmerged,
            at: item.closed_at,
        }),
    }
}

/// How a closed issue Linewright worked ended (`docs/specs/design_system.md`,
/// The board, Cards) and when it entered Done, from `merge`, the first pull
/// request that closes it to merge into the default branch since it was last
/// opened, and `closing`: "closed in" names that pull request before the
/// closer.
fn issue_ended(merge: Option<Merge>, closing: IssueClosing) -> (Ended, Timestamp) {
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
