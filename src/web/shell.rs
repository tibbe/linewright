//! What every screen has around it (`docs/specs/shell.md`): the navigation,
//! the notices and the breadcrumbs, and each screen's title
//! (`docs/specs/design_system.md`, Titles).

use std::sync::Arc;

use askama::Template;

use super::automations_page::{Automations, LastRun};
use super::html::{self, Html};
use super::names::Names;
use super::{Page, render, run_path};
use crate::api::{self, Sources};
use crate::domain::{Repo, Snapshot, Timestamp};
use crate::environment::shown::{Attention, BuildShown, EnvironmentShown};
use crate::phrase::{Phrase, Status};
use crate::repo_files::FileError;
use crate::runs::{Outcome, RunId};
use crate::status::{self, DrainShown, FailedPolls, Linewright, UnparsedAutomation};

/// The screen of the navigation that an area starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Area {
    Board,
    Runs,
    Automations,
    Suites,
    Server,
}

impl Area {
    pub(super) const ALL: [Area; 5] = [
        Area::Board,
        Area::Runs,
        Area::Automations,
        Area::Suites,
        Area::Server,
    ];

    /// Its screen's name in the navigation.
    pub(super) fn name(self) -> &'static str {
        match self {
            Area::Board => "Board",
            Area::Runs => "Runs",
            Area::Automations => "Automations",
            Area::Suites => "Suites",
            Area::Server => "Server",
        }
    }

    /// The path of its screen.
    pub(super) fn href(self) -> &'static str {
        match self {
            Area::Board => "/",
            Area::Runs => "/runs",
            Area::Automations => "/automations",
            Area::Suites => "/suites",
            Area::Server => "/server",
        }
    }
}

/// Where a screen is (`docs/specs/shell.md`, Breadcrumbs): the screen of the
/// navigation for an area, or a screen within an area, under a crumb for
/// each thing in it that the screen belongs to.
pub(super) enum Trail {
    Area(Area),
    Within {
        area: Area,
        crumbs: Vec<Crumb>,
        /// The screen's own name.
        here: String,
    },
}

/// A crumb that links to its screen.
pub(super) struct Crumb {
    name: String,
    href: String,
}

/// The crumbs of a screen within an area, up to the screen itself.
pub(super) struct Within {
    area: Area,
    crumbs: Vec<Crumb>,
}

/// A crumb as the breadcrumbs draw it: the screen's own has no link.
pub(super) struct CrumbShown<'a> {
    pub(super) name: &'a str,
    pub(super) href: Option<&'a str>,
}

impl Trail {
    /// The screen of the navigation for `area`.
    pub(super) fn of(area: Area) -> Trail {
        Trail::Area(area)
    }

    /// The crumbs of a screen within `area`, starting from its screen.
    pub(super) fn within(area: Area) -> Within {
        Within {
            area,
            crumbs: vec![],
        }
    }

    /// The crumbs in order, the screen's own last.
    pub(super) fn shown(&self) -> Vec<CrumbShown<'_>> {
        match self {
            Trail::Area(area) => vec![CrumbShown {
                name: area.name(),
                href: None,
            }],
            Trail::Within { area, crumbs, here } => {
                let area = CrumbShown {
                    name: area.name(),
                    href: Some(area.href()),
                };
                let crumbs = crumbs.iter().map(|crumb| CrumbShown {
                    name: &crumb.name,
                    href: Some(&crumb.href),
                });
                let here = CrumbShown {
                    name: here,
                    href: None,
                };
                std::iter::once(area)
                    .chain(crumbs)
                    .chain(std::iter::once(here))
                    .collect()
            }
        }
    }

    /// The screen's title, such as "Experiment 14 · Linewright"
    /// (`docs/specs/design_system.md`, Titles).
    pub(super) fn title(&self) -> String {
        let name = match self {
            Trail::Area(area) => area.name(),
            Trail::Within { here, .. } => here,
        };
        format!("{name} · Linewright")
    }
}

impl Within {
    /// The crumbs with the crumb `name`, linking to `href`, after the others.
    pub(super) fn under(mut self, name: impl Into<String>, href: impl Into<String>) -> Within {
        self.crumbs.push(Crumb {
            name: name.into(),
            href: href.into(),
        });
        self
    }

    /// The trail of the screen `name`, after the crumbs.
    pub(super) fn here(self, name: impl Into<String>) -> Trail {
        Trail::Within {
            area: self.area,
            crumbs: self.crumbs,
            here: name.into(),
        }
    }
}

/// The frame every page has around its own regions
/// (`docs/specs/shell.md`).
pub(super) struct Frame {
    pub(super) repo: Repo,
    pub(super) holds: Vec<HoldShown>,
}

