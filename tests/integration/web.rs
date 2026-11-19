use crate::common;

mod automations;
mod benchmark_runs;
mod benchmarks;
mod board;
mod capture;
mod environment;
mod frame;
mod item;
mod run;
mod runs;
mod script;
mod serving;
mod shell;

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use common::*;
use http_body_util::BodyExt as _;
use linewright::api::Sources;
use linewright::clock::Clock;
use linewright::host::UiHosts;
use linewright::web;
use linewright::webhook::{Listener, Pending};
use scraper::{ElementRef, Html, Selector};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

/// `serve` polling hourly, so its view holds still but for deliveries, and
/// its router driven in-process.
struct Web {
    serving: Serving,
    listener: Listener,
    router: Router,
    stopping: CancellationToken,
    store: std::sync::Arc<linewright::store::local::LocalStore>,
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
    since: linewright::domain::Timestamp,
    served: tokio::sync::watch::Receiver<Option<linewright::api::Served>>,
    polling: tokio::sync::watch::Receiver<Option<linewright::status::Polling>>,
) -> impl Future<Output = Sources> + use<> {
    let runs = runs_in(&work_dir(dir));
    let (clock, base) = (clock.clone(), dir.path().join("work"));
    async move {
        Sources {
            serving: linewright::status::Serving { since, max_runs: 4 },
            served,
            polling,
            deliveries: None,
            clock,
            work_dir: linewright::runs::RepoWorkDir {
                base,
                repo: REPO.parse().unwrap(),
            },
            runs: runs.await,
            ends: linewright::ending::Ends::new(),
            environment: linewright::api::EnvironmentLink::default(),
            benchmark_runs: linewright::api::BenchmarkRunsLink::default(),
            exiting: linewright::winding_up::WindingUp::default().exiting(),
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
        let router = web::router(sources, UiHosts::default(), stopping.clone());
        Web {
            serving,
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

    async fn factory_beside(&self) -> linewright::factory::Factory {
        linewright::factory::Factory::start(
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
}

/// The web UI's router driven in-process over a view built by hand over a
/// local store, with no factory and no poll.
struct Pages {
    router: Router,
    sources: Sources,
    served: tokio::sync::watch::Sender<Option<linewright::api::Served>>,
    /// Set by [`Pages::with_deliveries`].
    listener: Option<Listener>,
    store: std::sync::Arc<linewright::store::local::LocalStore>,
    stopping: CancellationToken,
    dir: tempfile::TempDir,
}

impl Pages {
    /// The pages of `view`, as `serve` shows them once a poll has made it.
    async fn of(view: linewright::status::View) -> Pages {
        Pages::over(view, false).await
    }

    /// [`Pages::of`]'s, with webhooks on: a bare webhook listener, that
    /// nothing takes the deliveries of, tallies them for the pages.
    async fn with_deliveries(view: linewright::status::View) -> Pages {
        Pages::over(view, true).await
    }

    async fn over(view: linewright::status::View, deliveries: bool) -> Pages {
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
        let polling = linewright::status::Polling {
            next: view.polls.next,
            failed: None,
        };
        let served = Some(linewright::api::Served::new(
            view,
            store.clone(),
            linewright::redaction::Redaction::default(),
            std::sync::Arc::new(linewright::briefs::Briefs::shipped()),
        ));
        let (served, served_receiver) = tokio::sync::watch::channel(served);
        let (_, polling) = tokio::sync::watch::channel(Some(polling));
        let since = clock.now();
        let sources = Sources {
            deliveries: listener.as_ref().map(Listener::tally),
            ..sources(&dir, &clock, since, served_receiver, polling).await
        };
        let stopping = CancellationToken::new();
        let router = web::router(sources.clone(), UiHosts::default(), stopping.clone());
        Pages {
            router,
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
    fn publish(&self, view: linewright::status::View) {
        let served = linewright::api::Served::new(
            view,
            self.store.clone(),
            linewright::redaction::Redaction::default(),
            std::sync::Arc::new(linewright::briefs::Briefs::shipped()),
        );
        self.served.send_replace(Some(served));
    }

    /// The web UI listening on a port of its own, answering `hosts` besides
    /// the local names and IP addresses.
    async fn listen(&self, hosts: &[&str]) -> web::Listener {
        let ui = web::Ui {
            address: "127.0.0.1:0".parse().unwrap(),
            hosts: UiHosts::new(hosts.iter().copied()).unwrap(),
        };
        web::Listener::bind(&ui, self.sources.clone())
            .await
            .unwrap()
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

    async fn response(&self, path: &str) -> axum::response::Response {
        routed(self.router(), hyper::Method::GET, Some("localhost"), path).await
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

/// The form of a name of a benchmark, a task or a configuration, as the
/// `pattern` of the inputs that take one (`docs/specs/benchmarks.md`,
/// Capturing).
const NAME_PATTERN: &str = r"[a-z0-9][a-z0-9\-]*";

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
        .header("Host", "localhost:8080")
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

/// A confirmation (`docs/specs/design_system.md`, Confirming) as a reader
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

/// Asserts that `selector` selects something in `html`, all of it in Muted
/// text and in no translucent text colour: `docs/specs/web/design_system.md`,
/// Colour scheme: secondary text is in Muted text, and the web UI draws in
/// no colour but its own.
fn assert_in_muted_text(html: &Html, selector: &str) {
    let selected = select(html, selector);
    assert!(!selected.is_empty(), "{selector}");
    for element in selected {
        assert!(
            has_classes(element, &["text-muted"])
                && !element
                    .value()
                    .classes()
                    .any(|class| class.starts_with("text-") && class.contains('/')),
            "{}",
            element.html()
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
/// (`docs/specs/design_system.md`, Layout).
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
    let labels = select(html, "#item dl > dt");
    let values = select(html, "#item dl > dd");
    assert_eq!(labels.len(), values.len(), "{}", html.html());
    labels
        .into_iter()
        .map(text)
        .zip(values.into_iter().map(text))
        .collect()
}

/// #1, ready for Linewright.
fn a_ticket() -> linewright::status::View {
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

/// Each icon beside the words it stands for.
fn assert_no_status_by_colour_alone(html: &Html) {
    for icon in select(html, "svg.icon") {
        let words = ElementRef::wrap(icon.parent().unwrap()).map(text);
        assert!(
            words.is_some_and(|words| !words.is_empty()),
            "an icon without words: {}",
            icon.html()
        );
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
/// option.
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
                _ => field.attr("value").unwrap_or_default().to_string(),
            };
            (field.attr("name").expect("a field's name"), value)
        })
        .collect()
}

/// The table cards on `html` (`docs/specs/design_system.md`, Tables), each
/// as a reader reads its bar: the name at its left, then the names of the
/// filters and the labels of the buttons at its right, in order.
#[track_caller]
fn table_bars(html: &Html) -> Vec<(String, Vec<String>)> {
    select(html, ".table-card > .table-bar")
        .into_iter()
        .map(|bar| {
            let name = select_in(bar, ".card-title")
                .into_iter()
                .map(text)
                .collect();
            let right = select_in(bar, "[name], button")
                .into_iter()
                .map(|field| match field.attr("name") {
                    Some(field_name) => field_name.to_string(),
                    None => text(field),
                })
                .collect();
            (name, right)
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
