//! A card's words (`docs/specs/design_system.md`, Cards), and a lane's head
//! (The board), shared by every page that shows a card's line or status or
//! a lane's head.

use super::html::{self, Html};
use super::names::Names;
use crate::board::{
    Card, CardLine, CardStatus, Elapsed, Ended, Hold, LaneHead, LaneHeading, Running, Stop,
};
use crate::domain::{IssueCloseReason, ReviewDecision};
use crate::phrase::{Phrase, Tone};
use crate::reconcile::WorkKind;

/// A card's line, the one place a page names items by their number alone
/// (`docs/specs/design_system.md`, Navigation). Made only by [`card_line`].
pub(super) struct Line(Phrase);

impl Line {
    /// The line as HTML, each item it names by its number alone.
    pub(super) fn html(&self, names: &Names) -> Html {
        html::phrase(&self.0, &names.numbers_only())
    }
}

/// The line under `card`'s title, ending with when it entered its column
/// where it says so.
pub(super) fn card_line(card: &Card) -> Option<Line> {
    card_phrase(card).map(Line)
}

fn card_phrase(card: &Card) -> Option<Phrase> {
    let line = card.line.as_ref().map(worded);
    let entered = card.entered.map(|at| Phrase::default().time(at));
    match (line, entered) {
        (Some(line), Some(entered)) => Some(line.text(" · ").then(entered)),
        (line, entered) => line.or(entered),
    }
}

/// A lane's head (`docs/specs/design_system.md`, The board, Lanes and
/// Finished lanes): the tracking issue's number and title and its tickets
/// closed of total, or the item's number and title; then when it finished,
/// if it did.
pub(super) fn lane_head(heading: &LaneHeading, repo: &str) -> Html {
    let head = match &heading.head {
        LaneHead::Tracking {
            number,
            title,
            tickets_closed,
            tickets,
        } => html::titled(*number, title, repo).then(&html::escape(&format!(
            " — {tickets_closed}/{tickets} tickets closed"
        ))),
        LaneHead::Item { number, title } => html::titled(*number, title, repo),
    };
    match heading.finished {
        Some(at) => head
            .then(&html::escape(" — finished "))
            .then(&html::time(at)),
        None => head,
    }
}

/// The tone `status` shows in (`docs/specs/web/design_system.md`, Status).
pub(super) fn tone(status: CardStatus) -> Tone {
    match status {
        CardStatus::Failed => Tone::Failure,
        CardStatus::WaitingForAPerson => Tone::Pending,
        CardStatus::InFlight => Tone::InProgress,
        CardStatus::PausedOrQueued => Tone::Paused,
    }
}

/// `line` in the UI's own words.
fn worded(line: &CardLine) -> Phrase {
    match line {
        CardLine::Stalled => "stalled".into(),
        CardLine::Asked(people) if people.is_empty() => "people asked".into(),
        CardLine::Asked(people) => Phrase::from("asked ").then(Phrase::list(
            people.iter().map(|person| Phrase::from(person.to_string())),
            "and",
        )),
        CardLine::HandedBack => "handed back".into(),
        CardLine::DecisionNeeded => "decision needed".into(),
        CardLine::BlockedBy(issue) => Phrase::from("blocked by ").item(*issue),
        CardLine::AssignedTo(assignee) => format!("assigned to {assignee}").into(),
        CardLine::Unassigned => "unassigned".into(),
        CardLine::Paused(Hold::UsageLimit) => "paused for the usage limit".into(),
        CardLine::Paused(Hold::Drain) => "paused for the drain".into(),
        CardLine::Queued => "queued".into(),
        CardLine::Running(running) => running_worded(*running),
        CardLine::Review {
            pull_request,
            author,
            stops,
        } => {
            let phrase = Phrase::default().item(*pull_request);
            let phrase = match author {
                Some(author) => phrase.text(format!(" by {author}")),
                None => phrase,
            };
            match stops {
                Some(stop) => phrase.text(" · ").then(stop_worded(stop)),
                None => phrase,
            }
        }
        CardLine::MergeStop(stop) => stop_worded(stop),
        CardLine::Ended { ended, at } => ended_worded(*ended).text(" · ").time(*at),
    }
}

/// A run's stage, as a noun, and how long it has run, kept up as it runs.
fn running_worded(running: Running) -> Phrase {
    let stage = match running.stage {
        WorkKind::Triage => "triage",
        WorkKind::Design => "design",
        WorkKind::Implement => "implementation",
        WorkKind::Rework => "rework",
        WorkKind::FixCi => "CI fix",
        WorkKind::Update => "update",
    };
    Phrase::from(format!("{stage} · ")).then(ticking(running.elapsed))
}

/// How long `elapsed` has gone on, kept up as it goes on.
fn ticking(elapsed: Elapsed) -> Phrase {
    Phrase::default().duration(elapsed.seconds, Some(elapsed.since))
}

fn stop_worded(stop: &Stop) -> Phrase {
    match stop {
        Stop::Stalled => "stalled".into(),
        Stop::Running(running) => running_worded(*running),
        Stop::ChecksFailed => "checks failed".into(),
        Stop::ChecksRunning => "checks running".into(),
        Stop::Conflicts => "conflicts".into(),
        Stop::Behind { branch } => format!("behind {branch}").into(),
        Stop::Draft => "draft".into(),
        Stop::Review(ReviewDecision::ChangesRequested) => "changes requested".into(),
        Stop::Review(ReviewDecision::ReviewRequired) => "review required".into(),
        Stop::Review(ReviewDecision::Approved) => "approved".into(),
    }
}

fn ended_worded(ended: Ended) -> Phrase {
    match ended {
        Ended::ClosedIn(pull) => Phrase::from("closed in ").item(pull),
        Ended::Closed(None) => "closed".into(),
        Ended::Closed(Some(IssueCloseReason::Completed)) => "closed as completed".into(),
        Ended::Closed(Some(IssueCloseReason::NotPlanned)) => "closed as not planned".into(),
        Ended::Closed(Some(IssueCloseReason::Duplicate)) => "closed as a duplicate".into(),
        Ended::Merged => "merged".into(),
        Ended::ClosedUnmerged => "closed without merging".into(),
        Ended::NothingToBuild { .. } => "merged · nothing to build".into(),
    }
}
