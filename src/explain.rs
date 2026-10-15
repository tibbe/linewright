//! `lw explain N`: where issue or pull request #N stands and what
//! Linewright waits for before it acts on it next, or why Linewright leaves
//! it alone (`docs/specs/monitoring.md`, "explain"). `serve` answers it from
//! its own view, with the waits the reconcile step derives, and for an item
//! it does not track from #N read from the store; the command prints the
//! answer.

use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize, Serializer};

use crate::domain::{Item, Login, Number, Pull, Timestamp, label};
use crate::reconcile::{NotSpecChange, Wait, not_a_spec_change};
use crate::runs::{self, Recorded, Run, RunInFlight, State};
use crate::status::{
    InReview, IssueExclusion, ItemState, Paint, Tracked, View, describe, describe_wait, standing,
    tracked, utc,
};
use crate::store::AnyItem;

/// The answer to `explain N`, as the local API sends it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Answer {
    // First, so that only an answer naming why reads as one.
    LeftAlone(LeftAlone),
    Tracked(Explanation),
}

/// The answer to `explain N`, as the local API sends it. Its fields keep
/// their names and meanings (`docs/specs/monitoring.md`, "Output"), and
/// beside them it gives #N's family, which its standing settles.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Explanation {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub standing: Standing,
    /// When `serve` last read #N.
    pub last_read: Timestamp,
    pub waiting_for: Vec<Wait>,
    #[serde(flatten)]
    pub runs: ItemRuns,
}

impl Serialize for Explanation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct WithFamily<'a> {
            number: Number,
            title: &'a str,
            #[serde(flatten)]
            family: Family,
            #[serde(flatten)]
            standing: &'a Standing,
            last_read: Timestamp,
            waiting_for: &'a [Wait],
            #[serde(flatten)]
            runs: &'a ItemRuns,
        }
        // Destructured, so that a field added to `Explanation` must be
        // added here too.
        let Explanation {
            number,
            title,
            standing,
            last_read,
            waiting_for,
            runs,
        } = self;
        WithFamily {
            number: *number,
            title,
            family: standing.family(),
            standing,
            last_read: *last_read,
            waiting_for,
            runs,
        }
        .serialize(serializer)
    }
}

/// An item's runs: its run in flight, with its quiet time, and its latest
/// [`PAST_RUNS_SHOWN`] past runs as `runs N` lists them
/// (`docs/specs/monitoring.md`, "explain"). Its fields are private so
/// that only `ItemRuns::of`, which cuts the past runs to the limit, and a
/// read of its JSON that checks the cut make one.
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
    /// The item the runs are recorded under, which `runs N` lists them for.
    runs_of: Number,
    run_in_flight: Option<RunInFlight>,
    /// Newest first.
    past_runs: Vec<Run>,
    /// How many earlier past runs the limit left out.
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

/// How many past runs `explain` shows.
pub const PAST_RUNS_SHOWN: usize = 3;

impl ItemRuns {
    /// The runs of #`item` at `now`: the one in flight in `view`, and the
    /// others of those `recorded`.
    fn of(view: &View, recorded: &[Recorded], item: Number, now: Timestamp) -> ItemRuns {
        let run_in_flight = view
            .in_flight
            .iter()
            .find(|run| run.id.of.item() == Some(item))
            .map(|run| run.at(now));
        // A run that has just ended may still be in flight in the view.
        let in_flight = run_in_flight.as_ref().map(|run| &run.id);
        let mut past: Vec<Recorded> = recorded
            .iter()
            .filter(|run| {
                run.id.of.item() == Some(item)
                    && run.state != State::InFlight
                    && Some(&run.id) != in_flight
            })
            .cloned()
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

    /// The runs as text, one per line, coloured with `paint`, ending with
    /// how many past runs the limit left out and how to show them.
    fn lines(&self, paint: Paint) -> Vec<String> {
        let in_flight = self
            .run_in_flight
            .iter()
            .map(|run| format!("run in flight: {}", run.render(paint)));
        let past = self
            .past_runs
            .iter()
            .map(|run| format!("past run: {}", runs::render_run(run, paint)));
        let left_out = (self.past_runs_left_out > 0).then(|| {
            let how = format!("lw runs {}", self.runs_of.0);
            runs::render_left_out(self.past_runs_left_out, &how)
        });
        in_flight.chain(past).chain(left_out).collect()
    }
}

/// #N's parent and children in the pipeline
/// (`docs/specs/monitoring.md`, "explain").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Family {
    pub parent: Option<Parent>,
    pub children: Vec<Child>,
}

