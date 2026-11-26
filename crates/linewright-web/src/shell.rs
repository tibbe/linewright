//! What every screen has around it (`docs/specs/shell.md`): the navigation,
//! the notices, the breadcrumbs, the heading card with what a dialog made
//! under it, and the screen's buttons, and each screen's title
//! (`docs/specs/design_system.md`, Screens, Titles, and
//! `docs/specs/components.md`, Heading).

use std::convert::Infallible;

use askama::Template;
use axum::body::Bytes;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use linewright_core::access::{self, Permit};

use super::components::{CardTitle, HeadingLevel};
use super::html::{self, Html};
use super::icons::{CardIcon, Icon};
use super::names::Naming;
use super::{Page, cookie_headers, cookie_values, render, run_path};
pub(super) use linewright_core::actions::Missing;
use linewright_core::actions::{self, Action, AutomationsScreen, LastRunScreen, Overview};
use linewright_core::api::Sources;
use linewright_core::domain::{Repo, Timestamp};
use linewright_core::environment::shown::{Attention, EnvironmentShown};
use linewright_core::phrase::{Phrase, Status};
use linewright_core::repo_files::FileError;
use linewright_core::runs::{OutcomeName, RunId};
use linewright_core::status::{self, DrainShown, FailedPolls, Linewright, UnparsedAutomation};

/// The screen of the navigation that an area starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Area {
    Board,
    Runs,
    Automations,
    Benchmarks,
    Server,
    People,
}

impl Area {
    pub(super) const ALL: [Area; 6] = [
        Area::Board,
        Area::Runs,
        Area::Automations,
        Area::Benchmarks,
        Area::Server,
        Area::People,
    ];

    /// Its screen's name in the navigation.
    pub(super) fn name(self) -> &'static str {
        match self {
            Area::Board => "Board",
            Area::Runs => "Runs",
            Area::Automations => "Automations",
            Area::Benchmarks => "Benchmarks",
            Area::Server => "Server",
            Area::People => "People",
        }
    }

    /// The name of its screen's own crumb (`docs/specs/shell.md`,
    /// Breadcrumbs, Names).
    pub(super) fn listing(self) -> &'static str {
        match self {
            Area::Board => "All items",
            Area::Runs => "All runs",
            Area::Automations => "All automations",
            Area::Benchmarks => "All benchmarks",
            Area::Server => "Status",
            Area::People => "All people",
        }
    }

    /// Its icon in the navigation (`docs/specs/shell.md`, Navigation).
    pub(super) fn icon(self) -> Icon {
        match self {
            Area::Board => Icon::ViewColumns,
            Area::Runs => Icon::CommandLine,
            Area::Automations => Icon::Bolt,
            Area::Benchmarks => Icon::Beaker,
            Area::Server => Icon::Server,
            Area::People => Icon::Users,
        }
    }

    /// The path of its screen.
    pub(super) fn href(self) -> &'static str {
        match self {
            Area::Board => "/",
            Area::Runs => "/runs",
            Area::Automations => "/automations",
            Area::Benchmarks => "/benchmarks",
            Area::Server => "/server",
            Area::People => "/people",
        }
    }
}

/// The navigation a page draws from Tailwind's `lg` width, as the reader
/// last chose (`docs/specs/shell.md`, Navigation, Remembered): the `navigation`
/// cookie says `rail` for the rail, and anything else, or none, is the
/// sidebar. The reader's choice is posted as the form value `sidebar` or
/// `rail` (`docs/specs/architecture.md`, The web UI, The reader's choice
/// of navigation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Navigation {
    Sidebar,
    Rail,
}

impl Navigation {
    /// The name of the cookie that holds the choice.
    const COOKIE: &str = "navigation";

    /// The path the reader's choice is posted to.
    pub(super) const PATH: &str = "/navigation";

    /// The choice the `Cookie` headers `cookies` hold.
    fn of(cookies: impl IntoIterator<Item = impl AsRef<str>>) -> Navigation {
        let rail = cookie_values(cookies, Navigation::COOKIE)
            .iter()
            .any(|value| value == Navigation::Rail.value());
        if rail {
            Navigation::Rail
        } else {
            Navigation::Sidebar
        }
    }

    /// The `Set-Cookie` that remembers it for a year (`docs/specs/shell.md`,
    /// Navigation, Remembered). The server sets it, as Safari deletes a cookie
    /// written from script after 7 days without the reader using the site
    /// (<https://webkit.org/tracking-prevention/>), and only the server reads
    /// it.
    fn set_cookie(self) -> String {
        format!(
            "{}={}; Path=/; Max-Age=31536000; SameSite=Lax; HttpOnly",
            Navigation::COOKIE,
            self.value()
        )
    }

