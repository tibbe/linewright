#![allow(
    clippy::items_after_statements,
    reason = "`test_log::test` puts its setup before each test's body, so a `use` at the top of a test comes after a statement the test never wrote"
)]

use linewright_core::testing;

mod automations;
mod benchmark_runs;
mod benchmarks;
mod board;
mod capture;
mod environment;
mod frame;
mod item;
mod look;
mod mcp;
mod oauth;
mod people;
mod run;
mod runs;
mod script;
mod serving;
mod shell;
mod sign_in;
mod task;
mod theme;

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use http_body_util::BodyExt as _;
use linewright_core::access::Role;
use linewright_core::api::Sources;
use linewright_core::clock::Clock;
use linewright_core::testing::*;
use linewright_core::webhook::{Listener, Pending};
use scraper::{ElementRef, Html, Selector};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

/// `serve` polling hourly, so its view holds still but for deliveries, and
/// its router driven in-process.
struct Web {
    serving: Serving,
    /// A manager's browser.
    cookie: String,
    listener: Listener,
    router: Router,
    stopping: CancellationToken,
    store: std::sync::Arc<linewright_core::store::local::LocalStore>,
    harness: std::sync::Arc<ScriptedHarness>,
    clock: std::sync::Arc<FakeClock>,
    dir: tempfile::TempDir,
}

/// The work directory of `REPO` under `dir`, where its runs keep their
/// records.
fn work_dir(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("work").join(REPO)
}

/// What `serve` answers from, over `served` and `polling`, with webhooks
/// off, and the runs of the database of the work directory, which it makes.
#[track_caller]
fn sources(
    dir: &tempfile::TempDir,
    clock: &std::sync::Arc<FakeClock>,
    since: linewright_core::domain::Timestamp,
    served: tokio::sync::watch::Receiver<Option<linewright_core::api::Served>>,
    polling: tokio::sync::watch::Receiver<Option<linewright_core::status::Polling>>,
) -> impl Future<Output = Sources> + use<> {
    let runs = runs_in(&work_dir(dir));
    let (clock, base) = (clock.clone(), dir.path().join("work"));
    async move {
        Sources {
            serving: linewright_core::status::Serving { since, max_runs: 4 },
            served,
            polling,
            deliveries: None,
            clock,
            work_dir: linewright_core::runs::RepoWorkDir {
                base,
                repo: REPO.parse().unwrap(),
            },
            runs: runs.await,
            ends: linewright_core::ending::Ends::new(),
            environment: linewright_core::api::EnvironmentLink::default(),
            benchmark_runs: linewright_core::api::BenchmarkRunsLink::default(),
            exiting: linewright_core::winding_up::WindingUp::default().exiting(),
        }
    }
}

impl Web {
    async fn start(world: World) -> Web {
        let web = Web::polling(world, Duration::from_hours(1)).await;
        let mut served = web.serving.served.clone();
        tokio::time::timeout(Duration::from_secs(10), served.wait_for(Option::is_some))
            .await
            .expect("the first poll")
            .unwrap();
        web
    }

    /// `serve` polling every `poll_interval`, without waiting for its first
    /// poll.
    async fn polling(world: World, poll_interval: Duration) -> Web {
        let World {
            clock,
            factory,
            dir,
            store,
            harness,
        } = world;
        let since = clock.now();
        let pending = Pending::default();
        let listener = Listener::bind(&webhooks("127.0.0.1:0"), pending.clone(), clock.clone())
            .await
            .unwrap();
        let serving = Serving::start(factory, poll_interval, pending);
        let stopping = CancellationToken::new();
        let sources = Sources {
            deliveries: Some(listener.tally()),
            environment: serving.environment.clone(),
            ..sources(
                &dir,
                &clock,
                since,
                serving.served.clone(),
                serving.polling.clone(),
            )
            .await
        };
        let cookie = signed_in_as(&sources, Role::Manager, MANAGER).await;
        let router = linewright_web::router(sources, &ui(), stopping.clone());
        Web {
            serving,
            cookie,
            listener,
            router,
            stopping,
            store,
            harness,
            clock,
            dir,
        }
    }

    /// A factory beside `serve`'s, as `linewright automation run` starts
    /// one, over the work directory a world adjusted to `work_dir/REPO`.
    fn work_dir(&self) -> std::path::PathBuf {
        work_dir(&self.dir)
    }

    async fn factory_beside(&self) -> linewright_core::factory::Factory {
        linewright_core::factory::Factory::start(
            self.store.clone(),
            self.harness.clone(),
            self.clock.clone(),
            config(&self.work_dir()).await,
        )
        .await
        .unwrap()
    }

    /// A delivery naming issue #`number`.
    async fn deliver_issue(&self, number: u64) {
        let body = serde_json::json!({"issue": {"number": number}}).to_string();
        let status = signed(&self.listener, "issues", &body).await;
        assert!(status.is_success(), "{status}");
    }

    async fn stop(self) {
        self.stopping.cancel();
        self.serving.stop().await;
        self.listener.stop().await;
    }

    async fn eventually_page(&self, path: &str, shown: impl Fn(&Html) -> bool) -> Html {
        let waited = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let html = self.page(path).await;
                if shown(&html) {
                    return html;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        waited.expect("shown in time")
    }
}

impl Routed for Web {
    fn router(&self) -> &Router {
        &self.router
    }

