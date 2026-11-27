//! A card's words (`docs/specs/board.md`, Cards): the line under its
//! title, as `card_line` gives it.

use linewright_core::board::{CardLine, Elapsed, Ended, Held, Running, Stop, card_line};
use linewright_core::domain::{IssueCloseReason, ItemKind, Login, Number, Timestamp};
use linewright_core::holds::Hold;
use linewright_core::phrase::Segment;
use linewright_core::reconcile::WorkKind;
use linewright_core::repo_files::FileError;
use linewright_core::testing::{at, card, logins};
use rstest::rstest;

/// The words under a card whose line is `line` and which entered its
/// column at `entered`.
fn words(line: Option<CardLine>, entered: Option<Timestamp>) -> Option<String> {
    card_line(&card(line, entered)).map(|words| words.to_string())
}

/// An implementation run 12 minutes in.
fn implementing() -> Running {
    Running {
        stage: WorkKind::Implement,
        elapsed: Elapsed {
            since: at("2026-10-01T11:48:00Z"),
            seconds: 12 * 60,
        },
    }
}

/// The hold of a `.linewright/config.toml` that cannot be used.
fn broken_config() -> Hold {
    Hold::File(FileError {
        path: ".linewright/config.toml".into(),
        error: "expected a table".into(),
    })
}

/// The line of an item in review on #42, opened by `author` where not by
/// Linewright, which `stops` stops merging.
fn review(author: Option<&str>, stops: Option<Stop>) -> CardLine {
    CardLine::Review {
        pull_request: Number(42),
        author: author.map(Login::new),
        stops,
    }
}

/// `docs/specs/board.md`, Cards: each line in its own words, as the table
/// of each column and the table of what stops a pull request merging say.
#[rstest]
#[case::stalled(CardLine::Stalled, "stalled")]
#[case::asked_no_one(CardLine::Asked(vec![]), "people asked")]
#[case::asked_one(CardLine::Asked(logins(&["alice"])), "asked alice")]
#[case::asked_two(CardLine::Asked(logins(&["alice", "bob"])), "asked alice and bob")]
#[case::asked_three(
    CardLine::Asked(logins(&["alice", "bob", "carol"])),
    "asked alice, bob and carol"
)]
#[case::handed_back(CardLine::HandedBack, "handed back")]
#[case::decision_needed(CardLine::DecisionNeeded, "decision needed")]
#[case::blocked_by(CardLine::BlockedBy(Number(12)), "blocked by #12")]
#[case::assigned_to(CardLine::AssignedTo(Login::new("alice")), "assigned to alice")]
#[case::unassigned(CardLine::Unassigned, "unassigned")]
#[case::not_marked(CardLine::NotMarked, "not marked for Linewright")]
#[case::paused_by_a_drain(
    CardLine::Held(Held::Paused(Hold::Drain)),
    "paused until `serve` restarts"
)]
#[case::paused_by_the_usage_limit(
    CardLine::Held(Held::Paused(Hold::UsageLimit { resets_at: Some(at("2026-10-01T13:00:00Z")) })),
    "paused until no session waits for it"
)]
#[case::paused_by_the_environment(
    CardLine::Held(Held::Paused(Hold::Environment)),
    "paused until the environment is built"
)]
#[case::paused_by_a_file(
    CardLine::Held(Held::Paused(broken_config())),
    "paused until the default branch fixes `.linewright/config.toml`"
)]
#[case::queued(CardLine::Held(Held::Queued), "queued")]
#[case::running(CardLine::Running(implementing()), "implementation · 12m")]
#[case::review_of_linewrights_pull_request(review(None, None), "#42")]
#[case::review_of_someone_elses_pull_request(review(Some("alice"), None), "#42 by alice")]
#[case::review_stopped(review(None, Some(Stop::ChecksFailed)), "#42 · checks failed")]
#[case::review_of_someone_elses_pull_request_stopped(
    review(Some("alice"), Some(Stop::Approved)),
    "#42 by alice · approved"
)]
#[case::stop_stalled(CardLine::MergeStop(Stop::Stalled), "stalled")]
#[case::stop_handed_back(CardLine::MergeStop(Stop::HandedBack), "handed back")]
#[case::stop_unassigned(CardLine::MergeStop(Stop::Unassigned), "unassigned")]
#[case::stop_running(
    CardLine::MergeStop(Stop::Running(implementing())),
    "implementation · 12m"
)]
#[case::stop_paused(
    CardLine::MergeStop(Stop::Held(Held::Paused(Hold::Environment))),
    "paused until the environment is built"
)]
#[case::stop_queued(CardLine::MergeStop(Stop::Held(Held::Queued)), "queued")]
#[case::stop_checks_failed(CardLine::MergeStop(Stop::ChecksFailed), "checks failed")]
#[case::stop_checks_running(CardLine::MergeStop(Stop::ChecksRunning), "checks running")]
#[case::stop_conflicts(CardLine::MergeStop(Stop::Conflicts), "conflicts")]
#[case::stop_behind(
    CardLine::MergeStop(Stop::Behind { branch: "main".into() }),
    "behind main"
)]
#[case::stop_draft(CardLine::MergeStop(Stop::Draft), "draft")]
#[case::stop_changes_requested(CardLine::MergeStop(Stop::ChangesRequested), "changes requested")]
#[case::stop_review_required(CardLine::MergeStop(Stop::ReviewRequired), "review required")]
#[case::stop_approved(CardLine::MergeStop(Stop::Approved), "approved")]
#[test_log::test]
fn each_line_says_what_the_spec_says(#[case] line: CardLine, #[case] expected: &str) {
    assert_eq!(words(Some(line), None).as_deref(), Some(expected));
}