/// A hold on Linewright's work, shown on every page while it lasts
/// (`docs/specs/shell.md`, Notices).
pub(super) enum Hold {
    UsageLimit {
        resets_at: Timestamp,
    },
    Drain(DrainShown),
    /// A webhook delivery refused, with none accepted since.
    WebhooksRefused {
        last_refused: Timestamp,
    },
    /// Polls failed, with none succeeded since.
    PollsFailed(FailedPolls),
    UnparsedAutomation(UnparsedAutomation),
    /// A file of the repository's Linewright cannot use.
    UnusableFile(FileError),
    /// With a setup command, no environment yet, or a latest build that
    /// failed.
    Environment(Attention),
    /// An automation whose last run failed.
    AutomationFailed {
        name: String,
        run: RunId,
        outcome: Outcome,
    },
}

impl Hold {
    pub(super) fn of(linewright: &Linewright, automations: Option<&Automations>) -> Vec<Hold> {
        let failed_polls = linewright.failed_polls.clone().map(Hold::PollsFailed);
        let usage_limit = linewright
            .usage_limit_resets_at
            .map(|resets_at| Hold::UsageLimit { resets_at });
        let drain = linewright.drain.map(Hold::Drain);
        let refused = linewright
            .webhooks
            .and_then(|received| received.refused_since_accepted())
            .map(|last_refused| Hold::WebhooksRefused { last_refused });
        let unparsed = automations
            .into_iter()
            .flat_map(|automations| &automations.unparsed)
            .map(|file| Hold::UnparsedAutomation(file.clone()));
        let unusable = linewright
            .unusable_files
            .iter()
            .map(|file| Hold::UnusableFile(file.clone()));
        let environment = linewright
            .environment
            .as_ref()
            .and_then(EnvironmentShown::attention)
            .map(Hold::Environment);
        let failed = automations
            .into_iter()
            .flat_map(|automations| &automations.rows)
            .filter_map(|row| match &row.last_run {
                LastRun::Run(run) if run.outcome.failed() => Some(Hold::AutomationFailed {
                    name: row.name.clone(),
                    run: run.id.clone(),
                    outcome: run.outcome,
                }),
                LastRun::Run(_) | LastRun::NoneYet { .. } => None,
            });
        [usage_limit, drain, refused, failed_polls]
            .into_iter()
            .flatten()
            .chain(unparsed)
            .chain(unusable)
            .chain(environment)
            .chain(failed)
            .collect()
    }

    pub(super) fn phrase(&self) -> Phrase {
        match self {
            Hold::PollsFailed(failed) => status::polls_failed_since(failed),
            Hold::UsageLimit { resets_at } => status::usage_limit(*resets_at),
            Hold::Drain(drain) => status::drain_under_way(*drain),
            Hold::WebhooksRefused { last_refused } => Phrase::from("webhook delivery refused: ")
                .time(*last_refused)
                .text(", none accepted since"),
            Hold::UnparsedAutomation(file) => status::does_not_parse(file),
            Hold::UnusableFile(file) => status::unusable_file(file),
            Hold::Environment(Attention::LatestFailed(at)) => {
                Phrase::from("environment: the latest build failed ").time(*at)
            }
            Hold::Environment(Attention::FirstBuilding(since)) => {
                Phrase::from("environment: none yet, building since ").time(*since)
            }
            Hold::Environment(Attention::NoneYet) => "environment: none yet".into(),
            Hold::AutomationFailed { name, run, outcome } => {
                format!("automation {name}: last run {run} {}", outcome.word()).into()
            }
        }
    }

    /// Its status (`docs/specs/shell.md`, Notices).
    pub(super) fn status(&self) -> Status {
        match self {
            Hold::UsageLimit { .. } | Hold::Drain(_) => Status::Paused,
            Hold::Environment(Attention::FirstBuilding(_)) => Status::InFlight,
            Hold::Environment(Attention::NoneYet) => Status::Paused,
            Hold::Environment(Attention::LatestFailed(_)) => Status::Failed,
            Hold::WebhooksRefused { .. }
            | Hold::PollsFailed(_)
            | Hold::UnparsedAutomation(_)
            | Hold::UnusableFile(_)
            | Hold::AutomationFailed { .. } => Status::Failed,
        }
    }

    /// The page that tells more of it.
    pub(super) fn href(&self) -> String {
        match self {
            Hold::PollsFailed(_)
            | Hold::UsageLimit { .. }
            | Hold::Drain(_)
            | Hold::WebhooksRefused { .. }
            | Hold::UnusableFile(_)
            | Hold::Environment(_) => "/server".into(),
            Hold::UnparsedAutomation(_) => "/automations".into(),
            Hold::AutomationFailed { run, .. } => run_path(run),
        }
    }
}

pub(super) struct HoldShown {
    pub(super) alert: &'static str,
    pub(super) phrase: Html,
    pub(super) href: String,
}

/// What the frame shows, from one read of `serve`'s view and the runs.
pub(super) struct Read {
    /// What `status` says of Linewright.
    pub(super) linewright: Linewright,
    /// `None` before the first poll.
    pub(super) automations: Option<Automations>,
    /// The view's, `None` before the first poll.
    pub(super) snapshot: Option<Arc<Snapshot>>,
    /// The environment's builds, from the same read as what `status` says
    /// of it.
    pub(super) builds: Vec<BuildShown>,
}