    fn cookie(&self) -> &str {
        &self.cookie
    }
}

/// The web UI's router driven in-process over a view built by hand over a
/// local store, with no factory and no poll.
struct Pages {
    router: Router,
    clock: std::sync::Arc<FakeClock>,
    /// A manager's browser.
    cookie: String,
    sources: Sources,
    served: tokio::sync::watch::Sender<Option<linewright_core::api::Served>>,
    /// Set by [`Pages::with_deliveries`].
    listener: Option<Listener>,
    store: std::sync::Arc<linewright_core::store::local::LocalStore>,
    stopping: CancellationToken,
    dir: tempfile::TempDir,
}

/// The session settings a run of `stage` uses in [`Pages`]: an implement
/// run's model `claude-opus-5-5` and effort high, every other stage's
/// `claude-sonnet-5-5` and medium, and every stage's session timeout 6
/// hours.
fn stage_settings(
    stage: linewright_core::task::TaskStage,
) -> linewright_core::work_settings::Partial {
    use linewright_core::work_settings::Effort;
    let (name, effort) = if stage == linewright_core::task::TaskStage::Implement {
        ("claude-opus-5-5", Effort::High)
    } else {
        ("claude-sonnet-5-5", Effort::Medium)
    };
    linewright_core::work_settings::Partial {
        model: Some(model(name)),
        effort: Some(effort),
        session_timeout: Some(std::time::Duration::from_hours(6)),
    }
}

impl Pages {
    /// The pages of `view`, as `serve` shows them once a poll has made it.
    async fn of(view: linewright_core::status::View) -> Pages {
        Pages::over(view, false).await
    }

    /// [`Pages::of`]'s, with webhooks on: a bare webhook listener, that
    /// nothing takes the deliveries of, tallies them for the pages.
    async fn with_deliveries(view: linewright_core::status::View) -> Pages {
        Pages::over(view, true).await
    }

    async fn over(view: linewright_core::status::View, deliveries: bool) -> Pages {
        let dir = tempfile::tempdir().unwrap();
        let clock = std::sync::Arc::new(FakeClock::new());
        let store = std::sync::Arc::new(
            local_store(
                &dir.path().join("store"),
                &Repository::default(),
                clock.clone(),
            )
            .await,
        );
        let listener = if deliveries {
            let listener =
                Listener::bind(&webhooks("127.0.0.1:0"), Pending::default(), clock.clone())
                    .await
                    .unwrap();
            Some(listener)
        } else {
            None
        };
        let polling = linewright_core::status::Polling {
            next: view.polls.next,
            failed: None,
        };
        let served = Some(linewright_core::api::Served::new(
            view,
            store.clone(),
            linewright_core::redaction::Redaction::default(),
            std::sync::Arc::new(linewright_core::briefs::Briefs::shipped()),
            stage_settings,
        ));
        let (served, served_receiver) = tokio::sync::watch::channel(served);
        let (_, polling) = tokio::sync::watch::channel(Some(polling));
        let since = clock.now();
        let sources = Sources {
            deliveries: listener.as_ref().map(Listener::tally),
            ..sources(&dir, &clock, since, served_receiver, polling).await
        };
        let stopping = CancellationToken::new();
        let router = linewright_web::router(sources.clone(), &ui(), stopping.clone());
        let cookie = signed_in_as(&sources, Role::Manager, MANAGER).await;
        Pages {
            router,
            clock,
            cookie,
            sources,
            served,
            listener,
            store,
            stopping,
            dir,
        }
    }

    /// The webhook listener of [`Pages::with_deliveries`].
    fn deliveries(&self) -> &Listener {
        self.listener.as_ref().expect("webhooks on")
    }

    /// Publishes `view` as the next one `serve` shows, as a poll would.
    fn publish(&self, view: linewright_core::status::View) {
        self.publish_with_settings(view, stage_settings);
    }

    /// Publishes `view` as [`Pages::publish`] does, a run of each stage
    /// using the session settings `settings` gives.
    fn publish_with_settings(
        &self,
        view: linewright_core::status::View,
        settings: impl Fn(linewright_core::task::TaskStage) -> linewright_core::work_settings::Partial,
    ) {
        let served = linewright_core::api::Served::new(
            view,
            self.store.clone(),
            linewright_core::redaction::Redaction::default(),
            std::sync::Arc::new(linewright_core::briefs::Briefs::shipped()),
            settings,
        );
        self.served.send_replace(Some(served));
    }

    /// The web UI listening on a port of its own, at [`public_url`].
    async fn listen(&self) -> linewright_core::serving::UiListener {
        linewright_core::serving::UiListener::bind(
            &ui_on("127.0.0.1:0"),
            self.sources.clone(),
            linewright_web::router,
        )
        .await
        .unwrap()
    }

    /// The pages' router, at the public URL `url`.
    #[track_caller]
    fn at(&self, url: &str) -> Router {
        let ui = linewright_core::config::Ui {
            public_url: linewright_core::host::PublicUrl::parse(url).unwrap(),
            ..ui()
        };
        linewright_web::router(self.sources.clone(), &ui, self.stopping.clone())
    }

    /// The pages' router where the `manager` setting names `manager`, asked
    /// from the manager's browser.
    #[track_caller]
    fn with_manager(&self, manager: &str) -> Asked {
        let ui = ui();
        let manager = linewright_core::access::people::ManagerName::parse(manager, &ui.providers)
            .unwrap_or_else(|e| panic!("{manager}: {e:?}"));
        let ui = linewright_core::config::Ui {
            manager: Some(manager),
            ..ui
        };
        Asked {
            router: linewright_web::router(self.sources.clone(), &ui, self.stopping.clone()),
            cookie: self.cookie.clone(),
        }
    }

