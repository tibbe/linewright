//! The server page (`docs/specs/operation.md`, Server): the
//! facts about Linewright itself, in the page's own words.

use std::num::NonZeroU64;

use askama::Template;
use askama_web::WebTemplate;
use axum::response::{IntoResponse, Response};

use super::caller::Reader;
use super::facts::{Fact, Piece};
use super::html::{self, Html};
use super::names::Names;
use super::run_page::{Row, command_row, command_text, entry_row, output_html, status_html};
use super::shell::{self, Area, Frame, Hold, Navigation, Place};
use super::{Page, events, render};
use crate::access::{Permit, Read};
use crate::api::Sources;
use crate::environment::shown::{self, Account, BuildShown, EnvironmentShown, EnvironmentState};
use crate::phrase::{Phrase, Status};
use crate::status::{self, FailedPolls, Linewright, LinewrightFact, PollsShown, TrialsInFlight};

#[derive(Template, WebTemplate)]
#[template(path = "server.html")]
struct ServerPage {
    page: Page,
    frame: Frame,
    facts: Vec<Fact>,
}

#[derive(Template)]
#[template(path = "server.html", block = "main")]
struct ServerRegion<'a> {
    facts: &'a [Fact],
}

/// The facts about Linewright itself as the page words them, each on a
/// line, past times standing alone, as the page's script shows them
/// relative. A hold's or a problem's value shows in its status.
/// The environment's fact ends with `builds`, each a line.
fn facts(linewright: &Linewright, mut builds: Vec<Html>) -> Vec<Fact<Phrase>> {
    let fact = |label, value| Fact {
        label,
        lines: vec![vec![Piece::Text(value)]],
    };
    let held = |hold: Hold, label, words| Fact {
        label,
        lines: vec![vec![Piece::Status {
            status: hold.status(),
            words,
        }]],
    };
    linewright
        .facts()
        .into_iter()
        .flat_map(|shown| match shown {
            LinewrightFact::ServingSince(since) => {
                vec![fact("Started", Phrase::default().time(since))]
            }
            LinewrightFact::Polls(polls) => vec![fact("Polls", self::polls(polls))],
            LinewrightFact::FailedPolls(failed) => vec![held(
                Hold::PollsFailed(failed.clone()),
                "Failed polls",
                failed_polls(failed),
            )],
            LinewrightFact::UsageLimit { resets_at } => vec![held(
                Hold::UsageLimit { resets_at },
                "Usage limit",
                Phrase::from("in force until ").time(resets_at),
            )],
            LinewrightFact::UnusableFile(file) => vec![held(
                Hold::UnusableFile(file.clone()),
                "Repository file",
                status::file_error(file),
            )],
            LinewrightFact::Drain(drain) => {
                let value = Phrase::from("started ").time(drain.since);
                let value = match drain.deadline {
                    Some(deadline) => value.text(", deadline ").time(deadline),
                    None => value,
                };
                vec![held(Hold::Drain(drain), "Drain", value)]
            }
            LinewrightFact::Environment(environment) => {
                vec![self::environment(environment, std::mem::take(&mut builds))]
            }
            LinewrightFact::RunsInFlight { runs, max_runs } => vec![fact(
                "Runs in flight",
                format!("{runs} of {max_runs}").into(),
            )],
            LinewrightFact::TrialsInFlight(TrialsInFlight {
                in_flight,
                max_trials,
            }) => vec![fact(
                "Trials and scorers of benchmark runs in flight",
                format!("{in_flight} of {max_trials}").into(),
            )],
            LinewrightFact::Webhooks(received) => vec![
                fact(
                    "Last webhook delivery accepted",
                    match received.last_accepted {
                        Some(at) => Phrase::default().time(at),
                        None => "none yet".into(),
                    },
                ),
                fact(
                    "Webhook deliveries refused",
                    received.refused.to_string().into(),
                ),
            ],
        })
        .collect()
}

