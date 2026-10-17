use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::{
    self, ClosingPull, Filed, Issue, Item, Login, MergedPull, Number, Pull, Snapshot, StatusLabel,
    Timestamp, label,
};
use crate::reconcile::{NotSpecChange, NotWorked, Wait, not_a_spec_change, worked_ticket};
use crate::runs::{self, Recorded, Run, RunInFlight, State};
use crate::status::{
    InReview, IssueExclusion, ItemState, Paint, View, describe, describe_wait, standing, tracked,
    utc,
};
use crate::store::AnyItem;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Answer {
    // First, so that only an answer naming why reads as one.
    LeftAlone(LeftAlone),
    Tracked(Explanation),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub family: Family,
    #[serde(flatten)]
    pub standing: Standing,
    pub last_read: Timestamp,
    pub waiting_for: Vec<Wait>,
    #[serde(flatten)]
    pub runs: ItemRuns,
}

impl Explanation {
    fn new(
        number: Number,
        title: String,
        family: Family,
        standing: Standing,
        last_read: Timestamp,
        waiting_for: Vec<Wait>,
        runs: ItemRuns,
    ) -> Explanation {
        Explanation {
            number,
            title,
            family,
            standing,
            last_read,
            waiting_for,
            runs,
        }
    }
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
        if pull.has_label(label::SPEC_PROCESSED) {
            family
                .children
                .extend(tracking_issues.iter().max().map(|&number| Child {
                    number,
                    role: ChildRole::TrackingIssue,
                }));
        }
        family
    }

    fn of_issue(issue: &Issue, snapshot: &Snapshot) -> Family {
        let mut family = match issue.filed {
            Some(Filed::Tracking { spec_change }) => Family {
                parents: vec![Parent {
                    number: spec_change,
                    role: ParentRole::SpecChange,
                }],
                children: issue
                    .sub_issues
                    .iter()
                    .map(|ticket| Child {
                        number: ticket.number,
                        role: ChildRole::Ticket,
                    })
                    .collect(),
            },
            _ => Family {
                parents: issue
                    .parent
                    .into_iter()
                    .filter_map(|parent| match parent {
                        // Only a tracking issue knows its spec change.
                        domain::Parent {
                            number,
                            spec_change: Some(_),
                        } => Some(Parent {
                            number,
                            role: ParentRole::TrackingIssue,
                        }),
                        domain::Parent {
                            spec_change: None, ..
                        } => None,
                    })
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

    fn lines(&self) -> Vec<String> {
        let parents = self
            .parents
            .iter()
            .map(|parent| format!("parent: {}, {}", parent.number, parent.role.describe()));
        let children = self
            .children
            .iter()
            .map(|child| format!("child: {}, {}", child.number, child.role.describe()));
        parents.chain(children).collect()
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

/// `None` unless #`number` is an item Linewright tracks or Linewright's
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
    let items = tracked(view, now);
    if let Some(tracked) = items.iter().find(|t| t.number == number) {
        // A tracked spec change is a merged pull request, not yet cut into
        // tickets; the rest are issues.
        let (read, family) = match view.snapshot.issue(number) {
            Some(issue) => (Item::Issue(number), Family::of_issue(issue, &view.snapshot)),
            None => (
                Item::Pull(number),
                view.snapshot
                    .merged_pull(number)
                    .map(|pull| Family::of_pull(&pull.closes))
                    .unwrap_or_default(),
            ),
        };
        return Some(Explanation::new(
            number,
            tracked.title.clone(),
            family,
            Standing::Item(tracked.state.clone()),
            last_read(read),
            tracked.waiting_for.clone(),
            ItemRuns::of(view, recorded, number, now),
        ));
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftAlone {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub family: Family,
    #[serde(flatten)]
    pub item: LeftAloneItem,
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
            Family::of_issue(issue, &fresh.snapshot)
        }
    };
    let tracked_as = |title: &str, state: ItemState, waiting_for| {
        Answer::Tracked(Explanation::new(
            number,
            title.to_string(),
            family.clone(),
            Standing::Item(state),
            now,
            waiting_for,
            ItemRuns::of(view, recorded, number, now),
        ))
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
        AnyItem::Merged { pull: merged, .. } => {
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
    Ok(Explanation::new(
        pull.number,
        pull.title.clone(),
        Family::of_pull(&pull.closes),
        Standing::PullRequest {
            pull_request_for: ticket,
            review: InReview::of(pull),
        },
        now,
        waiting_for.unwrap_or_default(),
        ItemRuns::of(view, recorded, pull.number, now),
    ))
}

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

fn one_of(names: &[&str]) -> String {
    let names: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match names.as_slice() {
        [rest @ .., last] if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

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
            format!(
                "it is labeled none of {}",
                one_of(&StatusLabel::ALL.map(StatusLabel::label))
            )
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
    }
}

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

pub fn render(explanation: &Explanation, colour: bool) -> String {
    let paint = Paint(colour);
    let mut lines = vec![format!(
        "{}: {}",
        paint.bold(&explanation.number.to_string()),
        explanation.title
    )];
    lines.extend(explanation.family.lines());
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