impl Family {
    /// The family of Linewright's pull request for the issue `issue`, if
    /// it is for one.
    fn pull_request_for(issue: Option<Number>) -> Family {
        Family {
            parent: issue.map(|number| Parent {
                number,
                role: ParentRole::IssueItIsFor,
            }),
            children: vec![],
        }
    }

    /// The family of the pull request by `author` closing the issues
    /// `closes`, as `view` places it at `now`: when Linewright opened it,
    /// the issue it is for.
    fn of_pull(view: &View, author: &Login, closes: &[Number], now: Timestamp) -> Family {
        if author != &view.policy.linewright {
            return Family::default();
        }
        Family::pull_request_for(issue_for(view, closes, now))
    }

    /// The parent and children as text, one per line.
    fn lines(&self) -> Vec<String> {
        let parent = self
            .parent
            .iter()
            .map(|parent| format!("parent: {}, {}", parent.number, parent.role.describe()));
        let children = self
            .children
            .iter()
            .map(|child| format!("child: {}, {}", child.number, child.role.describe()));
        parent.chain(children).collect()
    }
}

/// #N's parent, and what it is to #N.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent {
    pub number: Number,
    pub role: ParentRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParentRole {
    /// The issue Linewright's pull request #N is for.
    IssueItIsFor,
}

impl ParentRole {
    /// What the parent is to #N, as text.
    fn describe(self) -> &'static str {
        match self {
            ParentRole::IssueItIsFor => "the issue it is for",
        }
    }
}

/// One of #N's children, and what it is to #N.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Child {
    pub number: Number,
    pub role: ChildRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChildRole {
    /// Linewright's pull request for issue #N, in review.
    CurrentPullRequest,
}

impl ChildRole {
    /// What the child is to #N, as text.
    fn describe(self) -> &'static str {
        match self {
            ChildRole::CurrentPullRequest => "its current pull request",
        }
    }
}

/// The issue Linewright's pull request closing the issues `closes` is for,
/// as `view` places them at `now`: the one it closes, or of several the one
/// Linewright tracks, as [`pull_standing`] finds it.
fn issue_for(view: &View, closes: &[Number], now: Timestamp) -> Option<Number> {
    match closes {
        [issue] => Some(*issue),
        _ => tracked_closed_by(view, closes, now).map(|t| t.number),
    }
}

/// The issue among `closes` that Linewright tracks in `view` at `now`.
fn tracked_closed_by(view: &View, closes: &[Number], now: Timestamp) -> Option<Tracked> {
    tracked(view, now)
        .into_iter()
        .find(|t| closes.contains(&t.number))
}

/// Where #N is in the pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Standing {
    /// Linewright's pull request for the issue `pull_request_for`, which is
    /// in review with it; the waits are the issue's.
    PullRequest {
        pull_request_for: Number,
        #[serde(flatten)]
        review: InReview,
    },
    /// An item Linewright tracks.
    Item(ItemState),
}

impl Standing {
    /// #N's parent and children: the issue a pull request is for, or an
    /// issue's pull request in review.
    pub fn family(&self) -> Family {
        match self {
            Standing::PullRequest {
                pull_request_for, ..
            } => Family::pull_request_for(Some(*pull_request_for)),
            Standing::Item(ItemState::InReview(review)) => Family {
                parent: None,
                children: vec![Child {
                    number: review.pull_request,
                    role: ChildRole::CurrentPullRequest,
                }],
            },
            Standing::Item(_) => Family::default(),
        }
    }
}