/// `docs/specs/board.md`, Cards, Done and When: how the item ended, then
/// when it entered Done.
#[rstest]
#[case::closed_in(Ended::ClosedIn(Number(42)), "closed in #42 · 2026-10-01 12:00:00 UTC")]
#[case::closed_for_no_known_reason(Ended::Closed(None), "closed · 2026-10-01 12:00:00 UTC")]
#[case::closed_as_completed(
    Ended::Closed(Some(IssueCloseReason::Completed)),
    "closed as completed · 2026-10-01 12:00:00 UTC"
)]
#[case::closed_as_not_planned(
    Ended::Closed(Some(IssueCloseReason::NotPlanned)),
    "closed as not planned · 2026-10-01 12:00:00 UTC"
)]
#[case::closed_as_a_duplicate(
    Ended::Closed(Some(IssueCloseReason::Duplicate)),
    "closed as a duplicate · 2026-10-01 12:00:00 UTC"
)]
#[case::merged(Ended::Merged, "merged · 2026-10-01 12:00:00 UTC")]
#[case::closed_without_merging(
    Ended::ClosedUnmerged,
    "closed without merging · 2026-10-01 12:00:00 UTC"
)]
#[test_log::test]
fn a_done_line_says_how_it_ended_and_when(#[case] ended: Ended, #[case] expected: &str) {
    let line = CardLine::Ended {
        ended,
        at: at("2026-10-01T12:00:00Z"),
    };
    assert_eq!(words(Some(line), None).as_deref(), Some(expected));
}

/// `docs/specs/board.md`, Cards, When: in Triage, Needs a person and Ready
/// the line ends with when the item entered its column; with nothing else
/// to say it is the time alone, and with no time either, nothing.
#[rstest]
#[case::a_line_and_the_time(
    Some(CardLine::Stalled),
    Some(at("2026-10-01T12:00:00Z")),
    Some("stalled · 2026-10-01 12:00:00 UTC")
)]
#[case::the_time_alone(
    None,
    Some(at("2026-10-01T12:00:00Z")),
    Some("2026-10-01 12:00:00 UTC")
)]
#[case::the_line_alone(Some(CardLine::Stalled), None, Some("stalled"))]
#[case::nothing(None, None, None)]
#[test_log::test]
fn a_line_ends_with_when_the_card_entered_its_column(
    #[case] line: Option<CardLine>,
    #[case] entered: Option<Timestamp>,
    #[case] expected: Option<&str>,
) {
    assert_eq!(words(line, entered).as_deref(), expected);
}

/// `docs/specs/board.md`, Cards, Its line: an item a line names is an issue
/// or a pull request, which a pull request's icon before its number tells.
#[rstest]
#[case::blocked_by_an_issue(CardLine::BlockedBy(Number(12)), Number(12), ItemKind::Issue)]
#[case::in_review_on_a_pull_request(review(None, None), Number(42), ItemKind::PullRequest)]
#[case::closed_in_a_pull_request(
    CardLine::Ended { ended: Ended::ClosedIn(Number(42)), at: at("2026-10-01T12:00:00Z") },
    Number(42),
    ItemKind::PullRequest
)]
#[test_log::test]
fn a_line_names_an_item_as_an_issue_or_a_pull_request(
    #[case] line: CardLine,
    #[case] number: Number,
    #[case] kind: ItemKind,
) {
    let phrase = card_line(&card(Some(line), None)).unwrap();
    let items: Vec<&Segment> = phrase
        .segments()
        .iter()
        .filter(|segment| matches!(segment, Segment::Item { .. }))
        .collect();
    assert_eq!(items, [&Segment::Item { number, kind }]);
}
