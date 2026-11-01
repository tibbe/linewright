//! Which items Linewright manages (`GLOSSARY.md`, **Managed**), and for
//! each its state and what its card says beyond it, or why Linewright
//! leaves it alone: every view of the items asks here, and decides it
//! nowhere else.

use serde::{Deserialize, Serialize};

use crate::board::Column;
use crate::domain::{
    Asks, CheckState, ClosedItem, ClosedKind, Issue, IssueCloseReason, IssueClosing, Login,
    Marking, Merge, MergedPull, Number, Pull, PullState, Snapshot, StatusLabel, SubIssue,
    Timestamp, Tracks, last_labeling, tickets_cut,
};
use crate::lifecycle::{State, done_at, done_by_merge, issue_state, pull_state};
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

/// A managed item: its state and, beside it, what its card and `explain`
/// say beyond the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Managed {
    pub number: Number,
    pub title: String,
    /// The issues it closes, if it is a pull request.
    pub closes: Vec<Number>,
    pub state: State,
    pub detail: Option<Detail>,
    /// Its tickets, if it is a tracking issue, which heads its lane and has
    /// no card (`docs/specs/design_system.md`, The board).
    pub tickets: Option<Tickets>,
    pub waiting_for: Vec<Wait>,
}

impl Managed {
    /// Whether it is a pull request.
    pub fn is_pull_request(&self) -> bool {
        self.state.of_pull_request()
    }

    /// Whether it has ended, so that Linewright acts on it no more: done,
    /// merged or closed, but for a spec change not yet designed.
    pub fn has_ended(&self) -> bool {
        matches!(self.detail, Some(Detail::Ended { .. }))
    }

    /// The column of its card (`docs/specs/design_system.md`, The board):
    /// none for a tracking issue. A spec change not yet designed is in Ready,
    /// or in Working while its design run is in flight, as it is shown until
    /// an issue takes its place.
    pub fn column(&self) -> Option<Column> {
        match (&self.tickets, &self.detail) {
            (Some(_), _) => None,
            (None, Some(Detail::WaitingForDesign)) => Some(Column::Ready),
            (None, Some(Detail::SpecChangeBeingDesigned)) => Some(Column::Working),
            (None, _) => self.state.column(),
        }
    }
}

/// What an item's card and `explain` say beyond its state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "detail", rename_all = "kebab-case")]
pub enum Detail {
    /// An issue in review: Linewright's pull request that closes it.
    InReview(InReview),
    /// An issue in review: someone else's pull request that closes it.
    InReviewElsewhere { pull_request: Number, author: Login },
    /// An open pull request, for the issue `pull_request_for`, the first it
    /// closes that the snapshot holds, if it closes any.
    PullRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_request_for: Option<Number>,
        #[serde(flatten)]
        review: InReview,
    },
    /// How it ended, entering Done `at`. A spec change designed and cut into
    /// tickets has ended as [`Ended::Merged`].
    Ended { ended: Ended, at: Timestamp },
    /// A spec change waiting for its design.
    WaitingForDesign,
    /// A spec change its design run is in flight on.
    SpecChangeBeingDesigned,
}

impl Detail {
    /// Whether it is a spec change's, which is shown as its pull request
    /// until an issue takes its place.
    pub(crate) fn is_spec_change(&self) -> bool {
        match self {
            Detail::WaitingForDesign
            | Detail::SpecChangeBeingDesigned
            | Detail::Ended {
                ended: Ended::NothingToBuild,
                ..
            } => true,
            Detail::InReview(_)
            | Detail::InReviewElsewhere { .. }
            | Detail::PullRequest { .. }
            | Detail::Ended { .. } => false,
        }
    }
}

/// A tracking issue's tickets, closed of total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tickets {
    pub tickets_closed: usize,
    pub tickets: usize,
}

impl Tickets {
    /// Those of `sub_issues` cut from the issue, closed of total.
    pub(crate) fn of(sub_issues: &[SubIssue], linewright: &Login) -> Tickets {
        let tickets: Vec<&SubIssue> = tickets_cut(sub_issues, linewright).collect();
        Tickets {
            tickets_closed: tickets.iter().filter(|ticket| !ticket.is_open()).count(),
            tickets: tickets.len(),
        }
    }
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