    /// Keeps the run `record`, in the form of a run's record file, as its
    /// run would have kept it.
    #[track_caller]
    fn record(&self, record: &serde_json::Value) -> impl Future<Output = ()> + use<> {
        keep_record(&work_dir(&self.dir), record)
    }

    /// Keeps `input` as the input the run `id` recorded.
    #[track_caller]
    fn input(&self, id: &str, input: &serde_json::Value) -> impl Future<Output = ()> + use<> {
        keep_input(&work_dir(&self.dir), id, input)
    }

    /// Writes each of `files`, a path under the work directory and what it
    /// holds, as runs leave them there.
    fn write(&self, files: &[(&str, &str)]) {
        write_files(&work_dir(&self.dir), files);
    }

    /// Adds `text` to the end of `path` under the work directory, as a
    /// session adds to its output while it runs.
    fn append(&self, path: &str, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.path(path))
            .unwrap();
        std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
    }

    /// The work directory's path of `path`.
    fn path(&self, path: &str) -> std::path::PathBuf {
        work_dir(&self.dir).join(path)
    }
}

impl Routed for Pages {
    fn router(&self) -> &Router {
        &self.router
    }

    fn cookie(&self) -> &str {
        &self.cookie
    }
}

impl Pages {
    /// The pages as a browser signed in as a viewer sees them.
    #[track_caller]
    fn as_viewer(&self) -> impl Future<Output = AsRole<'_>> {
        self.as_role(Role::Viewer, "Vera Viewer")
    }

    /// The pages as a browser signed in as a contributor sees them.
    #[track_caller]
    fn as_contributor(&self) -> impl Future<Output = AsRole<'_>> {
        self.as_role(Role::Contributor, "Carl Contributor")
    }

    #[track_caller]
    fn as_role(&self, role: Role, name: &str) -> impl Future<Output = AsRole<'_>> {
        let cookie = signed_in_as(&self.sources, role, name);
        async move {
            AsRole {
                router: &self.router,
                cookie: cookie.await,
            }
        }
    }
}

/// The name of the manager whose browser the tests ask from, but where they
/// say otherwise.
const MANAGER: &str = "Ada Lovelace";

/// A router asked by a browser of its own.
struct AsRole<'a> {
    router: &'a Router,
    cookie: String,
}

impl Routed for AsRole<'_> {
    fn router(&self) -> &Router {
        self.router
    }

    fn cookie(&self) -> &str {
        &self.cookie
    }
}

/// A router of its own, asked by a browser.
struct Asked {
    router: Router,
    cookie: String,
}

impl Routed for Asked {
    fn router(&self) -> &Router {
        &self.router
    }

    fn cookie(&self) -> &str {
        &self.cookie
    }
}

impl Drop for Pages {
    /// Ends the event streams.
    fn drop(&mut self) {
        self.stopping.cancel();
    }
}

/// What answers the web UI's requests in-process.
trait Routed {
    fn router(&self) -> &Router;

    /// The `Cookie` of the browser that asks: signed in.
    fn cookie(&self) -> &str;

    async fn response(&self, path: &str) -> axum::response::Response {
        routed_with_cookie(
            self.router(),
            hyper::Method::GET,
            Some(UI_HOST),
            path,
            self.cookie(),
        )
        .await
    }

    async fn get(&self, path: &str) -> (u16, String) {
        status_and_body(self.response(path).await).await
    }

    async fn page(&self, path: &str) -> Html {
        let (status, body) = self.get(path).await;
        assert_eq!(status, 200, "{body}");
        Html::parse_document(&body)
    }
}

/// The page at `path` of `web` as a browser holding the cookies `cookie`
/// asks for it, whatever its status.
#[track_caller]
fn page_with_cookie<W: Routed>(
    web: &W,
    path: &str,
    cookie: &str,
) -> impl Future<Output = Html> + use<W> {
    let answer = answer_with_cookie(web, path, cookie);
    async move { answer.await.2 }
}

/// As [`page_with_cookie`], with the status and the headers of the answer
/// too.
#[track_caller]
fn answer_with_cookie<W: Routed>(
    web: &W,
    path: &str,
    cookie: &str,
) -> impl Future<Output = (u16, axum::http::HeaderMap, Html)> + use<W> {
    let caller = std::panic::Location::caller();
    let request = axum::http::Request::builder()
        .uri(path)
        .header("Host", UI_HOST)
        .header("Cookie", format!("{}; {cookie}", web.cookie()))
        .body(Body::empty())
        .unwrap_or_else(|e| panic!("building the request at {caller}: {e}"));
    let router = web.router().clone();
    async move {
        let Ok(response) = router.oneshot(request).await;
        let answered = response.headers().clone();
        let (status, body) = status_and_body(response).await;
        (status, answered, Html::parse_document(&body))
    }
}

/// A web UI's own page's request.
const SAME_ORIGIN: (&str, &str) = ("sec-fetch-site", "same-origin");

/// Another site's request.
const CROSS_SITE: (&str, &str) = ("sec-fetch-site", "cross-site");