    /// Its value in the cookie and in the form, as serde reads it.
    fn value(self) -> &'static str {
        match self {
            Navigation::Sidebar => "sidebar",
            Navigation::Rail => "rail",
        }
    }

    /// Whether it is the rail.
    pub(super) fn is_rail(self) -> bool {
        self == Navigation::Rail
    }

    /// The name of the button that switches to the other navigation
    /// (`docs/specs/shell.md`, Navigation, Expanding and collapsing).
    pub(super) fn toggle_name(self) -> &'static str {
        match self {
            Navigation::Sidebar => "Collapse navigation",
            Navigation::Rail => "Expand navigation",
        }
    }
}

impl<S: Sync> FromRequestParts<S> for Navigation {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> impl Future<Output = Result<Navigation, Infallible>> {
        std::future::ready(Ok(Navigation::of(cookie_headers(&parts.headers))))
    }
}

/// The form the navigation's foot button posts: the reader's choice.
#[derive(Deserialize)]
struct NavigationForm {
    navigation: Navigation,
}

/// Remembers the reader's choice of navigation in its cookie
/// (`docs/specs/shell.md`, Navigation, Remembered), for a request from any
/// page: a 204, or a 400 with no cookie for a form that does not read.
pub(super) async fn remember_navigation(body: Bytes) -> Response {
    match serde_urlencoded::from_bytes::<NavigationForm>(&body) {
        Ok(form) => (
            StatusCode::NO_CONTENT,
            [(header::SET_COOKIE, form.navigation.set_cookie())],
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            format!("the form does not read: {e}"),
        )
            .into_response(),
    }
}

/// Which of the spec's kinds of screen a page is (`docs/specs/shell.md`,
/// Breadcrumbs and No such thing; `docs/specs/components.md`, Heading),
/// which decides its breadcrumbs, its heading card and its title.
#[derive(Clone)]
pub(super) enum Place {
    /// A screen of the navigation: its crumbs are the area, linked, then
    /// what it lists, such as "All runs". It has no heading card.
    Navigation(Area),
    /// A screen of one thing that belongs to others: its crumbs are the
    /// area, linked, each crumb, linked, then `screen`, such as "Benchmark
    /// run". `crumbs` lacks what Linewright could not find.
    Within {
        area: Area,
        crumbs: Vec<Crumb>,
        screen: &'static str,
        heading: Heading,
    },
    /// A screen of one thing that belongs to no other, such as
    /// Run: it has no breadcrumbs, and its heading's name is its `h1`.
    Alone(Heading),
    /// A screen of the account's menu, named `name`, such as
    /// Authorizations: it has no breadcrumbs and no heading card, and its
    /// card's title is its `h1`.
    Account { name: &'static str },
    /// No such thing: an address with nothing at it. It has no breadcrumbs
    /// and no heading card, and its name, [`NOT_FOUND`], is its `h1`.
    NotFound,
}

/// The name of No such thing, as its `h1` shows it (`docs/specs/shell.md`,
/// No such thing).
pub(super) const NOT_FOUND: &str = "Not found";

/// A heading card (`docs/specs/components.md`, Heading).
#[derive(Clone)]
pub(super) struct Heading {
    /// Plain text, for the screen's title and for the crumb of a screen
    /// under it, such as "Benchmark run 14".
    pub(super) title: String,
    pub(super) name: Html,
    /// The icon before its name, if it has one.
    pub(super) icon: Option<CardIcon>,
    /// The badge of the screen's state, where the screen gives it there.
    pub(super) state: Option<(Status, &'static str)>,
    /// The line under the name, such as the benchmark's stage.
    pub(super) line: Option<Html>,
}

impl Heading {
    /// Its name as its card's title, at `level`.
    pub(super) fn card_title(&self, level: HeadingLevel) -> CardTitle {
        CardTitle {
            level,
            icon: self.icon,
            name: self.name.clone(),
        }
    }

    /// The heading named `title`, with no icon, no state and no line.
    pub(super) fn named(title: impl Into<String>) -> Heading {
        let title = title.into();
        Heading {
            name: html::escape(&title),
            title,
            icon: None,
            state: None,
            line: None,
        }
    }
}

/// A crumb that links to its screen.
#[derive(Clone)]
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
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CrumbShown<'a> {
    pub(super) name: &'a str,
    pub(super) href: Option<&'a str>,
}

impl Place {
    /// The crumbs of a screen within `area`, starting from its screen.
    pub(super) fn within(area: Area) -> Within {
        Within {
            area,
            crumbs: vec![],
        }
    }