    /// Whether it is a draft, and its CI, such as "draft, CI green".
    pub(crate) fn describe(&self) -> String {
        let stage = if self.draft {
            "draft"
        } else {
            "ready for review"
        };
        format!("{stage}, {}", self.ci.describe())
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

/// How the view holds an issue Linewright leaves alone. Its tag is not
/// `state`, which names the item's state beside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "held", rename_all = "kebab-case")]
pub enum LeftAloneIssue {
    Closed,
    Open { left_alone: IssueExclusion },
}

/// How the view holds a pull request Linewright leaves alone, as
/// [`LeftAloneIssue`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "held", rename_all = "kebab-case")]
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
    NotMarked { label: String },
    /// It holds `label`, which asks for a person's work, and Linewright has
    /// not worked it.
    WaitsOnAPerson { label: StatusLabel },
    /// It holds no status label, and has no state otherwise
    /// (`docs/specs/repository.md`, State).
    NoPipelineLabel,
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

/// An item Linewright leaves alone: its state, where it has one, and why
/// it leaves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unmanaged {
    pub state: Option<State>,
    pub why: LeftAloneItem,
}

/// Whether Linewright manages `held`, and if so its state; otherwise why it
/// leaves it alone.
pub fn classify(view: &View, held: Held<'_>, now: Timestamp) -> Result<Managed, Unmanaged> {
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
/// Linewright manages an item it has worked or that asks for its work,
/// while the item has a state (`docs/specs/repository.md`, Managed items).
fn judge(
    view: &View,
    held: Held<'_>,
    waiting_for: Vec<Wait>,
    now: Timestamp,
) -> Result<Managed, Unmanaged> {
    let snapshot = &view.snapshot;
    let policy = &view.policy;
    let linewright = &policy.linewright;
    let managed = |title: &str, closes: &[Number], state, detail, waits| Managed {
        number: held.number(),
        title: title.to_string(),
        closes: closes.to_vec(),
        state,
        detail,
        tickets: None,
        waiting_for: waits,
    };
    let unmanaged = |state, why| Unmanaged { state, why };
    match held {
        // Read closed without its closing.
        Held::Issue(issue) if !issue.open => Err(unmanaged(
            Some(State::Done),
            LeftAloneItem::Issue(LeftAloneIssue::Closed),
        )),
        Held::Issue(issue) => {
            let (state, detail) = open_issue_state(issue, view);
            match managed_state(issue, state, view) {
                Ok(state) => Ok(Managed {
                    tickets: issue
                        .tracks(linewright)
                        .map(|_| Tickets::of(&issue.sub_issues, linewright)),
                    ..managed(&issue.title, &[], state, detail, waiting_for)
                }),
                Err(left_alone) => Err(unmanaged(
                    state,
                    LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone }),
                )),
            }
        }
        Held::Pull(pull) if !pull.is_open() => {
            let state = pull_state(&pull.state);
            match pull.state {
                PullState::Closed { at } if pull.worked_by(linewright) => Ok(managed(
                    &pull.title,
                    &pull.closes,
                    state,
                    Some(Detail::Ended {
                        ended: Ended::ClosedUnmerged,
                        at,
                    }),
                    waiting_for,
                )),
                PullState::Open { .. } | PullState::Closed { .. } | PullState::Merged { .. } => {
                    Err(unmanaged(
                        Some(state),
                        LeftAloneItem::PullRequest(LeftAlonePull::Closed),
                    ))
                }
            }
        }
        Held::Pull(pull) => {
            if pull.held(linewright).is_none() {
                return Err(unmanaged(
                    Some(State::Open),
                    LeftAloneItem::PullRequest(LeftAlonePull::Open {
                        left_alone: PullExclusion::NotWorked {
                            author: pull.author.clone(),
                        },
                    }),
                ));
            }
            let detail = Detail::PullRequest {
                pull_request_for: ticket_of(snapshot, pull),
                review: InReview::of(pull),
            };
            Ok(managed(
                &pull.title,
                &pull.closes,
                State::Open,
                Some(detail),
                waiting_for,
            ))
        }
        Held::Merged(pr) => match merged_detail(pr, snapshot, now, policy) {
            Ok((Detail::WaitingForDesign, waits)) if view.run_stage(pr.number).is_some() => {
                Ok(managed(
                    &pr.title,
                    &pr.closes,
                    State::Merged,
                    Some(Detail::SpecChangeBeingDesigned),
                    waits.into_iter().chain(waiting_for).collect(),
                ))
            }
            Ok((detail, mut waits)) => {
                waits.extend(waiting_for);
                Ok(managed(
                    &pr.title,
                    &pr.closes,
                    State::Merged,
                    Some(detail),
                    waits,
                ))
            }
            Err(left_alone) => Err(unmanaged(
                Some(State::Merged),
                LeftAloneItem::PullRequest(LeftAlonePull::Merged { left_alone }),
            )),
        },
        Held::Closed(item) => match &item.kind {
            ClosedKind::Pull { closes, .. } => Ok(managed(
                &item.title,
                closes,
                State::Closed,
                Some(Detail::Ended {
                    ended: Ended::ClosedUnmerged,
                    at: item.closed_at,
                }),
                waiting_for,
            )),
            ClosedKind::Issue { facts, .. } => {
                let closing = item.issue_closing().filter(|_| item.worked_by(linewright));
                let Some(closing) = closing else {
                    return Err(unmanaged(
                        Some(State::Done),
                        LeftAloneItem::Issue(LeftAloneIssue::Closed),
                    ));
                };
                let merge = done_by_merge(
                    &facts.work.events,
                    &facts.work.closed_by,
                    &snapshot.default_branch,
                );
                let (ended, at) = issue_ended(merge, closing);
                Ok(Managed {
                    tickets: item
                        .tracks(linewright)
                        .map(|_| Tickets::of(&facts.sub_issues, linewright)),
                    ..managed(
                        &item.title,
                        &[],
                        State::Done,
                        Some(Detail::Ended { ended, at }),
                        waiting_for,
                    )
                })
            }
        },
    }
}

