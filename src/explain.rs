use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::{
    ClosingPull, Issue, Item, Login, MergedPull, Number, Pull, Snapshot, StatusLabel, Timestamp,
    Tracks, label,
};
use crate::phrase::{Phrase, Tone};
use crate::reconcile::{NotSpecChange, NotWorked, Wait, not_a_spec_change, worked_ticket};
use crate::runs::{self, Recorded, Run, RunInFlight, State};
use crate::status::{
    InReview, IssueExclusion, ItemState, Paint, View, describe, describe_wait, managed, standing,
};
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
            Line::fact(
                "left out",
                runs::left_out_phrase(self.past_runs_left_out, &how),
            )
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
        let tickets = || {
            issue
                .sub_issues
                .iter()
                .map(|ticket| Child {
                    number: ticket.linked.number,
                    role: ChildRole::Ticket,
                })
                .collect()
        };
        let mut family = match issue.tracks(linewright) {
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
                parents: issue
                    .parent
                    .filter(|_| issue.was_cut(linewright))
                    .map(|parent| Parent {
                        number: parent.number,
                        role: ParentRole::TrackingIssue,
                    })
                    .into_iter()
                    .collect(),
                children: vec![],
            },
        };
        let (open, ended): (Vec<ClosingPull>, Vec<ClosingPull>) = snapshot
            .pulls_closing(issue)
            .into_iter()
            .partition(ClosingPull::is_open);
        family
            .children
            .extend(open.into_iter().chain(ended).map(|pull| Child {
                number: pull.number,
                role: ChildRole::PullRequestClosingIt,
            }));
        family
    }

    fn lines(&self) -> impl Iterator<Item = Line<'_>> {
        let parents = self.parents.iter().map(|parent| {
            Line::fact(
                "parent",
                Phrase::from(parent.number)
                    .text(", ")
                    .text(parent.role.describe()),
            )
        });
        let children = self.children.iter().map(|child| {
            Line::fact(
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
    PullRequest {
        pull_request_for: Number,
        #[serde(flatten)]
        review: InReview,
    },
    Item(ItemState),
}

/// `None` unless #`number` is a managed item or Linewright's
/// open pull request it works on.
pub fn explain(
    view: &View,
    recorded: &[Recorded],
    number: Number,
    now: Timestamp,
) -> Option<Explanation> {
    let last_read = |item: Item| view.read_at.get(&item).copied().unwrap_or(view.polls.last);
    if let Some(pull) = view.snapshot.pull(number).filter(|p| p.is_open()) {
        let explanation = pull_standing(view, recorded, pull, now).ok()?;
        return Some(Explanation {
            last_read: last_read(Item::Pull(number)),
            ..explanation
        });
    }
    let items = managed(view, now);
    if let Some(managed) = items.iter().find(|m| m.number == number) {
        // A managed spec change is a merged pull request, not yet cut into
        // tickets; the rest are issues.
        let (read, family) = match view.snapshot.issue(number) {
            Some(issue) => (
                Item::Issue(number),
                Family::of_issue(issue, &view.snapshot, &view.policy.linewright),
            ),
            None => (
                Item::Pull(number),
                view.snapshot
                    .merged_pull(number)
                    .map(|pull| Family::of_pull(&pull.closes))
                    .unwrap_or_default(),
            ),
        };
        return Some(Explanation {
            number,
            title: managed.title.clone(),
            family,
            standing: Standing::Item(managed.state.clone()),
            stalled: view.snapshot.is_stalled(number),
            last_read: last_read(read),
            waiting_for: managed.waiting_for.clone(),
            runs: ItemRuns::of(view, recorded, number, now),
        });
    }
    None
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
        let pull = view.snapshot.pull(number)?;
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
    Open { left_alone: OpenIssueExclusion },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum OpenIssueExclusion {
    NotInView,
    #[serde(untagged)]
    Held(IssueExclusion),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum LeftAlonePull {
    Closed,
    Open { left_alone: PullExclusion },
    Merged { left_alone: NotSpecChange },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum PullExclusion {
    NotLinewrights { author: Login },
    ClosesNoIssue,
    Superseded { issue: Number, by: Number },
}

pub fn explain_read(view: &View, recorded: &[Recorded], read: &AnyItem, now: Timestamp) -> Answer {
    let mut fresh = view.clone();
    let linewright = &view.policy.linewright;
    // As a poll taking a snapshot now would hold #N.
    let held = Arc::make_mut(&mut fresh.snapshot);
    match read {
        AnyItem::HeldIssue(issue) => held.put_issue(issue.as_ref().clone()),
        AnyItem::Pull(pull) if &pull.author == linewright => held.put_pull(pull.as_ref().clone()),
        AnyItem::Merged { pull, .. } => held.put_merged(pull.as_ref().clone()),
        AnyItem::LeftOutIssue(_) | AnyItem::Pull(_) => {}
    }
    fresh.read_at.insert(read.item(), now);
    let number = read.number();
    let mut waits = fresh.decisions(now).waits;
    let mut take_waits = || waits.remove(&number).unwrap_or_default();
    let family = match read {
        AnyItem::Pull(pull) => Family::of_pull(&pull.closes),
        AnyItem::Merged {
            pull,
            tracking_issues,
        } => Family::of_merged(pull, tracking_issues),
        AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => {
            Family::of_issue(issue, &fresh.snapshot, &fresh.policy.linewright)
        }
    };
    let stalled = read.is_stalled();
    let managed_as = |title: &str, state: ItemState, waiting_for| {
        Answer::Managed(Explanation {
            number,
            title: title.to_string(),
            family: family.clone(),
            standing: Standing::Item(state),
            stalled,
            last_read: now,
            waiting_for,
            runs: ItemRuns::of(view, recorded, number, now),
        })
    };
    let (title, item) = match read {
        AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) if !issue.open => {
            (&issue.title, LeftAloneItem::Issue(LeftAloneIssue::Closed))
        }
        AnyItem::LeftOutIssue(issue) => (
            &issue.title,
            LeftAloneItem::Issue(LeftAloneIssue::Open {
                left_alone: OpenIssueExclusion::NotInView,
            }),
        ),
        AnyItem::HeldIssue(issue) => {
            let waiting_for = take_waits();
            match standing(issue, &fresh.snapshot, &fresh.policy, &waiting_for) {
                Ok(state) => return managed_as(&issue.title, state, waiting_for),
                Err(exclusion) => (
                    &issue.title,
                    LeftAloneItem::Issue(LeftAloneIssue::Open {
                        left_alone: OpenIssueExclusion::Held(exclusion),
                    }),
                ),
            }
        }
        AnyItem::Merged { pull: merged, .. } => {
            let default_branch = &fresh.snapshot.default_branch;
            match not_a_spec_change(merged, default_branch, now, &fresh.policy) {
                None => {
                    let state = ItemState::WaitingForDesign;
                    return managed_as(&merged.title, state, take_waits());
                }
                Some(left_alone) => (
                    &merged.title,
                    LeftAloneItem::PullRequest(LeftAlonePull::Merged { left_alone }),
                ),
            }
        }
        AnyItem::Pull(pull) if !pull.is_open() => (
            &pull.title,
            LeftAloneItem::PullRequest(LeftAlonePull::Closed),
        ),
        AnyItem::Pull(pull) => match pull_standing(&fresh, recorded, pull, now) {
            Ok(explanation) => return Answer::Managed(explanation),
            Err(left_alone) => (
                &pull.title,
                LeftAloneItem::PullRequest(LeftAlonePull::Open { left_alone }),
            ),
        },
    };
    Answer::LeftAlone(LeftAlone {
        number,
        title: title.clone(),
        family,
        item,
        stalled,
        last_read: now,
        runs: ItemRuns::of(view, recorded, number, now),
    })
}

fn pull_standing(
    view: &View,
    recorded: &[Recorded],
    pull: &Pull,
    now: Timestamp,
) -> Result<Explanation, PullExclusion> {
    let ticket =
        worked_ticket(&view.snapshot, pull, &view.policy.linewright).map_err(|not_worked| {
            match not_worked {
                NotWorked::NotLinewrights => PullExclusion::NotLinewrights {
                    author: pull.author.clone(),
                },
                NotWorked::ForNoIssue => PullExclusion::ClosesNoIssue,
                NotWorked::Superseded { ticket, by } => {
                    PullExclusion::Superseded { issue: ticket, by }
                }
            }
        })?;
    let waiting_for = view.decisions(now).waits.remove(&pull.number);
    Ok(Explanation {
        number: pull.number,
        title: pull.title.clone(),
        family: Family::of_pull(&pull.closes),
        standing: Standing::PullRequest {
            pull_request_for: ticket,
            review: InReview::of(pull),
        },
        stalled: pull.is_stalled(),
        last_read: now,
        waiting_for: waiting_for.unwrap_or_default(),
        runs: ItemRuns::of(view, recorded, pull.number, now),
    })
}

/// What `explain` shows of an item, after the heading naming it.
pub(crate) enum Line<'a> {
    Fact {
        label: &'static str,
        phrase: Phrase,
        tone: Option<Tone>,
    },
    Wait(&'a Wait),
    InFlight(&'a RunInFlight),
    Past(&'a Run),
}

impl Line<'_> {
    fn fact(label: &'static str, phrase: Phrase) -> Line<'static> {
        Line::Fact {
            label,
            phrase,
            tone: None,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Line::Fact { label, .. } => label,
            Line::Wait(_) => "waiting for",
            Line::InFlight(_) => "run in flight",
            Line::Past(_) => "past run",
        }
    }

    fn render(&self, paint: Paint) -> String {
        let shown = match self {
            Line::Fact { phrase, .. } => phrase.to_string(),
            Line::Wait(wait) => describe_wait(wait).to_string(),
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

    pub(crate) fn title(&self) -> &str {
        match self {
            Answer::Managed(explanation) => &explanation.title,
            Answer::LeftAlone(left_alone) => &left_alone.title,
        }
    }

    /// What `explain` shows of the item, after its number and title.
    pub(crate) fn lines(&self) -> Vec<Line<'_>> {
        let runs = match self {
            Answer::Managed(explanation) => &explanation.runs,
            Answer::LeftAlone(left_alone) => &left_alone.runs,
        };
        let mut lines = self.lines_without_runs();
        lines.extend(runs.lines());
        lines
    }

    /// [`Answer::lines`] up to its runs: the item's facts and waits, without
    /// the run in flight, the past runs and how many were left out.
    #[allow(dead_code, reason = "the item page's runs table is to call it")]
    pub(crate) fn lines_without_runs(&self) -> Vec<Line<'_>> {
        match self {
            Answer::Managed(explanation) => explanation_lines(explanation),
            Answer::LeftAlone(left_alone) => left_alone_lines(left_alone),
        }
    }
}

fn left_alone_lines(left_alone: &LeftAlone) -> Vec<Line<'_>> {
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
    let tone = match left_alone.item {
        LeftAloneItem::Issue(LeftAloneIssue::Closed)
        | LeftAloneItem::PullRequest(LeftAlonePull::Closed) => Some(Tone::Cancelled),
        LeftAloneItem::PullRequest(LeftAlonePull::Merged { .. }) => Some(Tone::Success),
        LeftAloneItem::Issue(LeftAloneIssue::Open { .. })
        | LeftAloneItem::PullRequest(LeftAlonePull::Open { .. }) => None,
    };
    left_alone
        .family
        .lines()
        .chain([
            Line::Fact {
                label: "state",
                phrase: with_stall(state.into(), left_alone.stalled),
                tone,
            },
            Line::fact("last read", Phrase::default().time(left_alone.last_read)),
            Line::fact("left alone", why),
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

fn explanation_lines(explanation: &Explanation) -> Vec<Line<'_>> {
    let (state, tone) = match &explanation.standing {
        Standing::PullRequest { review, .. } | Standing::Item(ItemState::InReview(review)) => {
            (review.describe(), review.ci.tone())
        }
        Standing::Item(state) => (describe(state), None),
    };
    let nothing = explanation
        .waiting_for
        .is_empty()
        .then(|| Line::fact("waiting for", NOTHING_WAITED_FOR.into()));
    explanation
        .family
        .lines()
        .chain([
            Line::Fact {
                label: "state",
                phrase: with_stall(state, explanation.stalled),
                tone,
            },
            Line::fact("last read", Phrase::default().time(explanation.last_read)),
        ])
        .chain(nothing)
        .chain(explanation.waiting_for.iter().map(Line::Wait))
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

fn one_of(names: &[&str]) -> Phrase {
    Phrase::list(names.iter().map(|&name| Phrase::default().code(name)), "or")
}

fn describe_issue_exclusion(exclusion: &OpenIssueExclusion) -> Phrase {
    let exclusion = match exclusion {
        OpenIssueExclusion::NotInView => {
            return Phrase::from("it is labeled none of ")
                .then(one_of(&label::IN_VIEW))
                .text(", and Linewright neither opened it nor is assigned to it");
        }
        OpenIssueExclusion::Held(exclusion) => exclusion,
    };
    match exclusion {
        IssueExclusion::NoPipelineLabel => Phrase::from("it is labeled none of ")
            .then(one_of(&StatusLabel::ALL.map(StatusLabel::label))),
        IssueExclusion::NoTickets => {
            "it is a tracking issue with no tickets, and none are being cut".into()
        }
    }
}

fn describe_pull_exclusion(exclusion: &PullExclusion) -> Phrase {
    match exclusion {
        PullExclusion::NotLinewrights { author } => {
            format!("{author} opened it, and Linewright works only on its own pull requests").into()
        }
        PullExclusion::ClosesNoIssue => {
            "it is Linewright's, and closes no issue Linewright could work on".into()
        }
        PullExclusion::Superseded { issue, by } => Phrase::from("it is Linewright's for ")
            .item(*issue)
            .text(", which it works on through ")
            .item(*by),
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
        NotSpecChange::Unassigned {
            at,
            required_label: None,
        } => Phrase::from("Linewright was unassigned from it ")
            .time(*at)
            .text(": assign Linewright to cut it again"),
        NotSpecChange::Unassigned {
            at,
            required_label: Some(label),
        } => Phrase::from("Linewright was unassigned from it ")
            .time(*at)
            .text(", and ")
            .code(label.clone())
            .text(
                ", the required label, was not applied since: assign Linewright or apply the \
                 label to cut it again",
            ),
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