/// Posts the form `fields` to `path` with `headers` besides its `Host`, as
/// a browser does: the status and the fragment it answers with.
#[track_caller]
fn post<W: Routed>(
    web: &W,
    path: &str,
    headers: &[(&str, &str)],
    fields: &[(&str, &str)],
) -> impl Future<Output = (u16, Html)> + use<W> {
    let answer = post_answer(web, path, headers, fields);
    async move {
        let (status, _, html) = answer.await;
        (status, html)
    }
}

/// As [`post`], with the headers of the answer too.
#[track_caller]
fn post_answer<W: Routed>(
    web: &W,
    path: &str,
    headers: &[(&str, &str)],
    fields: &[(&str, &str)],
) -> impl Future<Output = (u16, axum::http::HeaderMap, Html)> + use<W> {
    let caller = std::panic::Location::caller();
    let mut form = form_urlencoded::Serializer::new(String::new());
    for (name, value) in fields {
        form.append_pair(name, value);
    }
    let mut request = axum::http::Request::builder()
        .method(hyper::Method::POST)
        .uri(path)
        .header("Host", format!("{UI_HOST}:8080"))
        .header("Cookie", web.cookie())
        .header("Content-Type", "application/x-www-form-urlencoded");
    for (header, value) in headers {
        request = request.header(*header, *value);
    }
    let request = request
        .body(Body::from(form.finish()))
        .unwrap_or_else(|e| panic!("building the request at {caller}: {e}"));
    let router = web.router().clone();
    async move {
        let Ok(response) = router.oneshot(request).await;
        let answered = response.headers().clone();
        let (status, body) = status_and_body(response).await;
        (status, answered, Html::parse_fragment(&body))
    }
}

/// What a fragment says, as a reader reads it.
#[track_caller]
fn said(html: &Html) -> String {
    let alerts = select(html, ".alert");
    assert_eq!(alerts.len(), 1, "not one thing said");
    text(alerts[0])
}

/// The tasks under `pages`' work directory, by name.
#[track_caller]
fn tasks(pages: &Pages) -> Vec<String> {
    match std::fs::read_dir(pages.path("tasks")) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => panic!("reading the tasks: {e}"),
    }
}

/// The rows of the table `table` on `html`, each its cells' text.
fn rows(html: &Html, table: &str) -> Vec<Vec<String>> {
    select(html, &format!("{table} tbody tr"))
        .into_iter()
        .map(|row| select_in(row, "th, td").into_iter().map(text).collect())
        .collect()
}

fn select<'a>(html: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    html.select(&Selector::parse(selector).unwrap()).collect()
}

/// What `selector` selects in `element`.
fn select_in<'a>(element: ElementRef<'a>, selector: &str) -> Vec<ElementRef<'a>> {
    element
        .select(&Selector::parse(selector).unwrap())
        .collect()
}

/// A confirmation (`docs/specs/ui_patterns.md`, Confirming) as a reader
/// meets it: the label of the button that asks, the question, the label of
/// the button that turns it down, and where going ahead posts and what.
#[derive(Debug, PartialEq, Eq)]
struct Confirmation {
    asking: String,
    question: String,
    turn_down: String,
    action: String,
    fields: Vec<(String, String)>,
}

/// Each confirmation on the page, by the button that asks, but for the
/// buttons that open a "New" dialog or the capture's; checking that the button opens its dialog
/// as a modal with no request, that the dialog is named by its question,
/// that focus starts on turning it down, and that the button that goes ahead
/// has the asking button's label.
#[track_caller]
fn confirmations(html: &Html) -> Vec<Confirmation> {
    let mut confirmations = vec![];
    for asking in select(html, "button[command=show-modal]") {
        assert_eq!(asking.attr("type"), Some("button"), "{}", asking.html());
        let id = asking.attr("commandfor").expect("the dialog it opens");
        let dialogs = select(html, &format!("[id='{id}']"));
        assert_eq!(dialogs.len(), 1, "one element {id}");
        let dialog = dialogs[0];
        if !select_in(dialog, "form[data-new], form.capture").is_empty() {
            continue;
        }
        assert_eq!(dialog.value().name(), "dialog", "{}", dialog.html());
        assert!(has_classes(dialog, &["modal"]), "{}", dialog.html());
        let question = dialog.attr("aria-labelledby").expect("its question");
        let questions = select(html, &format!("[id='{question}']"));
        assert_eq!(questions.len(), 1, "one element {question}");
        let turn_down = select_in(dialog, "form[method=dialog] button");
        assert_eq!(turn_down.len(), 1, "{}", dialog.html());
        assert!(turn_down[0].attr("autofocus").is_some());
        let ahead = select_in(dialog, "form[hx-post]");
        assert_eq!(ahead.len(), 1, "{}", dialog.html());
        let action = ahead[0].attr("hx-post").unwrap().to_string();
        assert_eq!(ahead[0].attr("action"), Some(action.as_str()));
        let submit = select_in(ahead[0], "button[type=submit]");
        assert_eq!(submit.len(), 1, "{}", dialog.html());
        let asking = text(asking);
        assert_eq!(text(submit[0]), asking);
        let mut fields = vec![];
        for input in select_in(ahead[0], "input[type=hidden]") {
            fields.push((
                input.attr("name").unwrap().to_string(),
                input.attr("value").unwrap().to_string(),
            ));
        }
        confirmations.push(Confirmation {
            asking,
            question: text(questions[0]),
            turn_down: text(turn_down[0]),
            action,
            fields,
        });
    }
    confirmations
}