/// What the environment was built from, a build in progress, a latest
/// build that failed and then `builds`, each on a line. The line that names
/// the environment's state shows in its status.
fn environment(environment: &EnvironmentShown, builds: Vec<Html>) -> Fact<Phrase> {
    let (status, stating) = match environment.state() {
        EnvironmentState::Building => (Status::InFlight, EnvironmentLine::Building),
        EnvironmentState::LatestFailed => (Status::Failed, EnvironmentLine::LatestFailed),
        EnvironmentState::NoneYet => (Status::Paused, EnvironmentLine::BuiltOrNone),
        EnvironmentState::Built => (Status::Done, EnvironmentLine::BuiltOrNone),
    };
    let line_of = |line, words| {
        if line == stating {
            Piece::Status { status, words }
        } else {
            Piece::Text(words)
        }
    };
    let built = line_of(
        EnvironmentLine::BuiltOrNone,
        match &environment.built {
            Some(built) => Phrase::from("built from ")
                .code(shown::short(&built.commit))
                .text(" ")
                .time(built.at),
            None => "none yet".into(),
        },
    );
    let building = environment.building_since.map(|since| {
        line_of(
            EnvironmentLine::Building,
            Phrase::from("building since ").time(since),
        )
    });
    let failed = environment.latest_failed.map(|at| {
        line_of(
            EnvironmentLine::LatestFailed,
            Phrase::from("the latest build failed ").time(at),
        )
    });
    Fact {
        label: "Environment",
        lines: [Some(built), building, failed]
            .into_iter()
            .flatten()
            .chain(builds.into_iter().map(Piece::Markup))
            .map(|piece| vec![piece])
            .collect(),
    }
}

/// A line of the Environment fact that can name the environment's state,
/// and then shows in its status.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvironmentLine {
    /// What it was built from, or that there is none yet.
    BuiltOrNone,
    Building,
    LatestFailed,
}

/// The last poll that succeeded and the next.
fn polls(polls: Option<PollsShown>) -> Phrase {
    let last = match polls.and_then(|polls| polls.last) {
        Some(last) => Phrase::from("last ").time(last),
        None => "none yet".into(),
    };
    match polls {
        Some(polls) => last.text(", next ").time(polls.next),
        None => last.text(", next due now"),
    }
}

/// How many polls failed, when the first and the last did, and the last's
/// error.
fn failed_polls(failed: &FailedPolls) -> Phrase {
    let phrase = if failed.count == NonZeroU64::MIN {
        Phrase::from("1, ").time(failed.last_at)
    } else {
        Phrase::from(format!("{}, the first ", failed.count))
            .time(failed.since)
            .text(", the last ")
            .time(failed.last_at)
    };
    phrase.text(": ").text(&failed.last_error)
}

/// The frame and the facts, from one answer of `status`.
async fn shown(permit: &Permit<Read>, sources: &Sources) -> (Frame, Vec<Fact>) {
    let read = shell::Read::of(permit, sources).await;
    let names = read.names(&sources.work_dir.repo);
    let builds = read
        .builds
        .iter()
        .map(|build| build_html(build, &names))
        .collect();
    let facts = facts(&read.linewright, builds)
        .into_iter()
        .map(|fact| fact.map(|phrase| html::phrase(&phrase, &names)))
        .collect();
    (Frame::of_read(permit, sources, &read), facts)
}

/// A build as a transcript's maintenance command shows, on the same line
/// with its duration at the right, opening the same way, under a line
/// saying when it started and from what, and with the output so far of a
/// build in progress.
fn build_html(build: &BuildShown, names: &Names) -> Html {
    let heading = html::phrase(&shown::heading(build), names);
    let record = &build.record;
    let command = &record.line.command;
    let line = match shown::account(build) {
        Account::Failed(account) if record.line.ended.is_none() => entry_row(&Row {
            id: None,
            text: command_text("setup", command, &status_html(Status::Failed, &account)),
            lasted: None,
            fold: None,
        }),
        Account::Running | Account::Ended(_) | Account::Failed(_) => {
            let (course, lasted) = shown::course(build);
            command_row(None, "setup", command, course, Some(lasted), false)
        }
    };
    let so_far = build
        .so_far
        .as_ref()
        .map_or_else(String::new, |so_far| output_html(&so_far.output));
    html::built(format!(
        r#"<div class="build font-mono text-sm" id="build-{}"><div class="text-base-content/60">{heading}</div>{line}<div class="ml-6">{so_far}</div></div>"#,
        build.build
    ))
}

fn page(navigation: Navigation) -> Page {
    Page::new(
        Place::Navigation(Area::Server),
        "/events/server".into(),
        navigation,
    )
}

