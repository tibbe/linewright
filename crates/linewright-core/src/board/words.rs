//! A card's words (`docs/specs/board.md`, Cards): the line under its
//! title, as the screens and the tools show it.

use super::{Card, CardLine, Elapsed, Held, Running, Stop};
use crate::holds::Hold;
use crate::phrase::Phrase;
use crate::reconcile::WorkKind;

/// The line under `card`'s title, ending with when it entered its column
/// where it says so; `None` for the title alone.
pub fn card_line(card: &Card) -> Option<Phrase> {
    let line = card.line.as_ref().map(worded);
    let entered = card.entered.map(|at| Phrase::default().time(at));
    match (line, entered) {
        (Some(line), Some(entered)) => Some(line.text(" · ").then(entered)),
        (line, entered) => line.or(entered),
    }
}

/// What ends `hold` (`docs/specs/runs.md`, Holds, Ends when).
fn ends(hold: &Hold) -> Phrase {
    match hold {
        Hold::Drain => Phrase::default().code("serve").text(" restarts"),
        Hold::UsageLimit { .. } => "the usage limit resets".into(),
        Hold::Environment => "the environment is built".into(),
        Hold::File(file) => Phrase::from("the default branch fixes ").code(file.path.clone()),
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
        CardLine::BlockedBy(issue) => Phrase::from("blocked by ").issue(*issue),
        CardLine::AssignedTo(assignee) => format!("assigned to {assignee}").into(),
        CardLine::Unassigned => "unassigned".into(),
        CardLine::NotMarked => "not marked for Linewright".into(),
        CardLine::Held(held) => held_worded(held),
        CardLine::Running(running) => running_worded(*running),
        CardLine::Review {
            pull_request,
            author,
            stops,
        } => {
            let phrase = Phrase::default().pull_request(*pull_request);
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
        CardLine::Ended { ended, at } => ended.worded().text(" · ").time(*at),
    }
}

/// `held` in the UI's own words.
fn held_worded(held: &Held) -> Phrase {
    match held {
        Held::Paused(hold) => Phrase::from("paused until ").then(ends(hold)),
        Held::Queued => "queued".into(),
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
        Stop::HandedBack => "handed back".into(),
        Stop::Unassigned => "unassigned".into(),
        Stop::Running(running) => running_worded(*running),
        Stop::Held(held) => held_worded(held),
        Stop::ChecksFailed => "checks failed".into(),
        Stop::ChecksRunning => "checks running".into(),
        Stop::Conflicts => "conflicts".into(),
        Stop::Behind { branch } => format!("behind {branch}").into(),
        Stop::Draft => "draft".into(),
        Stop::ChangesRequested => "changes requested".into(),
        Stop::ReviewRequired => "review required".into(),
        Stop::Approved => "approved".into(),
    }
}
