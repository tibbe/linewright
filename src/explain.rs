use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::{
    self, ClosedItem, ClosedKind, ClosingPull, Issue, Item, Login, ManagedFacts, MergedPull,
    Number, Snapshot, SubIssue, Timestamp, Tracks,
};
use crate::managed::{
    self, Held, InReview, IssueExclusion, ItemState, LeftAloneIssue, LeftAloneItem, LeftAlonePull,
    Managed, PullExclusion, classify,
};
use crate::phrase::Phrase;
use crate::reconcile::{NotSpecChange, Wait};
use crate::runs::{self, Recorded, Run, RunInFlight, State};
use crate::status::{Paint, View, describe, describe_wait, shown};
use crate::store::AnyItem;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Answer {
    // First, so that only an answer naming why reads as one.
    LeftAlone(LeftAlone),
    Managed(Explanation),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub family: Family,
    #[serde(flatten)]
    pub standing: Standing,
    /// Whether it was handed back in a stall (`GLOSSARY.md`, **Stall**).
    #[serde(default)]
    pub stalled: bool,
    pub last_read: Timestamp,
    pub waiting_for: Vec<Wait>,
    #[serde(flatten)]
    pub runs: ItemRuns,
}

/// Its fields are private so that only `ItemRuns::of`, which cuts the past
/// runs to the limit, and a read of its JSON that checks the cut make one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "ItemRunsAnswer", try_from = "ItemRunsAnswer")]
pub struct ItemRuns {
    runs_of: Number,
    run_in_flight: Option<RunInFlight>,
    past_runs: Vec<Run>,
    past_runs_left_out: usize,
}

#[derive(Serialize, Deserialize)]
struct ItemRunsAnswer {
    runs_of: Number,
    run_in_flight: Option<RunInFlight>,
    /// Newest first.
    past_runs: Vec<Run>,
    past_runs_left_out: usize,
}

impl From<ItemRuns> for ItemRunsAnswer {
    fn from(runs: ItemRuns) -> ItemRunsAnswer {
        ItemRunsAnswer {
            runs_of: runs.runs_of,
            run_in_flight: runs.run_in_flight,
            past_runs: runs.past_runs,
            past_runs_left_out: runs.past_runs_left_out,
        }
    }
}

impl TryFrom<ItemRunsAnswer> for ItemRuns {
    type Error = String;

    fn try_from(answer: ItemRunsAnswer) -> Result<ItemRuns, String> {
        let shown = answer.past_runs.len();
        if shown > PAST_RUNS_SHOWN || (answer.past_runs_left_out > 0 && shown < PAST_RUNS_SHOWN) {
            return Err(format!(
                "{shown} past runs shown and {} left out is no cut to the latest {PAST_RUNS_SHOWN}",
                answer.past_runs_left_out
            ));
        }
        Ok(ItemRuns {
            runs_of: answer.runs_of,
            run_in_flight: answer.run_in_flight,
            past_runs: answer.past_runs,
            past_runs_left_out: answer.past_runs_left_out,
        })
    }
}

pub const PAST_RUNS_SHOWN: usize = 3;

impl ItemRuns {
    fn of(view: &View, recorded: &[Recorded], item: Number, now: Timestamp) -> ItemRuns {
        let of_item = runs::of_item(recorded, item);
        let run_in_flight = view
            .in_flight
            .iter()
            .find(|run| of_item.iter().any(|of| of.id == run.id))
            .map(|run| run.at(now));
        // A run that has just ended may still be in flight in the view.
        let in_flight = run_in_flight.as_ref().map(|run| &run.id);
        let mut past: Vec<Recorded> = of_item
            .into_iter()
            .filter(|run| run.state != State::InFlight && Some(&run.id) != in_flight)
            .collect();
        runs::sort_newest_first(&mut past);
        let past_runs_left_out = past.len().saturating_sub(PAST_RUNS_SHOWN);
        past.truncate(PAST_RUNS_SHOWN);
        ItemRuns {
            runs_of: item,
            run_in_flight,
            past_runs: past.into_iter().map(|run| run.at(now)).collect(),
            past_runs_left_out,
        }
    }