    /// Its crumbs in order, the screen's own last; none on a screen of one
    /// thing that belongs to no other.
    pub(super) fn crumbs(&self) -> Vec<CrumbShown<'_>> {
        let (area, crumbs, screen): (Area, &[Crumb], &str) = match self {
            Place::Navigation(area) => (*area, &[], area.listing()),
            Place::Within {
                area,
                crumbs,
                screen,
                ..
            } => (*area, crumbs, screen),
            Place::Alone(..) | Place::Account { .. } | Place::NotFound => return vec![],
        };
        let area = CrumbShown {
            name: area.name(),
            href: Some(area.href()),
        };
        let crumbs = crumbs.iter().map(|crumb| CrumbShown {
            name: &crumb.name,
            href: Some(&crumb.href),
        });
        let screen = CrumbShown {
            name: screen,
            href: None,
        };
        std::iter::once(area)
            .chain(crumbs)
            .chain(std::iter::once(screen))
            .collect()
    }

    /// Whether it has breadcrumbs, whose last crumb is then the page's `h1`
    /// and at whose right are the screen's buttons.
    pub(super) fn has_crumbs(&self) -> bool {
        !matches!(
            self,
            Place::Alone(..) | Place::Account { .. } | Place::NotFound
        )
    }

    /// Its heading card, on a screen of one thing.
    pub(super) fn heading(&self) -> Option<&Heading> {
        match self {
            Place::Navigation(_) | Place::Account { .. } | Place::NotFound => None,
            Place::Within { heading, .. } | Place::Alone(heading) => Some(heading),
        }
    }