/// The facts in the `stats` of `section` on `html`
/// (`docs/specs/benchmarks.md`, Benchmark run), each its `stat-title` and
/// its `stat-value`.
#[track_caller]
fn stats(html: &Html, section: &str) -> Vec<(String, String)> {
    select(html, &format!("{section} .stats > .stat"))
        .into_iter()
        .map(|stat| {
            (
                text(select_in(stat, ".stat-title")[0]),
                text(select_in(stat, ".stat-value")[0]),
            )
        })
        .collect()
}

/// Where each of `links` goes.
fn hrefs(links: Vec<ElementRef>) -> Vec<String> {
    links
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .map(str::to_string)
        .collect()
}

/// Whether `element` has every one of `classes`.
fn has_classes(element: ElementRef, classes: &[&str]) -> bool {
    classes.iter().all(|class| {
        element
            .value()
            .has_class(class, scraper::CaseSensitivity::CaseSensitive)
    })
}

/// Asserts that `selector` selects something in `html`, all of it in
/// secondary text and in no other translucent text colour:
/// `docs/specs/design_system.md`, Type: secondary text is in
/// `text-base-content/60`.
fn assert_in_secondary_text(html: &Html, selector: &str) {
    let selected = select(html, selector);
    assert!(!selected.is_empty(), "{selector}");
    for element in selected {
        assert!(
            has_classes(element, &["text-base-content/60"])
                && element
                    .value()
                    .classes()
                    .filter(|class| class.starts_with("text-") && class.contains('/'))
                    .count()
                    == 1,
            "{}",
            element.html()
        );
    }
}

/// The classes of `element` that start with any of `prefixes`, sorted.
fn classes_starting_with<'a>(element: ElementRef<'a>, prefixes: &[&str]) -> Vec<&'a str> {
    let mut classes: Vec<&str> = element
        .value()
        .classes()
        .filter(|class| prefixes.iter().any(|prefix| class.starts_with(prefix)))
        .collect();
    classes.sort_unstable();
    classes
}

/// Asserts that every part of the transcript on `html` that it shows is
/// drawn in the classes of `docs/specs/components.md`, Transcript: a
/// session's heading, its settings, its entries and their lines, what an
/// entry opens to, a sub-agent's work and its count, and thinking.
#[track_caller]
fn assert_transcript_drawn_as_the_spec_says(html: &Html) {
    let headings = select(html, "#transcript .session > h2");
    assert_ne!(headings.len(), 0, "{}", html.html());
    for heading in headings {
        assert!(
            has_classes(
                heading,
                &["font-semibold", "mt-4", "in-[.session:first-child]:mt-0"]
            ),
            "{}",
            heading.html()
        );
    }
    for settings in select(html, "#transcript .session > .settings") {
        assert!(has_classes(settings, &["text-sm"]), "{}", settings.html());
    }
    for entries in select(html, "#transcript .session > ol.entries") {
        assert!(
            has_classes(entries, &["font-mono", "text-sm"]),
            "{}",
            entries.html()
        );
    }
    for line in select(html, "#transcript .line") {
        assert_eq!(
            classes_starting_with(line, &["hover:", "focus-visible:"]),
            [
                "focus-visible:bg-base-content/10",
                "hover:bg-base-content/10"
            ],
            "{}",
            line.html()
        );
    }
    for block in select(
        html,
        "#transcript pre, #transcript .prompt, #transcript .thinking",
    ) {
        assert!(
            has_classes(
                block,
                &[
                    "bg-base-200",
                    "rounded-box",
                    "px-3",
                    "py-2",
                    "max-h-[30rem]",
                    "overflow-auto",
                    "whitespace-pre-wrap",
                ]
            ),
            "{}",
            block.html()
        );
    }
    for work in select(html, "#transcript .entry ol.entries") {
        assert!(
            has_classes(work, &["border-s-2", "border-base-content/10"]),
            "{}",
            work.html()
        );
    }
    for count in select(html, "#transcript .count") {
        assert_eq!(
            classes_starting_with(count, &["badge"]),
            ["badge", "badge-sm"],
            "{}",
            count.html()
        );
    }
}

/// Classes that would grow cut text in place on hover or on focus, moving
/// what is around it.
fn grows_in_place(element: ElementRef<'_>) -> Vec<&str> {
    element
        .value()
        .classes()
        .filter(|class| {
            [
                "hover:",
                "focus-within:",
                "group-hover:",
                "group-focus-within:",
            ]
            .iter()
            .any(|variant| class.starts_with(variant))
        })
        .collect()
}

/// Asserts that `holder` holds a name cut to fit to `lines` lines, as the
/// stylesheet's cut-name rule cuts it: by its class, with the page's tooltip,
/// and with only its title cut, its number and its link to GitHub outside
/// the title, and nothing in it but elements, each of which the rule sizes
/// (`docs/specs/ui_patterns.md`, Layout).
#[track_caller]
fn assert_cuts_only_the_title(holder: ElementRef, lines: u8) {
    let html = holder.html();
    assert!(
        has_classes(holder, &[&format!("name-cut-{lines}")]),
        "{html}"
    );
    assert!(holder.attr("data-tip-cut").is_some(), "{html}");
    assert_eq!(select_in(holder, ".title").len(), 1, "{html}");
    assert_eq!(select_in(holder, ".number").len(), 1, "{html}");
    assert_eq!(select_in(holder, ":scope > a.github").len(), 1, "{html}");
    assert_eq!(
        select_in(holder, ".title .number, .title a.github").len(),
        0,
        "{html}"
    );
    let loose: Vec<&str> = holder
        .children()
        .filter_map(|child| child.value().as_text())
        .map(|text| &**text)
        .filter(|text| !text.trim().is_empty())
        .collect();
    assert_eq!(loose, Vec::<&str>::new(), "{html}");
}