    fn lines(&self) -> impl Iterator<Item = Line<'_>> {
        let in_flight = self.run_in_flight.iter().map(Line::InFlight);
        let past = self.past_runs.iter().map(Line::Past);
        let left_out = (self.past_runs_left_out > 0).then(|| {
            let how = format!("lw runs {}", self.runs_of.0);
            Line::Item(ItemLine::fact(
                "left out",
                runs::left_out_phrase(self.past_runs_left_out, &how),
            ))
        });
        in_flight.chain(past).chain(left_out)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "FamilyAnswer", try_from = "FamilyAnswer")]
pub struct Family {
    pub parents: Vec<Parent>,
    pub children: Vec<Child>,
}

#[derive(Serialize, Deserialize)]
struct FamilyAnswer {
    /// The first of `parents`, as answers named #N's parent before it could
    /// have several.
    parent: Option<Parent>,
    parents: Vec<Parent>,
    children: Vec<Child>,
}

impl From<Family> for FamilyAnswer {
    fn from(family: Family) -> FamilyAnswer {
        FamilyAnswer {
            parent: family.parents.first().copied(),
            parents: family.parents,
            children: family.children,
        }
    }
}

impl TryFrom<FamilyAnswer> for Family {
    type Error = String;

    fn try_from(answer: FamilyAnswer) -> Result<Family, String> {
        if answer.parent != answer.parents.first().copied() {
            return Err(format!(
                "parent {:?} is not the first of parents {:?}",
                answer.parent, answer.parents
            ));
        }
        Ok(Family {
            parents: answer.parents,
            children: answer.children,
        })
    }
}

impl Family {
    fn of_pull(closes: &[Number]) -> Family {
        Family {
            parents: closes
                .iter()
                .map(|&number| Parent {
                    number,
                    role: ParentRole::IssueItCloses,
                })
                .collect(),
            children: vec![],
        }
    }

    /// The tracking issue is the newest, since a cut discarded part way
    /// leaves an older one.
    fn of_merged(pull: &MergedPull, tracking_issues: &[Number]) -> Family {
        let mut family = Family::of_pull(&pull.closes);
        if pull.designed_label().is_some() {
            family
                .children
                .extend(tracking_issues.iter().max().map(|&number| Child {
                    number,
                    role: ChildRole::TrackingIssue,
                }));
        }
        family
    }

    fn of_issue(issue: &Issue, snapshot: &Snapshot, linewright: &Login) -> Family {
        Family::of_work(
            issue.tracks(linewright),
            &issue.sub_issues,
            issue.parent.filter(|_| issue.was_cut(linewright)),
            snapshot.pulls_closing(issue),
        )
    }

    /// As [`Family::of_issue`], of a closed issue the snapshot holds with its
    /// closing, under `parent`, with `facts`, closed by `closing`.
    fn of_closed_issue(
        parent: Option<domain::Parent>,
        facts: &ManagedFacts,
        closing: Vec<ClosingPull>,
        linewright: &Login,
    ) -> Family {
        Family::of_work(
            facts.tracks(linewright),
            &facts.sub_issues,
            parent.filter(|&parent| facts.was_cut(Some(parent), linewright)),
            closing,
        )
    }

    /// The family of an issue tracking `tracks`, with `sub_issues`, cut from
    /// its tracking issue `cut_from`, if it is a ticket, and closed by
    /// `closing`, in number order.
    fn of_work(
        tracks: Option<Tracks>,
        sub_issues: &[SubIssue],
        cut_from: Option<domain::Parent>,
        closing: Vec<ClosingPull>,
    ) -> Family {
        let tickets = || {
            sub_issues
                .iter()
                .map(|ticket| Child {
                    number: ticket.number,
                    role: ChildRole::Ticket,
                })
                .collect()
        };
        let mut family = match tracks {
            Some(Tracks::SpecChange(spec_change)) => Family {
                parents: vec![Parent {
                    number: spec_change,
                    role: ParentRole::SpecChange,
                }],
                children: tickets(),
            },
            Some(Tracks::Cut) => Family {
                parents: vec![],
                children: tickets(),
            },
            // Only a ticket's parent is its tracking issue.
            None => Family {
                parents: cut_from
                    .map(|parent| Parent {
                        number: parent.number,
                        role: ParentRole::TrackingIssue,
                    })
                    .into_iter()
                    .collect(),
                children: vec![],
            },
        };
        let (open, ended): (Vec<ClosingPull>, Vec<ClosingPull>) =
            closing.into_iter().partition(ClosingPull::is_open);
        family
            .children
            .extend(open.into_iter().chain(ended).map(|pull| Child {
                number: pull.number,
                role: ChildRole::PullRequestClosingIt,
            }));
        family
    }