pub(super) async fn server_page(
    Reader { permit, sources }: Reader,
    navigation: Navigation,
) -> Response {
    let (frame, facts) = shown(&permit, &sources).await;
    ServerPage {
        page: page(navigation),
        frame,
        facts,
    }
    .into_response()
}

/// The server page's regions: the shell's, and the facts.
pub(super) async fn server_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    navigation: Navigation,
) -> Response {
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        async move {
            let (frame, facts) = shown(&permit, &sources).await;
            let mut regions = shell::regions(&frame, &page(navigation));
            regions.push(("server", render(&ServerRegion { facts: &facts })));
            regions
        }
    };
    events(regions, watch)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use crate::repo_files::FileError;
    use crate::status::DrainShown;
    use crate::webhook::Received;

    fn at(time: &str) -> crate::domain::Timestamp {
        time.parse().unwrap()
    }

    /// Each fact as label, value as the commands print its times, and the
    /// status it shows in, for a fact of one line that is all text or one
    /// status.
    fn worded(linewright: &Linewright) -> Vec<(&'static str, String, Option<Status>)> {
        facts(linewright, vec![])
            .into_iter()
            .map(|fact| {
                let (value, status) = match &fact.lines[..] {
                    [line] => match &line[..] {
                        [Piece::Text(text)] => (text.to_string(), None),
                        [Piece::Status { status, words }] => (words.to_string(), Some(*status)),
                        _ => panic!("a line of more than one piece"),
                    },
                    _ => panic!("a fact of more than one line"),
                };
                (fact.label, value, status)
            })
            .collect()
    }

    fn serving() -> Linewright {
        Linewright {
            serving_since: at("2026-10-01T12:00:00Z"),
            polls: None,
            failed_polls: None,
            usage_limit_resets_at: None,
            unusable_files: vec![],
            drain: None,
            environment: None,
            runs_in_flight: 1,
            max_runs: 4,
            trials_in_flight: None,
            webhooks: None,
        }
    }

    /// `docs/specs/operation.md`, Server.
    #[test_log::test]
    fn before_the_first_poll_ends_the_facts_always_shown_say_the_next_is_due() {
        assert_eq!(
            worded(&serving()),
            [
                ("Started", "2026-10-01 12:00:00 UTC".into(), None),
                ("Polls", "none yet, next due now".into(), None),
                ("Runs in flight", "1 of 4".into(), None),
            ]
        );
    }

    #[test_log::test]
    fn every_fact_shows_in_the_tables_order_while_it_holds() {
        let linewright = Linewright {
            polls: Some(PollsShown {
                last: Some(at("2026-10-01T12:30:00Z")),
                next: at("2026-10-01T12:35:00Z"),
            }),
            failed_polls: Some(FailedPolls {
                count: NonZeroU64::new(3).unwrap(),
                since: at("2026-10-01T12:31:00Z"),
                last_at: at("2026-10-01T12:33:00Z"),
                last_error: "taking a snapshot: GitHub is down".into(),
            }),
            usage_limit_resets_at: Some(at("2026-10-01T15:00:00Z")),
            unusable_files: vec![FileError {
                path: ".linewright/config.toml".into(),
                error: "the setting `max-runs` is the operator's".into(),
            }],
            drain: Some(DrainShown {
                since: at("2026-10-01T12:32:00Z"),
                deadline: Some(at("2026-10-01T14:32:00Z")),
            }),
            webhooks: Some(Received {
                last_accepted: None,
                refused: 2,
                last_refused: Some(at("2026-10-01T12:20:00Z")),
            }),
            trials_in_flight: Some(TrialsInFlight {
                in_flight: 2,
                max_trials: 3,
            }),
            ..serving()
        };

        assert_eq!(
            worded(&linewright),
            [
                ("Started", "2026-10-01 12:00:00 UTC".into(), None),
                (
                    "Polls",
                    "last 2026-10-01 12:30:00 UTC, next 2026-10-01 12:35:00 UTC".into(),
                    None
                ),
                (
                    "Failed polls",
                    "3, the first 2026-10-01 12:31:00 UTC, the last 2026-10-01 12:33:00 UTC: \
                     taking a snapshot: GitHub is down"
                        .into(),
                    Some(Status::Failed)
                ),
                (
                    "Usage limit",
                    "in force until 2026-10-01 15:00:00 UTC".into(),
                    Some(Status::Paused)
                ),
                (
                    "Repository file",
                    "`.linewright/config.toml`: the setting `max-runs` is the operator's".into(),
                    Some(Status::Failed)
                ),
                (
                    "Drain",
                    "started 2026-10-01 12:32:00 UTC, deadline 2026-10-01 14:32:00 UTC".into(),
                    Some(Status::Paused)
                ),
                ("Runs in flight", "1 of 4".into(), None),
                (
                    "Trials and scorers of benchmark runs in flight",
                    "2 of 3".into(),
                    None
                ),
                ("Last webhook delivery accepted", "none yet".into(), None),
                ("Webhook deliveries refused", "2".into(), None),
            ]
        );
    }

    /// The Environment fact of `environment`, each line as the commands
    /// print its times, with the status it shows in, for lines of one piece.
    #[track_caller]
    fn environment_lines(environment: &EnvironmentShown) -> Vec<(String, Option<Status>)> {
        self::environment(environment, vec![])
            .lines
            .into_iter()
            .map(|line| match &line[..] {
                [Piece::Text(text)] => (text.to_string(), None),
                [Piece::Status { status, words }] => (words.to_string(), Some(*status)),
                _ => panic!("a line of other than one text or status"),
            })
            .collect()
    }

    fn built() -> shown::BuiltShown {
        shown::BuiltShown {
            commit: "0123456789abcdef".into(),
            at: at("2026-10-01T11:00:00Z"),
        }
    }

    /// `docs/specs/operation.md`, Server, The environment's status: a build
    /// running comes first, even after a failed latest build.
    #[test_log::test]
    fn the_environment_shows_in_flight_on_its_building_line_while_a_build_runs() {
        let environment = EnvironmentShown {
            built: None,
            building_since: Some(at("2026-10-01T12:10:00Z")),
            latest_failed: Some(at("2026-10-01T12:05:00Z")),
        };

        assert_eq!(
            environment_lines(&environment),
            [
                ("none yet".into(), None),
                (
                    "building since 2026-10-01 12:10:00 UTC".into(),
                    Some(Status::InFlight)
                ),
                (
                    "the latest build failed 2026-10-01 12:05:00 UTC".into(),
                    None
                ),
            ]
        );
    }

    #[test_log::test]
    fn the_environment_shows_failed_on_its_failed_line_after_a_failed_latest_build() {
        let environment = EnvironmentShown {
            built: Some(built()),
            building_since: None,
            latest_failed: Some(at("2026-10-01T12:05:00Z")),
        };

        assert_eq!(
            environment_lines(&environment),
            [
                (
                    "built from `0123456789ab` 2026-10-01 11:00:00 UTC".into(),
                    None
                ),
                (
                    "the latest build failed 2026-10-01 12:05:00 UTC".into(),
                    Some(Status::Failed)
                ),
            ]
        );
    }

    #[test_log::test]
    fn the_environment_shows_paused_while_there_is_none_yet() {
        let environment = EnvironmentShown {
            built: None,
            building_since: None,
            latest_failed: None,
        };

        assert_eq!(
            environment_lines(&environment),
            [("none yet".into(), Some(Status::Paused))]
        );
    }

    #[test_log::test]
    fn the_environment_shows_done_on_what_it_was_built_from_otherwise() {
        let environment = EnvironmentShown {
            built: Some(built()),
            building_since: None,
            latest_failed: None,
        };

        assert_eq!(
            environment_lines(&environment),
            [(
                "built from `0123456789ab` 2026-10-01 11:00:00 UTC".into(),
                Some(Status::Done)
            )]
        );
    }

    #[test_log::test]
    fn a_single_failed_poll_shows_when_it_failed_and_its_error() {
        let linewright = Linewright {
            failed_polls: Some(FailedPolls {
                count: NonZeroU64::MIN,
                since: at("2026-10-01T12:31:00Z"),
                last_at: at("2026-10-01T12:31:00Z"),
                last_error: "taking a snapshot: bad credentials".into(),
            }),
            ..serving()
        };

        assert_eq!(
            worded(&linewright)[2],
            (
                "Failed polls",
                "1, 2026-10-01 12:31:00 UTC: taking a snapshot: bad credentials".into(),
                Some(Status::Failed)
            )
        );
    }
}