/// The answer about #`number` at `now`, with its runs among those
/// `recorded`: an item Linewright tracks, or Linewright's open pull request
/// for one, which shows the runs of the issue it is for, as the runs on it
/// are recorded under the issue. `None` for any other number.
pub fn explain(
    view: &View,
    recorded: &[Recorded],
    number: Number,
    now: Timestamp,
) -> Option<Explanation> {
    let items = tracked(view, now);
    let last_read = |item: Item| view.read_at.get(&item).copied().unwrap_or(view.polls.last);
    if let Some(tracked) = items.iter().find(|t| t.number == number) {
        // A tracked spec change is a merged pull request; the rest are issues.
        let read = if view.snapshot.issue(number).is_some() {
            Item::Issue(number)
        } else {
            Item::Pull(number)
        };
        return Some(Explanation {
            number,
            title: tracked.title.clone(),
            standing: Standing::Item(tracked.state.clone()),
            last_read: last_read(read),
            waiting_for: tracked.waiting_for.clone(),
            runs: ItemRuns::of(view, recorded, number, now),
        });
    }
    items.into_iter().find_map(|tracked| match tracked.state {
        ItemState::InReview(review) if review.pull_request == number => Some(Explanation {
            number,
            title: view.snapshot.pull(number)?.title.clone(),
            standing: Standing::PullRequest {
                pull_request_for: tracked.number,
                review,
            },
            last_read: last_read(Item::Pull(number)),
            waiting_for: tracked.waiting_for,
            runs: ItemRuns::of(view, recorded, tracked.number, now),
        }),
        _ => None,
    })
}

/// An item Linewright leaves alone, and why. Its fields keep their names and
/// meanings (`docs/specs/monitoring.md`, "Output").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftAlone {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub family: Family,
    #[serde(flatten)]
    pub item: LeftAloneItem,
    /// When `serve` read #N.
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

/// An issue Linewright leaves alone: closed, or open and kept out by
/// `left_alone`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum LeftAloneIssue {
    Closed,
    Open { left_alone: OpenIssueExclusion },
}

/// Why Linewright leaves an open issue alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum OpenIssueExclusion {
    /// A snapshot does not read it: it has none of the labels that bring an
    /// issue in, and Linewright neither opened it nor is assigned to it.
    NotInView,
    /// A snapshot reads it, and the rules that place an issue in the
    /// pipeline keep it out.
    #[serde(untagged)]
    Held(IssueExclusion),
}

/// A pull request Linewright leaves alone: closed without merging, open and
/// kept out by `left_alone`, or merged and no spec change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum LeftAlonePull {
    Closed,
    Open { left_alone: PullExclusion },
    Merged { left_alone: NotSpecChange },
}

/// Why Linewright leaves an open pull request alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum PullExclusion {
    /// Someone other than Linewright, `author`, opened it.
    NotLinewrights { author: Login },
    /// It is Linewright's, closing no issue.
    ClosesNoIssue,
    /// It is Linewright's for `issue`, which Linewright works on through its
    /// newer pull request `by`.
    Superseded { issue: Number, by: Number },
    /// It is Linewright's for `issue`, which Linewright tracks in
    /// `issue_state`, not in review.
    IssueNotInReview {
        issue: Number,
        issue_state: ItemState,
    },
    /// It is Linewright's for `issues`, none of which Linewright tracks.
    ForIssuesLeftAlone { issues: Vec<Number> },
}