/// Whether `element` has none of `classes`.
fn lacks_classes(element: ElementRef, classes: &[&str]) -> bool {
    !classes.iter().any(|class| has_classes(element, &[class]))
}

/// The path of the event stream the page connects to.
fn events_path(html: &Html) -> Option<&str> {
    select(html, "#stream")[0].attr("hx-sse:connect")
}

/// The text a reader sees, with its spacing, and its code in backticks, as
/// the commands print them.
fn text(element: ElementRef) -> String {
    fn gather(element: ElementRef, into: &mut String) {
        for child in element.children() {
            if let Some(text) = child.value().as_text() {
                into.push_str(text);
            } else if let Some(child) = ElementRef::wrap(child) {
                let code = child.value().name() == "code";
                if code {
                    into.push('`');
                }
                gather(child, into);
                if code {
                    into.push('`');
                }
            }
        }
    }
    let mut text = String::new();
    gather(element, &mut text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The item page's facts, each label and its value's text, in order.
fn item_facts(html: &Html) -> Vec<(String, String)> {
    let labels = select(html, "#item dl > div > dt");
    let values = select(html, "#item dl > div > dd");
    assert_eq!(labels.len(), values.len(), "{}", html.html());
    labels
        .into_iter()
        .map(text)
        .zip(values.into_iter().map(text))
        .collect()
}

/// #1, ready for Linewright.
fn a_ticket() -> linewright_core::status::View {
    view(vec![ticket(1, "Do the thing")], vec![])
}

async fn with_ticket() -> World {
    let world = World::new().await;
    open_issue_linewright_labeled(
        &world.store,
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-human"],
    );
    world
}

/// An icon of a status (`docs/specs/ui_patterns.md`, Status).
const STATUS_ICON: &str = "svg:is([data-icon=check-circle], [data-icon=x-circle], [data-icon=hand-raised], [data-icon=arrow-path], [data-icon=pause-circle], [data-icon=no-symbol])";

/// Each status's icon beside the words it stands for, in its parent.
fn assert_no_status_by_colour_alone(html: &Html) {
    for icon in select(html, STATUS_ICON) {
        let parent = ElementRef::wrap(icon.parent().unwrap()).unwrap();
        assert!(
            !text(parent).is_empty(),
            "an icon without words: {}",
            icon.html()
        );
    }
}

/// `docs/specs/ui_patterns.md`, Status, A badge's words: a badge within
/// `within` holds no reason or detail, here no words at all.
#[track_caller]
fn assert_badges_hold_no_words(html: &Html, within: &str) {
    for badge in select(html, &format!("{within} .badge")) {
        assert_eq!(text(badge), "", "{}", badge.html());
    }
}

/// The data of each event the stream at `path` sends.
async fn stream(web: &impl Routed, path: &str) -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let response = web.response(path).await;
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut body = response.into_body();
    tokio::spawn(async move {
        let mut unread = String::new();
        while let Some(Ok(frame)) = body.frame().await {
            let Some(bytes) = frame.data_ref() else {
                continue;
            };
            unread.push_str(std::str::from_utf8(bytes).unwrap());
            while let Some(end) = unread.find("\n\n") {
                let block: String = unread.drain(..end + 2).collect();
                let data: Vec<&str> = block
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .collect();
                if !data.is_empty() && sender.send(data.join("\n")).is_err() {
                    return;
                }
            }
        }
    });
    receiver
}

/// The regions an event replaces, by id.
fn regions(event: &str) -> Vec<(String, Html)> {
    let html = Html::parse_fragment(event);
    select(&html, "hx-partial")
        .into_iter()
        .map(|partial| {
            (
                partial.attr("id").unwrap().to_string(),
                Html::parse_fragment(&partial.inner_html()),
            )
        })
        .collect()
}

/// The region `id` of `regions`.
#[track_caller]
fn region_of<'a>(regions: &'a [(String, Html)], id: &str) -> &'a Html {
    regions
        .iter()
        .find(|(region, _)| region == id)
        .map(|(_, html)| html)
        .unwrap_or_else(|| panic!("no {id}: {regions:?}"))
}

/// The column heads of the table of runs, as a reader reads them.
fn runs_heads(html: &Html) -> Vec<String> {
    select(html, "#runs thead th")
        .into_iter()
        .map(text)
        .collect()
}

/// Each row of the table of runs, cell by cell, as a reader reads them.
fn runs_shown(html: &Html) -> Vec<Vec<String>> {
    select(html, "#runs tbody tr")
        .into_iter()
        .map(|row| {
            row.select(&Selector::parse("td, th").unwrap())
                .map(text)
                .collect()
        })
        .collect()
}

/// The filters' names and values, a select's the value of its chosen
/// option.
#[track_caller]
fn filters(html: &Html) -> Vec<(&str, String)> {
    field_values(select(html, "form[data-filters] :is(input, select)"))
}