    fn lines(&self) -> impl Iterator<Item = ItemLine<'_>> {
        let parents = self.parents.iter().map(|parent| {
            ItemLine::fact(
                "parent",
                Phrase::from(parent.number)
                    .text(", ")
                    .text(parent.role.describe()),
            )
        });
        let children = self.children.iter().map(|child| {
            ItemLine::fact(
                "child",
                Phrase::from(child.number)
                    .text(", ")
                    .text(child.role.describe()),
            )
        });
        parents.chain(children)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent {
    pub number: Number,
    pub role: ParentRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParentRole {
    IssueItCloses,
    TrackingIssue,
    SpecChange,
}

impl ParentRole {
    fn describe(self) -> &'static str {
        match self {
            ParentRole::IssueItCloses => "an issue it closes",
            ParentRole::TrackingIssue => "its tracking issue",
            ParentRole::SpecChange => "the spec change it tracks",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Child {
    pub number: Number,
    pub role: ChildRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChildRole {
    PullRequestClosingIt,
    Ticket,
    TrackingIssue,
}

impl ChildRole {
    fn describe(self) -> &'static str {
        match self {
            ChildRole::PullRequestClosingIt => "a pull request that closes it",
            ChildRole::Ticket => "one of its tickets",
            ChildRole::TrackingIssue => "its tracking issue",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Standing {
    // First, so that only an answer naming its state reads as one.
    Item(ItemState),
    PullRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_request_for: Option<Number>,
        #[serde(flatten)]
        review: InReview,
    },
}

impl Standing {
    fn of(state: ItemState) -> Standing {
        if let ItemState::PullRequest {
            pull_request_for,
            review,
        } = state
        {
            Standing::PullRequest {
                pull_request_for,
                review,
            }
        } else {
            Standing::Item(shown(state))
        }
    }
}

/// `None` unless #`number` is a managed item the view holds.
pub fn explain(
    view: &View,
    recorded: &[Recorded],
    number: Number,
    now: Timestamp,
) -> Option<Explanation> {
    let snapshot = &view.snapshot;
    let linewright = &view.policy.linewright;
    let held = managed::held(snapshot, number)?;
    let managed = classify(view, held, now).ok()?;
    let (read, family) = match held {
        Held::Issue(issue) => (
            Item::Issue(number),
            Family::of_issue(issue, snapshot, linewright),
        ),
        Held::Pull(pull) => (Item::Pull(number), Family::of_pull(&pull.closes)),
        Held::Merged(pull) => (
            Item::Pull(number),
            Family::of_merged(
                pull,
                &managed::tracking_issues(snapshot, number, linewright).collect::<Vec<_>>(),
            ),
        ),
        Held::Closed(item) => match &item.kind {
            ClosedKind::Issue { parent, facts, .. } => (
                Item::Issue(number),
                Family::of_closed_issue(*parent, facts, item.closing_pulls(), linewright),
            ),
            ClosedKind::Pull { closes, .. } => (Item::Pull(number), Family::of_pull(closes)),
        },
    };
    Some(Explanation {
        family,
        stalled: snapshot.is_stalled(number),
        last_read: view.read_at.get(&read).copied().unwrap_or(view.polls.last),
        ..explanation(view, recorded, managed, now)
    })
}

/// What `explain` answers of `managed`, read `now`, related to no other item
/// and not stalled.
fn explanation(
    view: &View,
    recorded: &[Recorded],
    managed: Managed,
    now: Timestamp,
) -> Explanation {
    Explanation {
        number: managed.number,
        title: managed.title,
        family: Family::default(),
        standing: Standing::of(managed.state),
        stalled: false,
        last_read: now,
        waiting_for: managed.waiting_for,
        runs: ItemRuns::of(view, recorded, managed.number, now),
    }
}

/// What `explain` answers of #`number` from `view` alone, without reading
/// GitHub: `None` when the view does not hold it.
pub(crate) fn explain_in_view(
    view: &View,
    recorded: &[Recorded],
    number: Number,
    now: Timestamp,
) -> Option<Answer> {
    if let Some(explanation) = explain(view, recorded, number, now) {
        return Some(Answer::Managed(explanation));
    }
    let (held, item) = if let Some(issue) = view.snapshot.issue(number) {
        (
            AnyItem::HeldIssue(Box::new(issue.clone())),
            Item::Issue(number),
        )
    } else {
        let pull = view.snapshot.any_pull(number)?;
        (AnyItem::Pull(Box::new(pull.clone())), Item::Pull(number))
    };
    // Read when the view was, not now.
    let last_read = view.read_at.get(&item).copied().unwrap_or(view.polls.last);
    let mut answer = explain_read(view, recorded, &held, now);
    *answer.last_read_mut() = last_read;
    Some(answer)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftAlone {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub family: Family,
    #[serde(flatten)]
    pub item: LeftAloneItem,
    /// As [`Explanation::stalled`].
    #[serde(default)]
    pub stalled: bool,
    pub last_read: Timestamp,
    #[serde(flatten)]
    pub runs: ItemRuns,
}

/// `view` with `read`, read from GitHub `now`, put in as a poll taking a
/// snapshot now would hold it, and an issue left out of the view put in
/// too, so that what relates to it can be read from the view. A closed
/// issue is put in among the closed items, with its closing.
pub fn fresh_view(view: &View, read: &AnyItem, now: Timestamp) -> View {
    let mut fresh = view.clone();
    let linewright = &view.policy.linewright;
    let held = Arc::make_mut(&mut fresh.snapshot);
    match read {
        AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => {
            held.put_issue(issue.as_ref().clone());
        }
        AnyItem::ClosedIssue { issue, closing } => {
            held.put_closed(ClosedItem::of_issue(issue, *closing));
        }
        AnyItem::Pull(pull) => held.put_read_pull(pull.as_ref().clone(), linewright),
        AnyItem::Merged { pull, .. } => {
            held.remove_peoples_pull(pull.number);
            held.put_merged(pull.as_ref().clone());
        }
    }
    fresh.read_at.insert(read.item(), now);
    fresh
}

pub fn explain_read(view: &View, recorded: &[Recorded], read: &AnyItem, now: Timestamp) -> Answer {
    explain_fresh(&fresh_view(view, read, now), recorded, read, now)
}

/// [`explain_read`] from `fresh`, the view [`fresh_view`] put `read` in.
pub fn explain_fresh(
    fresh: &View,
    recorded: &[Recorded],
    read: &AnyItem,
    now: Timestamp,
) -> Answer {
    let number = read.number();
    let family = match read {
        AnyItem::Pull(pull) => Family::of_pull(&pull.closes),
        AnyItem::Merged {
            pull,
            tracking_issues,
        } => Family::of_merged(pull, tracking_issues),
        AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => {
            Family::of_issue(issue, &fresh.snapshot, &fresh.policy.linewright)
        }
        AnyItem::ClosedIssue { issue, .. } => {
            Family::of_issue(issue, &fresh.snapshot, &fresh.policy.linewright)
        }
    };
    let stalled = read.is_stalled();
    let managed_as = |managed: Managed| {
        Answer::Managed(Explanation {
            family: family.clone(),
            stalled,
            ..explanation(fresh, recorded, managed, now)
        })
    };
    let (title, item) = match read {
        AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => {
            match classify(fresh, Held::Issue(issue), now) {
                Ok(managed) => return managed_as(managed),
                Err(left_alone) => (&issue.title, left_alone),
            }
        }
        AnyItem::ClosedIssue { issue, closing } => {
            let closed = ClosedItem::of_issue(issue, *closing);
            match classify(fresh, Held::Closed(&closed), now) {
                Ok(managed) => return managed_as(managed),
                Err(left_alone) => (&issue.title, left_alone),
            }
        }
        AnyItem::Merged { pull: merged, .. } => match classify(fresh, Held::Merged(merged), now) {
            Ok(managed) => return managed_as(managed),
            Err(left_alone) => (&merged.title, left_alone),
        },
        AnyItem::Pull(pull) => match classify(fresh, Held::Pull(pull), now) {
            Ok(managed) => return managed_as(managed),
            Err(left_alone) => (&pull.title, left_alone),
        },
    };
    Answer::LeftAlone(LeftAlone {
        number,
        title: title.clone(),
        family,
        item,
        stalled,
        last_read: now,
        runs: ItemRuns::of(fresh, recorded, number, now),
    })
}

/// What `explain` shows of an item, after the heading naming it.
enum Line<'a> {
    Item(ItemLine<'a>),
    InFlight(&'a RunInFlight),
    Past(&'a Run),
}

/// A line of what `explain` shows of an item that is not one of its runs.
enum ItemLine<'a> {
    Fact { label: &'static str, phrase: Phrase },
    Wait(&'a Wait),
}

impl ItemLine<'_> {
    fn fact(label: &'static str, phrase: Phrase) -> ItemLine<'static> {
        ItemLine::Fact { label, phrase }
    }

    fn label(&self) -> &'static str {
        match self {
            ItemLine::Fact { label, .. } => label,
            ItemLine::Wait(_) => "waiting for",
        }
    }
}

impl Line<'_> {
    fn label(&self) -> &'static str {
        match self {
            Line::Item(line) => line.label(),
            Line::InFlight(_) => "run in flight",
            Line::Past(_) => "past run",
        }
    }

    fn render(&self, paint: Paint) -> String {
        let shown = match self {
            Line::Item(ItemLine::Fact { phrase, .. }) => phrase.to_string(),
            Line::Item(ItemLine::Wait(wait)) => describe_wait(wait).to_string(),
            Line::InFlight(run) => run.render(paint),
            Line::Past(run) => runs::render_run(run, paint),
        };
        format!("{}: {shown}", self.label())
    }
}

impl Answer {
    pub(crate) fn number(&self) -> Number {
        match self {
            Answer::Managed(explanation) => explanation.number,
            Answer::LeftAlone(left_alone) => left_alone.number,
        }
    }

    fn last_read_mut(&mut self) -> &mut Timestamp {
        match self {
            Answer::Managed(explanation) => &mut explanation.last_read,
            Answer::LeftAlone(left_alone) => &mut left_alone.last_read,
        }
    }

    pub(crate) fn last_read(&self) -> Timestamp {
        match self {
            Answer::Managed(explanation) => explanation.last_read,
            Answer::LeftAlone(left_alone) => left_alone.last_read,
        }
    }

    pub(crate) fn family(&self) -> &Family {
        match self {
            Answer::Managed(explanation) => &explanation.family,
            Answer::LeftAlone(left_alone) => &left_alone.family,
        }
    }

    /// What Linewright waits for before it acts on the item next: nothing
    /// for an item it leaves.
    pub(crate) fn waiting_for(&self) -> &[Wait] {
        match self {
            Answer::Managed(explanation) => &explanation.waiting_for,
            Answer::LeftAlone(_) => &[],
        }
    }

    /// Whether the item is a pull request: one in review, or one Linewright
    /// leaves. A spec change's pull request is managed as an item.
    pub(crate) fn is_pull_request(&self) -> bool {
        match self {
            Answer::Managed(explanation) => {
                matches!(explanation.standing, Standing::PullRequest { .. })
            }
            Answer::LeftAlone(left_alone) => {
                matches!(left_alone.item, LeftAloneItem::PullRequest(_))
            }
        }
    }

    pub(crate) fn title(&self) -> &str {
        match self {
            Answer::Managed(explanation) => &explanation.title,
            Answer::LeftAlone(left_alone) => &left_alone.title,
        }
    }

    /// What `explain` shows of the item, after its number and title.
    fn lines(&self) -> Vec<Line<'_>> {
        let runs = match self {
            Answer::Managed(explanation) => &explanation.runs,
            Answer::LeftAlone(left_alone) => &left_alone.runs,
        };
        self.lines_without_runs()
            .into_iter()
            .map(Line::Item)
            .chain(runs.lines())
            .collect()
    }

    /// [`Answer::lines`] up to its runs: the item's facts and waits, without
    /// the run in flight, the past runs and how many were left out.
    fn lines_without_runs(&self) -> Vec<ItemLine<'_>> {
        match self {
            Answer::Managed(explanation) => explanation_lines(explanation),
            Answer::LeftAlone(left_alone) => left_alone_lines(left_alone),
        }
    }
}

fn left_alone_lines(left_alone: &LeftAlone) -> Vec<ItemLine<'_>> {
    let closed = || Phrase::from("it is closed");
    let (state, why) = match &left_alone.item {
        LeftAloneItem::Issue(LeftAloneIssue::Closed) => ("closed issue", closed()),
        LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone }) => {
            ("open issue", describe_issue_exclusion(left_alone))
        }
        LeftAloneItem::PullRequest(LeftAlonePull::Closed) => ("closed pull request", closed()),
        LeftAloneItem::PullRequest(LeftAlonePull::Open { left_alone }) => {
            ("open pull request", describe_pull_exclusion(left_alone))
        }
        LeftAloneItem::PullRequest(LeftAlonePull::Merged { left_alone }) => (
            "merged pull request",
            Phrase::from("it is no spec change: ").then(describe_not_spec_change(left_alone)),
        ),
    };
    left_alone
        .family
        .lines()
        .chain([
            ItemLine::fact("state", with_stall(state.into(), left_alone.stalled)),
            ItemLine::fact("last read", Phrase::default().time(left_alone.last_read)),
            ItemLine::fact("left alone", why),
        ])
        .collect()
}

/// An item's `state`, saying whether it is `stalled`.
fn with_stall(state: Phrase, stalled: bool) -> Phrase {
    if stalled {
        state.text(" · stalled")
    } else {
        state
    }
}

/// What Linewright waits for on an item it acts on at its next poll.
pub(crate) const NOTHING_WAITED_FOR: &str = "nothing: Linewright acts on it at its next poll";

fn explanation_lines(explanation: &Explanation) -> Vec<ItemLine<'_>> {
    let state = match &explanation.standing {
        Standing::PullRequest { review, .. } | Standing::Item(ItemState::InReview(review)) => {
            review.describe()
        }
        Standing::Item(state) => describe(state),
    };
    // Linewright acts no more on an item that has ended.
    let ended = matches!(
        explanation.standing,
        Standing::Item(ItemState::Ended { .. })
    );
    let nothing = (explanation.waiting_for.is_empty() && !ended)
        .then(|| ItemLine::fact("waiting for", NOTHING_WAITED_FOR.into()));
    explanation
        .family
        .lines()
        .chain([
            ItemLine::fact("state", with_stall(state, explanation.stalled)),
            ItemLine::fact("last read", Phrase::default().time(explanation.last_read)),
        ])
        .chain(nothing)
        .chain(explanation.waiting_for.iter().map(ItemLine::Wait))
        .collect()
}

pub fn render_answer(answer: &Answer, colour: bool) -> String {
    let paint = Paint(colour);
    let heading = format!("{}: {}", paint.bold(answer.number()), answer.title());
    let mut text = String::new();
    for line in std::iter::once(heading).chain(answer.lines().iter().map(|line| line.render(paint)))
    {
        let _ = writeln!(text, "{line}");
    }
    text
}

fn describe_issue_exclusion(exclusion: &IssueExclusion) -> Phrase {
    let subject = match exclusion {
        IssueExclusion::WaitsOnAPerson { .. } => "it ",
        IssueExclusion::NotMarked { .. }
        | IssueExclusion::NoPipelineLabel
        | IssueExclusion::NoTickets
        | IssueExclusion::NoOpenTickets => "it is ",
    };
    Phrase::from(subject).then(exclusion.worded())
}

fn describe_pull_exclusion(exclusion: &PullExclusion) -> Phrase {
    match exclusion {
        PullExclusion::NotWorked { author } => format!(
            "{author} opened it, and Linewright has not worked it and is not assigned to it"
        )
        .into(),
    }
}

fn describe_not_spec_change(because: &NotSpecChange) -> Phrase {
    match because {
        NotSpecChange::Linewrights => "Linewright opened it".into(),
        NotSpecChange::OtherBranch { base } => Phrase::from("it was merged into ")
            .code(base.clone())
            .text(", not the default branch"),
        NotSpecChange::NoSpecFile { spec_glob } => Phrase::from("it touches no file matching ")
            .code(spec_glob.clone())
            .text(", the spec-glob"),
        NotSpecChange::Designed { label } => Phrase::from("it is labeled ")
            .code(label.name())
            .text(": designed already"),
        NotSpecChange::BeforeLookback {
            merged_at,
            window_opened,
        } => Phrase::from("it was merged ")
            .time(*merged_at)
            .text(", before the spec-lookback window opened ")
            .time(*window_opened),
        NotSpecChange::RequiredLabel { label } => Phrase::from("it is not labeled ")
            .code(label.clone())
            .text(", the required label"),
    }
}

pub fn render(explanation: &Explanation, colour: bool) -> String {
    render_answer(&Answer::Managed(explanation.clone()), colour)
}