/// The answer about `read`, #N as `serve` read it from the store at `now`
/// because its view tracks no such item: where #N stands if the read places
/// it in the pipeline after all, as a label applied since the last poll
/// does, or else why Linewright leaves it alone, by the rules that decide
/// what it tracks; with its runs among those `recorded`, as [`explain`]
/// shows them.
pub fn explain_read(view: &View, recorded: &[Recorded], read: &AnyItem, now: Timestamp) -> Answer {
    let mut fresh = view.clone();
    let linewright = &view.policy.linewright;
    // As a poll taking a snapshot now would hold #N.
    let held = Arc::make_mut(&mut fresh.snapshot);
    match read {
        AnyItem::HeldIssue(issue) => held.put_issue(issue.as_ref().clone()),
        AnyItem::Pull(pull) if &pull.author == linewright => held.put_pull(pull.as_ref().clone()),
        AnyItem::Merged(merged) => held.put_merged(merged.clone()),
        AnyItem::LeftOutIssue(_) | AnyItem::Pull(_) => {}
    }
    fresh.read_at.insert(read.item(), now);
    let number = read.number();
    let mut waits = fresh.decisions(now).waits;
    let mut take_waits = || waits.remove(&number).unwrap_or_default();
    let tracked_as = |title: &str, state: ItemState, waiting_for: Vec<Wait>| {
        Answer::Tracked(Explanation {
            number,
            title: title.to_string(),
            standing: Standing::Item(state),
            last_read: now,
            waiting_for,
            runs: ItemRuns::of(view, recorded, number, now),
        })
    };
    let family = match read {
        AnyItem::Pull(pull) => Family::of_pull(&fresh, &pull.author, &pull.closes, now),
        AnyItem::Merged(merged) => Family::of_pull(&fresh, &merged.author, &merged.closes, now),
        AnyItem::HeldIssue(_) | AnyItem::LeftOutIssue(_) => Family::default(),
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
                Ok(state) => return tracked_as(&issue.title, state, waiting_for),
                Err(exclusion) => (
                    &issue.title,
                    LeftAloneItem::Issue(LeftAloneIssue::Open {
                        left_alone: OpenIssueExclusion::Held(exclusion),
                    }),
                ),
            }
        }
        AnyItem::Merged(merged) => {
            let default_branch = &fresh.snapshot.default_branch;
            match not_a_spec_change(merged, default_branch, now, &fresh.policy) {
                None => {
                    let state = ItemState::WaitingForTickets;
                    return tracked_as(&merged.title, state, take_waits());
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
            Ok(explanation) => return Answer::Tracked(explanation),
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
        last_read: now,
        runs: ItemRuns::of(view, recorded, number, now),
    })
}

/// Where the open pull request `pull` stands in `view` at `now`, as
/// Linewright's pull request for an issue in review, or why Linewright
/// leaves it alone.
fn pull_standing(
    view: &View,
    recorded: &[Recorded],
    pull: &Pull,
    now: Timestamp,
) -> Result<Explanation, PullExclusion> {
    if pull.author != view.policy.linewright {
        return Err(PullExclusion::NotLinewrights {
            author: pull.author.clone(),
        });
    }
    if pull.closes.is_empty() {
        return Err(PullExclusion::ClosesNoIssue);
    }
    let Some(issue) = tracked_closed_by(view, &pull.closes, now) else {
        return Err(PullExclusion::ForIssuesLeftAlone {
            issues: pull.closes.clone(),
        });
    };
    match issue.state {
        ItemState::InReview(review) if review.pull_request == pull.number => Ok(Explanation {
            number: pull.number,
            title: pull.title.clone(),
            standing: Standing::PullRequest {
                pull_request_for: issue.number,
                review,
            },
            last_read: now,
            waiting_for: issue.waiting_for,
            runs: ItemRuns::of(view, recorded, issue.number, now),
        }),
        ItemState::InReview(review) => Err(PullExclusion::Superseded {
            issue: issue.number,
            by: review.pull_request,
        }),
        issue_state => Err(PullExclusion::IssueNotInReview {
            issue: issue.number,
            issue_state,
        }),
    }
}

/// The answer as text, as [`render`] gives a tracked item's, and for an item
/// Linewright leaves alone: where it is and why.
pub fn render_answer(answer: &Answer, colour: bool) -> String {
    let left_alone = match answer {
        Answer::Tracked(explanation) => return render(explanation, colour),
        Answer::LeftAlone(left_alone) => left_alone,
    };
    let paint = Paint(colour);
    let closed = || "it is closed".to_string();
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
            format!(
                "it is no spec change: {}",
                describe_not_spec_change(left_alone)
            ),
        ),
    };
    let lines = [format!(
        "{}: {}",
        paint.bold(&left_alone.number.to_string()),
        left_alone.title
    )]
    .into_iter()
    .chain(left_alone.family.lines())
    .chain([
        format!("state: {state}"),
        format!("last read: {}", utc(left_alone.last_read)),
        format!("left alone: {why}"),
    ])
    .chain(left_alone.runs.lines(paint));
    let mut text = String::new();
    for line in lines {
        let _ = writeln!(text, "{line}");
    }
    text
}