/// The names and values of `fields`, a select's the value of its chosen
/// option and a textarea's its text.
#[track_caller]
fn field_values(fields: Vec<ElementRef<'_>>) -> Vec<(&str, String)> {
    fields
        .into_iter()
        .map(|field| {
            let value = match field.value().name() {
                "select" => select_in(field, "option[selected]")
                    .into_iter()
                    .filter_map(|option| option.attr("value"))
                    .collect(),
                "textarea" => field.text().collect(),
                _ => field.attr("value").unwrap_or_default().to_string(),
            };
            (field.attr("name").expect("a field's name"), value)
        })
        .collect()
}

/// A making dialog's form (`docs/specs/ui_patterns.md`, Making) as a reader
/// meets it.
#[derive(Debug, PartialEq, Eq)]
struct MakingShown<'a> {
    /// The name and value of each field, as [`field_values`] gives them.
    values: Vec<(&'a str, String)>,
    /// The name of each field Linewright refused, with the reason under it.
    refused: Vec<(&'a str, String)>,
    /// The reason of a refusal of no field, above the buttons.
    whole: Option<String>,
}

/// What `form`, a making dialog's, shows. Each field is a `validator` in a
/// wrapper of its own with its `validator-hint`, a refused one marked
/// `aria-invalid` and described by its hint, and another's hint saying what
/// it says again once the reader edits a refused one; a refusal of no field
/// is the one `alert` before the buttons.
#[track_caller]
fn making_shown(form: &Html) -> MakingShown<'_> {
    let controls = select(
        form,
        "input[name]:not([type=hidden]), select[name], textarea[name]",
    );
    let mut refused = vec![];
    for control in &controls {
        let name = control.attr("name").unwrap();
        assert!(has_classes(*control, &["validator"]), "{}", control.html());
        let wrapper = ElementRef::wrap(control.parent().unwrap()).unwrap();
        assert_eq!(
            select_in(wrapper, ".validator").len(),
            1,
            "{}",
            wrapper.html()
        );
        let hints = select_in(wrapper, ".validator-hint");
        assert_eq!(hints.len(), 1, "{}", wrapper.html());
        let hint = hints[0];
        assert!(
            hint.prev_siblings().any(|sibling| sibling == **control),
            "the hint follows its field: {}",
            wrapper.html()
        );
        match control.attr("aria-invalid") {
            Some("true") => {
                assert_eq!(control.attr("aria-describedby"), hint.attr("id"));
                refused.push((name, text(hint)));
            }
            None => {
                assert_eq!(control.attr("aria-describedby"), None);
                assert_eq!(
                    Some(text(hint).as_str()),
                    hint.attr("data-unrefused-hint"),
                    "{}",
                    wrapper.html()
                );
            }
            Some(other) => panic!("aria-invalid={other:?} on {name}"),
        }
    }
    let alerts = select(form, ".alert");
    assert!(alerts.len() <= 1, "{}", form.html());
    let buttons = select(form, ".modal-action");
    if let (Some(alert), Some(buttons)) = (alerts.first(), buttons.first()) {
        assert!(
            alert.next_siblings().any(|sibling| sibling == **buttons),
            "the refusal of no field is above the buttons: {}",
            form.html()
        );
    }
    MakingShown {
        values: field_values(controls),
        refused,
        whole: alerts.first().map(|alert| text(*alert)),
    }
}

/// The fields of `form`, a making dialog's, that Linewright refused, by
/// name, each with the reason under it, and the reason above its buttons, if
/// any.
#[track_caller]
fn placed(form: &Html) -> (Vec<(&str, String)>, Option<String>) {
    let shown = making_shown(form);
    (shown.refused, shown.whole)
}

/// A table card (`docs/specs/ui_patterns.md`, Tables) as a reader reads
/// it: the name at the left of its top, if it shows one, the labels of the
/// buttons at the right of its top, and the names of the filters in the row
/// under the top, each in order.
#[derive(Debug, PartialEq, Eq)]
struct TableCard {
    name: Option<String>,
    buttons: Vec<String>,
    filters: Vec<String>,
}

/// The table cards on `html`. A card's top, where it has one, comes first,
/// then its filters' row, if it has filters, then the rest.
#[track_caller]
fn table_cards(html: &Html) -> Vec<TableCard> {
    select(html, ".table-card")
        .into_iter()
        .map(|card| {
            let parts: Vec<&str> = card
                .children()
                .filter_map(ElementRef::wrap)
                .map(|part| {
                    let classes: Vec<&str> = part.value().classes().collect();
                    ["table-top", "table-filters"]
                        .into_iter()
                        .find(|kind| classes.contains(kind))
                        .unwrap_or("rest")
                })
                .collect();
            let mut ordered = parts.clone();
            ordered.sort_by_key(|part| match *part {
                "table-top" => 0,
                "table-filters" => 1,
                _ => 2,
            });
            assert_eq!(parts, ordered, "{}", card.html());
            let titles: Vec<String> = select_in(card, ":scope > .table-top > .card-title")
                .into_iter()
                .map(text)
                .collect();
            assert!(titles.len() <= 1, "{titles:?}");
            TableCard {
                name: titles.into_iter().next(),
                buttons: select_in(card, ":scope > .table-top button")
                    .into_iter()
                    .map(text)
                    .collect(),
                filters: select_in(card, ":scope > .table-filters [name]")
                    .into_iter()
                    .map(|field| field.attr("name").unwrap().to_string())
                    .collect(),
            }
        })
        .collect()
}

