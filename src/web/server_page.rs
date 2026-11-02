//! The server page (`docs/specs/design_system.md`, The server page): the
//! facts about Linewright itself, in the page's own words.

use std::num::NonZeroU64;

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::facts::{Fact, Piece};
use super::html::{self, Html};
use super::names::Names;
use super::run_page::{command_html, command_line_html, output_html, status_html};
use super::{Frame, Hold, Page, Read, events, frame_regions, render};
use crate::api::{self, Sources};
use crate::environment::shown::{self, Account, BuildShown, EnvironmentShown};
use crate::phrase::{Phrase, Tone};
use crate::status::{self, FailedPolls, Linewright, LinewrightFact, PollsShown};

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
/// relative. A hold's or a problem's value shows in its tone.
/// The environment's fact ends with `builds`, each a line.
fn facts(linewright: &Linewright, mut builds: Vec<Html>) -> Vec<Fact<Phrase>> {
    let fact = |label, value| Fact {
        label,
        lines: vec![vec![Piece::Text(value)]],
    };
    let held = |hold: Hold, label, words| Fact {
        label,
        lines: vec![vec![Piece::Status {
            tone: hold.tone(),
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
            LinewrightFact::RunsInFlight { runs, max_runs } => {
                vec![fact(
                    "Runs in flight",
                    format!("{runs} of {max_runs}").into(),
                )]
            }
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
/// build that failed and then `builds`, each on a line.
fn environment(environment: &EnvironmentShown, builds: Vec<Html>) -> Fact<Phrase> {
    let built = match &environment.built {
        Some(built) => Piece::Text(
            Phrase::from("built from ")
                .code(shown::short(&built.commit))
                .text(" ")
                .time(built.at),
        ),
        None => Piece::Status {
            tone: Tone::Pending,
            words: "none yet".into(),
        },
    };
    let building = environment.building_since.map(|since| Piece::Status {
        tone: Tone::InProgress,
        words: Phrase::from("building since ").time(since),
    });
    let failed = environment.latest_failed.map(|at| Piece::Status {
        tone: Tone::Failure,
        words: Phrase::from("the latest build failed ").time(at),
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
fn shown(sources: &Sources) -> (Frame, Vec<Fact>) {
    let read = Read::of(sources);
    let names = read.names(&sources.work_dir.repo);
    let builds = read
        .linewright
        .environment
        .is_some()
        .then(|| api::environment_or_none(sources))
        .flatten()
        .and_then(|answer| answer.environment)
        .map_or_else(Vec::new, |environment| environment.builds)
        .iter()
        .map(|build| build_html(build, &names))
        .collect();
    let facts = facts(&read.linewright, builds)
        .into_iter()
        .map(|fact| fact.map(|phrase| html::phrase(&phrase, &names)))
        .collect();
    (Frame::of_read(sources, &read), facts)
}

/// A build as a transcript's maintenance command shows, opening the same
/// way, under a line saying when it started and from what, and with the
/// output so far of a build in progress.
fn build_html(build: &BuildShown, names: &Names) -> Html {
    let heading = html::phrase(&shown::heading(build), names);
    let command = &build.record.line.command;
    let line = match shown::account(build) {
        Account::Failed(account) if build.record.line.ended.is_none() => {
            command_line_html("", "setup", command, &status_html(Tone::Failure, &account))
        }
        Account::Running | Account::Ended(_) | Account::Failed(_) => command_html(
            "",
            "setup",
            command,
            build.record.line.ended.as_ref(),
            false,
        ),
    };
    let so_far = build
        .so_far
        .as_ref()
        .map_or_else(String::new, |so_far| output_html(&so_far.output));
    html::built(format!(
        r#"<div class="build font-mono text-sm" id="build-{}"><div class="text-muted">{heading}</div>{line}{so_far}</div>"#,
        build.build
    ))
}

pub(super) async fn server_page(State(sources): State<Sources>) -> Response {
    let (frame, facts) = shown(&sources);
    ServerPage {
        page: Page::new(
            format!("Server · {}", sources.work_dir.repo),
            "/events/server".into(),
        ),
        frame,
        facts,
    }
    .into_response()
}

/// The server page's regions: the frame's, and the facts.
pub(super) async fn server_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
) -> Response {
    let regions = move || {
        let (frame, facts) = shown(&sources);
        let mut regions = frame_regions(&frame);
        regions.push(("server", render(&ServerRegion { facts: &facts })));
        regions
    };
    events(regions, stopping)
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
    /// tone it shows in, for a fact of one line that is all text or one
    /// status.
    fn worded(linewright: &Linewright) -> Vec<(&'static str, String, Option<Tone>)> {
        facts(linewright, vec![])
            .into_iter()
            .map(|fact| {
                let (value, tone) = match &fact.lines[..] {
                    [line] => match &line[..] {
                        [Piece::Text(text)] => (text.to_string(), None),
                        [Piece::Status { tone, words }] => (words.to_string(), Some(*tone)),
                        _ => panic!("a line of more than one piece"),
                    },
                    _ => panic!("a fact of more than one line"),
                };
                (fact.label, value, tone)
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
            webhooks: None,
        }
    }

    /// `docs/specs/design_system.md`, The server page.
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
                    Some(Tone::Failure)
                ),
                (
                    "Usage limit",
                    "in force until 2026-10-01 15:00:00 UTC".into(),
                    Some(Tone::Cancelled)
                ),
                (
                    "Repository file",
                    "`.linewright/config.toml`: the setting `max-runs` is the operator's".into(),
                    Some(Tone::Failure)
                ),
                (
                    "Drain",
                    "started 2026-10-01 12:32:00 UTC, deadline 2026-10-01 14:32:00 UTC".into(),
                    Some(Tone::InProgress)
                ),
                ("Runs in flight", "1 of 4".into(), None),
                ("Last webhook delivery accepted", "none yet".into(), None),
                ("Webhook deliveries refused", "2".into(), None),
            ]
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
                Some(Tone::Failure)
            )
        );
    }
}