/// The labels `names`, quoted, as "`a`, `b` or `c`".
fn one_of(names: &[&str]) -> String {
    let names: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match names.as_slice() {
        [rest @ .., last] if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

/// Why Linewright leaves an open issue alone, as a phrase after "left
/// alone:".
fn describe_issue_exclusion(exclusion: &OpenIssueExclusion) -> String {
    let exclusion = match exclusion {
        OpenIssueExclusion::NotInView => {
            return format!(
                "it is labeled none of {}, and Linewright neither opened it nor is assigned to it",
                one_of(&label::IN_VIEW)
            );
        }
        OpenIssueExclusion::Held(exclusion) => exclusion,
    };
    match exclusion {
        IssueExclusion::NoPipelineLabel => {
            format!("it is labeled none of {}", one_of(&label::STATES))
        }
        IssueExclusion::OwnIssueForTriage => format!(
            "Linewright opened it and triages no issue of its own, though it is labeled `{}`",
            label::NEEDS_TRIAGE
        ),
        IssueExclusion::NoTickets => {
            "it is a tracking issue with no tickets, and none are being cut".into()
        }
    }
}

/// Why Linewright leaves an open pull request alone, as a phrase after
/// "left alone:".
fn describe_pull_exclusion(exclusion: &PullExclusion) -> String {
    match exclusion {
        PullExclusion::NotLinewrights { author } => {
            format!("{author} opened it, and Linewright works only on its own pull requests")
        }
        PullExclusion::ClosesNoIssue => {
            "it is Linewright's, and closes no issue Linewright could work on".into()
        }
        PullExclusion::Superseded { issue, by } => {
            format!("it is Linewright's for {issue}, which it works on through {by}")
        }
        PullExclusion::IssueNotInReview { issue, issue_state } => format!(
            "it is Linewright's for {issue}, which is not in review but {}",
            describe(issue_state)
        ),
        PullExclusion::ForIssuesLeftAlone { issues } => {
            let issues: Vec<String> = issues.iter().map(Number::to_string).collect();
            format!(
                "it is Linewright's for {}, which Linewright does not work on",
                issues.join(", ")
            )
        }
    }
}

/// Why a merged pull request is no spec change, as a phrase.
fn describe_not_spec_change(because: &NotSpecChange) -> String {
    match because {
        NotSpecChange::Linewrights => "Linewright opened it".into(),
        NotSpecChange::OtherBranch { base } => {
            format!("it was merged into `{base}`, not the default branch")
        }
        NotSpecChange::NoSpecFile { spec_glob } => {
            format!("it touches no file matching `{spec_glob}`, the spec-glob")
        }
        NotSpecChange::Processed => format!(
            "it is labeled `{}`: cut into tickets already",
            label::SPEC_PROCESSED
        ),
        NotSpecChange::HandedBack => format!(
            "it is labeled `{}`: handed to the maintainer",
            label::READY_FOR_HUMAN
        ),
        NotSpecChange::BeforeLookback {
            merged_at,
            window_opened,
        } => format!(
            "it was merged {}, before the spec-lookback window opened {}",
            utc(*merged_at),
            utc(*window_opened)
        ),
        NotSpecChange::RequiredLabel { label } => {
            format!("it is not labeled `{label}`, the required label")
        }
    }
}

/// The answer as text: one fact per line as `label: value`, coloured if
/// `colour` (`docs/specs/monitoring.md`, "Output").
pub fn render(explanation: &Explanation, colour: bool) -> String {
    let paint = Paint(colour);
    let mut lines = vec![format!(
        "{}: {}",
        paint.bold(&explanation.number.to_string()),
        explanation.title
    )];
    lines.extend(explanation.standing.family().lines());
    let state = match &explanation.standing {
        Standing::PullRequest { review, .. } => review.describe(),
        Standing::Item(state) => describe(state),
    };
    lines.push(format!("state: {state}"));
    lines.push(format!("last read: {}", utc(explanation.last_read)));
    if explanation.waiting_for.is_empty() {
        lines.push("waiting for: nothing: Linewright acts on it at its next poll".into());
    }
    for wait in &explanation.waiting_for {
        lines.push(format!("waiting for: {}", describe_wait(wait)));
    }
    lines.extend(explanation.runs.lines(paint));
    let mut text = String::new();
    for line in lines {
        let _ = writeln!(text, "{line}");
    }
    text
}