impl Read {
    pub(super) async fn of(sources: &Sources) -> Read {
        let view = api::view(sources);
        let recorded = api::recorded_or_none(sources).await;
        let now = sources.clock.now();
        let (environment, builds) = api::environment_or_none(sources)
            .await
            .and_then(|answer| answer.environment)
            .map_or((None, Vec::new()), |environment| {
                (Some(environment.shown), environment.builds)
            });
        let status = api::status_of(sources, view.as_ref(), &recorded, environment);
        Read {
            builds,
            automations: view
                .as_ref()
                .map(|view| Automations::of(&status, view, &recorded, now)),
            linewright: status.linewright,
            snapshot: view.map(|view| view.snapshot),
        }
    }

    /// How the page names items: with the titles of the view it read.
    pub(super) fn names<'a>(&'a self, repo: &'a str) -> Names<'a> {
        Names::of(repo, self.snapshot.as_deref())
    }
}

impl Frame {
    pub(super) async fn of(sources: &Sources) -> Frame {
        Frame::of_read(sources, &Read::of(sources).await)
    }

    pub(super) fn of_read(sources: &Sources, read: &Read) -> Frame {
        let linewright = &read.linewright;
        let repo = sources.work_dir.repo.clone();
        let names = read.names(&repo);
        let holds = Hold::of(linewright, read.automations.as_ref())
            .iter()
            .map(|hold| HoldShown {
                alert: hold.status().alert(),
                phrase: html::phrase(&hold.phrase(), &names),
                href: hold.href(),
            })
            .collect();
        Frame { repo, holds }
    }
}

#[derive(Template)]
#[template(path = "layout.html", block = "holds")]
struct HoldsRegion<'a> {
    frame: &'a Frame,
}

#[derive(Template)]
#[template(path = "layout.html", block = "title")]
struct TitleRegion<'a> {
    page: &'a Page,
}

#[derive(Template)]
#[template(path = "layout.html", block = "crumbs")]
struct CrumbsRegion<'a> {
    page: &'a Page,
}

/// The region of `page`'s title, which the stream of a page whose title can
/// change sends.
pub(super) fn title_region(page: &Page) -> (&'static str, String) {
    ("title", render(&TitleRegion { page }))
}

/// The region of `page`'s breadcrumbs, which the stream of a page whose
/// trail can change sends.
pub(super) fn crumbs_region(page: &Page) -> (&'static str, String) {
    ("crumbs", render(&CrumbsRegion { page }))
}

/// The frame's regions, which every page's stream sends before its own.
pub(super) fn frame_regions(frame: &Frame) -> Vec<(&'static str, String)> {
    vec![("holds", render(&HoldsRegion { frame }))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::shown::BuiltShown;

    #[track_caller]
    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// The status of the environment's notice, if it has one, with a setup
    /// command and `environment`.
    #[track_caller]
    fn environment_status(environment: EnvironmentShown) -> Option<Status> {
        let linewright = Linewright {
            serving_since: at("2026-10-01T12:00:00Z"),
            polls: None,
            failed_polls: None,
            usage_limit_resets_at: None,
            unusable_files: vec![],
            drain: None,
            environment: Some(environment),
            runs_in_flight: 0,
            max_runs: 4,
            webhooks: None,
        };
        match &Hold::of(&linewright, None)[..] {
            [] => None,
            [hold] => Some(hold.status()),
            holds => panic!("{} notices", holds.len()),
        }
    }

    /// `docs/specs/shell.md`, Notices: a setup command's rows.
    #[test_log::test]
    fn the_environments_notice_shows_as_the_notices_table_gives() {
        let built = Some(BuiltShown {
            commit: "abc1234".into(),
            at: at("2026-10-01T11:00:00Z"),
        });
        let building_since = Some(at("2026-10-01T12:10:00Z"));
        let latest_failed = Some(at("2026-10-01T12:05:00Z"));
        let cases = [
            (
                "a failed latest build, with an environment and a build running",
                EnvironmentShown {
                    built: built.clone(),
                    building_since,
                    latest_failed,
                },
                Some(Status::Failed),
            ),
            (
                "a failed latest build, with none yet",
                EnvironmentShown {
                    built: None,
                    building_since: None,
                    latest_failed,
                },
                Some(Status::Failed),
            ),
            (
                "none yet, while it builds",
                EnvironmentShown {
                    built: None,
                    building_since,
                    latest_failed: None,
                },
                Some(Status::InFlight),
            ),
            (
                "none yet, otherwise",
                EnvironmentShown {
                    built: None,
                    building_since: None,
                    latest_failed: None,
                },
                Some(Status::Paused),
            ),
            (
                "built, with a build running",
                EnvironmentShown {
                    built,
                    building_since,
                    latest_failed: None,
                },
                None,
            ),
        ];

        for (case, environment, expected) in cases {
            assert_eq!(environment_status(environment), expected, "{case}");
        }
    }
}