/// A table card showing `name`, with no buttons and the filters `filters`.
fn named_card(name: &str, filters: &[&str]) -> TableCard {
    TableCard {
        name: Some(name.to_string()),
        buttons: vec![],
        filters: filters.iter().map(ToString::to_string).collect(),
    }
}

/// A table card showing no name, with the buttons `buttons` and the filters
/// `filters`.
fn unnamed_card(buttons: &[&str], filters: &[&str]) -> TableCard {
    TableCard {
        name: None,
        buttons: buttons.iter().map(ToString::to_string).collect(),
        filters: filters.iter().map(ToString::to_string).collect(),
    }
}

/// The links at the ends of the rows of the table `table` on `html`
/// (`docs/specs/ui_patterns.md`, Tables, A row's screen), each as a reader
/// meets it: its text, its accessible name and where it goes.
#[track_caller]
fn views(html: &Html, table: &str) -> Vec<(String, String, String)> {
    select(
        html,
        &format!("{table} tbody tr > :last-child > a.row-view"),
    )
    .into_iter()
    .map(|link| {
        assert!(
            has_classes(link, &["btn", "btn-ghost", "btn-xs"]),
            "{}",
            link.html()
        );
        (
            text(link),
            link.attr("aria-label").unwrap_or_default().to_string(),
            link.attr("href").unwrap().to_string(),
        )
    })
    .collect()
}

/// How each cell of the table `table` on `html` aligns, its head row
/// first, "start" or "end" as its class says, and "none" for a cell that
/// says neither.
#[track_caller]
fn alignments(html: &Html, table: &str) -> Vec<Vec<&'static str>> {
    select(html, &format!("{table} tr"))
        .into_iter()
        .map(|row| {
            select_in(row, "th, td")
                .into_iter()
                .map(|cell| {
                    let classes: Vec<&str> = cell.value().classes().collect();
                    match (
                        classes.contains(&"text-left"),
                        classes.contains(&"text-right"),
                    ) {
                        (true, false) => "start",
                        (false, true) => "end",
                        _ => "none",
                    }
                })
                .collect()
        })
        .collect()
}

/// Whether `rows` holds the run `id` with the outcome `outcome`.
fn has_run(rows: &[Vec<String>], outcome: &str, id: &str) -> bool {
    rows.iter()
        .any(|row| row.len() > 1 && row[0] == outcome && row[1] == id)
}

/// The classes of the elements `selector` matches.
fn classes<'a>(html: &'a Html, selector: &str) -> Vec<Vec<&'a str>> {
    select(html, selector)
        .into_iter()
        .map(|element| element.value().classes().collect())
        .collect()
}

/// Records run `n` of #`item`'s triage as started at `started_at` and never
/// ended nor in flight: interrupted, as when its process died.
#[track_caller]
fn record_interrupted(
    pages: &Pages,
    item: u64,
    n: u32,
    started_at: &str,
) -> impl Future<Output = ()> + use<> {
    let record = serde_json::json!({
        "item": item,
        "stage": "triage",
        "n": n,
        "started_at": started_at,
        "end": null,
        "sessions": [],
    });
    pages.record(&record)
}

/// The pages under a table (`docs/specs/ui_patterns.md`, Tables, Pages), as
/// a reader meets them.
#[derive(Debug, PartialEq)]
struct PagesNav {
    label: String,
    /// Which rows show, of how many.
    showing: String,
    /// "Previous", the numbers, "…" and "Next" in order, the current page's
    /// number marked "*".
    items: Vec<PageLink>,
}

#[derive(Debug, PartialEq)]
struct PageLink {
    text: String,
    /// The page's address and its stream's, `None` where it is no link.
    to: Option<(String, String)>,
}

/// The page link or item `text` leading to `page` and its stream at `stream`.
fn page_link(text: &str, page: &str, stream: &str) -> PageLink {
    PageLink {
        text: text.into(),
        to: Some((page.into(), stream.into())),
    }
}

/// The page item `text` that is no link: a disabled end, or "…".
fn no_link(text: &str) -> PageLink {
    PageLink {
        text: text.into(),
        to: None,
    }
}

/// The pages under the table in `within` on `html`, if it has any. Each item
/// is a small button in a daisyUI `join`, and which rows show is in secondary
/// text.
#[track_caller]
fn pages_nav(html: &Html, within: &str) -> Option<PagesNav> {
    let navs = select(html, &format!("{within} nav.pages"));
    assert!(navs.len() <= 1, "{}", html.html());
    let nav = *navs.first()?;
    assert_in_secondary_text(html, &format!("{within} nav.pages .pages-showing"));
    let showing = select_in(nav, ".pages-showing");
    let items = select_in(nav, ".join > *")
        .into_iter()
        .map(|item| {
            assert!(
                has_classes(item, &["join-item", "btn", "btn-sm"]),
                "{}",
                item.html()
            );
            let current = item.attr("aria-current") == Some("page");
            PageLink {
                text: format!("{}{}", text(item), if current { "*" } else { "" }),
                to: (item.value().name() == "a").then(|| {
                    (
                        item.attr("href").unwrap().to_string(),
                        item.attr("data-stream").unwrap().to_string(),
                    )
                }),
            }
        })
        .collect();
    Some(PagesNav {
        label: nav.attr("aria-label").unwrap_or_default().to_string(),
        showing: text(showing[0]),
        items,
    })
}