    /// The screen's title, such as "Benchmark run 14 · Linewright"
    /// (`docs/specs/design_system.md`, Screens, Titles).
    pub(super) fn title(&self) -> String {
        let name = match self {
            Place::Navigation(area) => area.name(),
            Place::Account { name, .. } => name,
            Place::Within { heading, .. } | Place::Alone(heading) => &heading.title,
            Place::NotFound => NOT_FOUND,
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

    /// The place of the screen `screen`, under `heading`, after the crumbs.
    pub(super) fn screen(self, screen: &'static str, heading: Heading) -> Place {
        Place::Within {
            area: self.area,
            crumbs: self.crumbs,
            screen,
            heading,
        }
    }
}

/// The frame every page has around its own regions
/// (`docs/specs/shell.md`).
pub(super) struct Frame {
    pub(super) repo: Repo,
    pub(super) holds: Vec<HoldShown>,
    pub(super) account: Account,
    /// The reader's access level.
    reach: access::Level,
}

/// The reader, as the account at the foot of the navigation shows them
/// (`docs/specs/shell.md`, The account).
pub(super) struct Account {
    pub(super) name: String,
    pub(super) avatar: Option<String>,
    /// What stands in for the avatar without one.
    pub(super) letter: String,
    pub(super) role: &'static str,
}

impl Account {
    fn of(caller: &access::Caller) -> Account {
        let person = caller.person();
        Account {
            name: person.shown_name().to_string(),
            avatar: person.avatar.as_ref().map(ToString::to_string),
            letter: person.letter(),
            role: caller.role().name(),
        }
    }
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
        outcome: OutcomeName,
    },
}

impl Hold {
    pub(super) fn of(
        linewright: &Linewright,
        automations: Option<&AutomationsScreen>,
    ) -> Vec<Hold> {
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
            .flat_map(|automations| &automations.unparsed_automations)
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
            .flat_map(|automations| &automations.automations)
            .filter_map(|automation| match &automation.last_run {
                LastRunScreen::Run {
                    run,
                    last_run_outcome,
                } if last_run_outcome.failed() => Some(Hold::AutomationFailed {
                    name: automation.name.clone(),
                    run: run.id.clone(),
                    outcome: *last_run_outcome,
                }),
                LastRunScreen::Run { .. } | LastRunScreen::NoneYet { .. } => None,
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
    pub(super) status: Status,
    pub(super) phrase: Html,
    pub(super) href: String,
}

/// How a page names items: with the titles of the view `overview` read.
pub(super) fn naming<'a>(overview: &'a Overview, repo: &'a str) -> Naming<'a> {
    Naming::Titled {
        repo,
        snapshot: overview.snapshot.as_deref(),
    }
}

impl Frame {
    pub(super) async fn of(permit: &Permit<access::Read>, sources: &Sources) -> Frame {
        Frame::of_read(permit, sources, &actions::overview(permit, sources).await)
    }

    pub(super) fn of_read(
        permit: &Permit<access::Read>,
        sources: &Sources,
        read: &Overview,
    ) -> Frame {
        let linewright = &read.server.linewright;
        let repo = sources.work_dir.repo.clone();
        let naming = naming(read, &repo);
        let holds = Hold::of(linewright, read.automations.as_ref())
            .iter()
            .map(|hold| HoldShown {
                status: hold.status(),
                phrase: html::phrase(&hold.phrase(), &naming),
                href: hold.href(),
            })
            .collect();
        Frame {
            repo,
            holds,
            account: Account::of(permit.caller()),
            reach: permit.caller().role().level(),
        }
    }

    /// Whether the reader's role allows `action`, for whether the screen
    /// shows its buttons (`docs/specs/access.md`, Actions, What the
    /// reader's role does not allow).
    pub(super) fn allows(&self, action: Action) -> bool {
        self.reach >= action.level()
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
#[template(path = "shell_crumbs.html")]
struct CrumbsRegion<'a> {
    page: &'a Page,
}

#[derive(Template)]
#[template(path = "shell_heading.html")]
struct HeadingRegion<'a> {
    page: &'a Page,
    frame: &'a Frame,
}

#[derive(Template)]
#[template(path = "shell_buttons.html")]
struct ButtonsRegion<'a> {
    page: &'a Page,
    frame: &'a Frame,
}

/// The regions of the shell, which every page's stream sends before its
/// own: the notices, the title, the breadcrumbs, the heading card and the
/// screen's buttons. Each that `page` lacks is sent empty, so that its id is
/// always there to morph. The `#made` under the heading card is none of them,
/// so that what a dialog made stays there until the reader leaves
/// (`docs/specs/design_system.md`, Actions, What was made).
pub(super) fn regions(frame: &Frame, page: &Page) -> Vec<(&'static str, String)> {
    vec![
        holds(frame),
        ("title", render(&TitleRegion { page })),
        ("crumbs", render(&CrumbsRegion { page })),
        ("heading", render(&HeadingRegion { page, frame })),
        ("buttons", render(&ButtonsRegion { page, frame })),
    ]
}

/// The region of the notices alone, for a stream that could not read what
/// its screen is of this time, which leaves the rest of the shell as it
/// last sent it.
pub(super) fn holds(frame: &Frame) -> (&'static str, String) {
    ("holds", render(&HoldsRegion { frame }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use linewright_core::environment::shown::BuiltShown;

    /// `docs/specs/shell.md`, Navigation, Remembered: the cookie the server
    /// sets, among the browser's others, chooses the rail; anything else is
    /// the sidebar.
    #[test_log::test]
    fn the_navigation_cookie_saying_rail_chooses_the_rail_and_anything_else_the_sidebar() {
        for (headers, chosen) in [
            (vec!["theme=dark", "navigation=rail"], Navigation::Rail),
            (vec!["navigation=wide"], Navigation::Sidebar),
            (vec!["railnavigation=rail"], Navigation::Sidebar),
            (vec!["navigation=railway"], Navigation::Sidebar),
        ] {
            assert_eq!(Navigation::of(&headers), chosen, "{headers:?}");
        }
    }

    /// A crumb linking to `href`.
    fn link<'a>(name: &'a str, href: &'a str) -> CrumbShown<'a> {
        CrumbShown {
            name,
            href: Some(href),
        }
    }

    /// The crumb of the screen itself.
    fn here(name: &str) -> CrumbShown<'_> {
        CrumbShown { name, href: None }
    }

    /// `docs/specs/shell.md`, Breadcrumbs, Names; `docs/specs/design_system.md`,
    /// Screens, Titles.
    #[test_log::test]
    fn a_screen_of_the_navigation_ends_its_crumbs_in_what_it_lists_and_is_titled_by_its_area() {
        let cases = [
            (
                Area::Board,
                ("Board", "/"),
                "All items",
                "Board · Linewright",
            ),
            (
                Area::Automations,
                ("Automations", "/automations"),
                "All automations",
                "Automations · Linewright",
            ),
            (
                Area::People,
                ("People", "/people"),
                "All people",
                "People · Linewright",
            ),
        ];

        for (area, (name, href), listing, title) in cases {
            let place = Place::Navigation(area);

            assert_eq!(
                place.crumbs(),
                [link(name, href), here(listing)],
                "{area:?}"
            );
            assert_eq!(place.title(), title, "{area:?}");
            assert!(place.heading().is_none(), "{area:?}");
        }
    }

    /// `docs/specs/shell.md`, No such thing; Breadcrumbs, No such thing.
    #[test_log::test]
    fn no_such_thing_has_no_crumbs_and_no_heading_and_is_titled_not_found() {
        let place = Place::NotFound;

        assert_eq!(place.crumbs(), []);
        assert!(!place.has_crumbs());
        assert!(place.heading().is_none());
        assert_eq!(place.title(), "Not found · Linewright");
    }

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// The status of the environment's notice, if it has one, with a setup
    /// command and `environment`.
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
            trials_in_flight: None,
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