/// Open `issue`'s state, `state`, if Linewright manages it; otherwise why it
/// leaves the issue alone (`docs/specs/repository.md`, Managed items): it
/// has not worked the issue and the issue does not ask for its work, or the
/// issue has no state.
fn managed_state(
    issue: &Issue,
    state: Option<State>,
    view: &View,
) -> Result<State, IssueExclusion> {
    let policy = &view.policy;
    let marking = policy.marking(&issue.labels);
    let closing = view.snapshot.pulls_closing(issue);
    if issue.candidate(&policy.linewright, &closing, marking) {
        return state.ok_or(IssueExclusion::NoPipelineLabel);
    }
    Err(match (issue.status_label(), &policy.require_label) {
        (Some(status), _) if status.asks() == Asks::Person => {
            IssueExclusion::WaitsOnAPerson { label: status }
        }
        (Some(_), Some(label)) if marking == Marking::Unmarked => IssueExclusion::NotMarked {
            label: label.clone(),
        },
        _ => IssueExclusion::NoPipelineLabel,
    })
}

/// Open `issue`'s state, with the run in flight on it, and what its card
/// says beyond it: the pull request an issue in review is in review in,
/// and the merge that made an issue done.
fn open_issue_state(issue: &Issue, view: &View) -> (Option<State>, Option<Detail>) {
    let snapshot = &view.snapshot;
    let linewright = &view.policy.linewright;
    let closing = snapshot.pulls_closing(issue);
    let merge = done_by_merge(&issue.events, &closing, &snapshot.default_branch);
    let state = issue_state(
        issue,
        &closing,
        merge,
        view.run_stage(issue.number),
        linewright,
    );
    let detail =
        match state {
            Some(State::Done) => merge.map(|merge| Detail::Ended {
                ended: Ended::ClosedIn(merge.pull_request),
                at: merge.at,
            }),
            Some(State::InReview) => match open_pull_for(snapshot, issue.number, linewright) {
                Some(pull) => Some(Detail::InReview(InReview::of(pull))),
                None => closing.iter().find(|pull| pull.is_open()).map(|pull| {
                    Detail::InReviewElsewhere {
                        pull_request: pull.number,
                        author: pull.author.clone(),
                    }
                }),
            },
            Some(_) | None => None,
        };
    (state, detail)
}

/// What merged `pr` shows beyond its state, with the waits that alone
/// implies, which the decisions' waits for it follow; or why it is no spec
/// change. A spec change Linewright was unassigned from has been worked
/// (`GLOSSARY.md`, **Managed**), so it stays managed, though Linewright does
/// not cut it. Any other that Linewright worked has ended: a spec change
/// designed with nothing to build as such, the rest as merged
/// (`docs/specs/design_system.md`, Cards).
fn merged_detail(
    pr: &MergedPull,
    snapshot: &Snapshot,
    now: Timestamp,
    policy: &Policy,
) -> Result<(Detail, Vec<Wait>), NotSpecChange> {
    let linewright = &policy.linewright;
    let ended = |ended, at| Ok((Detail::Ended { ended, at }, vec![]));
    match spec_change(pr, &snapshot.default_branch, now, policy) {
        Ok(SpecChange::Pending) => Ok((Detail::WaitingForDesign, vec![])),
        Ok(SpecChange::Unassigned { required_label }) => Ok((
            Detail::WaitingForDesign,
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

/// How a closed issue Linewright worked ended (`docs/specs/design_system.md`,
/// The board, Cards) and when it entered Done, from `merge`, the merge that
/// made it done ([`done_by_merge`]), and `closing`: "closed in" names that
/// pull request before the closer.
fn issue_ended(merge: Option<Merge>, closing: IssueClosing) -> (Ended, Timestamp) {
    match merge {
        Some(merge) => (
            Ended::ClosedIn(merge.pull_request),
            done_at(Some(merge), Some(closing.at)).unwrap_or(closing.at),
        ),
        None => (
            closing
                .merged_by
                .map_or(Ended::Closed(closing.reason), Ended::ClosedIn),
            closing.at,
        ),
    }
}
