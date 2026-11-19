use super::*;

use std::panic::Location;

use serde_json::json;

/// The run page of #1's implement run.
const RUN_PAGE: &str = "/runs/1/implement-1";

/// Criteria a capture is given.
const CRITERIA: &str = "A good result ships gh.\n";

/// The benchmark of implement tasks #1's run is captured into.
const IMPLEMENTS: &str = "implements";

/// A benchmark of design tasks.
const DESIGNS: &str = "designs";

/// What run 1 recorded of its input.
enum Recorded {
    Nothing,
    WithoutProperties,
    Whole,
}

/// #1, `Ship gh`, with its implement run ended, recorded as run 1, which
/// recorded of its input what `recorded` says, the default branch's head in
/// the store among it.
#[track_caller]
fn implemented(recorded: Recorded) -> impl Future<Output = Pages> {
    let caller = Location::caller();
    async move {
        let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
        keep_benchmark(&pages.path(""), IMPLEMENTS, "implement").await;
        keep_benchmark(&pages.path(""), DESIGNS, "design").await;
        pages
            .record(&json!({
                "item": 1,
                "stage": "implement",
                "n": 1,
                "started_at": "2026-10-13T10:09:00Z",
                "end": {"at": "2026-10-13T10:13:00Z", "ending": "done"},
                "sessions": [],
            }))
            .await;
        let properties = match recorded {
            Recorded::Nothing => return pages,
            Recorded::WithoutProperties => None,
            Recorded::Whole => Some(properties()),
        };
        let head = pages
            .store
            .commit("maintainer", "main", &[("README.md", "Gadgets\n")], "Start")
            .await
            .unwrap_or_else(|e| panic!("committing on main at {caller}: {e:#}"));
        record_input(&pages, &head, properties).await;
        pages
    }
}

/// Properties as a run records them.
fn properties() -> serde_json::Value {
    json!({"resources": {"account": {"login": "factory", "id": 7}}})
}

/// Writes the input #1's implement run 1 recorded: `main` at `head`, with
/// `properties`, if any.
#[track_caller]
fn record_input(
    pages: &Pages,
    head: &str,
    properties: Option<serde_json::Value>,
) -> impl Future<Output = ()> + use<> {
    let mut input = json!({
        "api_version": linewright::store::API_VERSION,
        "start": "2026-10-13T10:09:00Z",
        "items": [{"number": 1, "resources": {"issues/1": {"number": 1, "title": "Ship gh"}}}],
        "heads": {"main": head},
    });
    if let Some(properties) = properties {
        input["properties"] = properties;
    }
    pages.input("1/implement-1", &input)
}

/// The capture form on `html`, if it shows one.
#[track_caller]
fn capture_form(html: &Html) -> Option<ElementRef<'_>> {
    let forms = select(html, "form.capture");
    assert!(forms.len() <= 1, "more than one capture form");
    forms.into_iter().next()
}

#[test_log::test(tokio::test)]
async fn a_run_that_can_be_captured_offers_a_form_with_the_suggested_name() {
    let pages = implemented(Recorded::Whole).await;

    let html = pages.page(RUN_PAGE).await;

    let form = capture_form(&html).expect("a capture form");
    assert_eq!(form.attr("hx-post"), Some(RUN_PAGE));
    let name = select_in(form, "input[name=name]");
    assert_eq!(name[0].attr("value"), Some("1-implement-1"));
    assert_eq!(name[0].attr("pattern"), Some(NAME_PATTERN));
    let criteria = select_in(form, "textarea[name=criteria]");
    assert_eq!(criteria.len(), 1);
    assert!(criteria[0].attr("required").is_some());
    assert_eq!(
        disclosures(&html, &["capture"]),
        [("Add case".to_string(), false, "Add case".to_string())]
    );
}

#[test_log::test(tokio::test)]
async fn a_run_with_no_input_offers_no_capture() {
    let pages = implemented(Recorded::Nothing).await;

    let html = pages.page(RUN_PAGE).await;

    assert!(capture_form(&html).is_none());
}

#[test_log::test(tokio::test)]
async fn a_run_whose_record_lacks_the_properties_offers_no_capture_and_refuses_one_naming_them() {
    let pages = implemented(Recorded::WithoutProperties).await;

    let html = pages.page(RUN_PAGE).await;
    let (status, answer) = submit(&pages, "gadgets", CRITERIA).await;

    assert!(capture_form(&html).is_none());
    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: the run 1/implement-1's record lacks the properties, so it cannot be \
         captured"
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

/// Submits the capture form of #1's implement run, naming the task `name`
/// with `criteria`, into the benchmark [`IMPLEMENTS`], as a page of the web
/// UI's own does: the status and the fragment it answers with.
#[track_caller]
fn submit<'a>(
    pages: &'a Pages,
    name: &'a str,
    criteria: &'a str,
) -> impl Future<Output = (u16, Html)> + 'a {
    submit_into(pages, Chosen::Benchmark(IMPLEMENTS), name, criteria)
}

/// The benchmark a capture goes into.
#[derive(Clone, Copy)]
enum Chosen<'a> {
    Benchmark(&'a str),
    New(&'a str),
}

/// [`submit`], into the benchmark `into`.
#[track_caller]
fn submit_into(
    pages: &Pages,
    into: Chosen<'_>,
    name: &str,
    criteria: &str,
) -> impl Future<Output = (u16, Html)> + use<> {
    post_capture(pages, &[SAME_ORIGIN], into, name, criteria)
}

/// Posts the capture form of #1's implement run with `headers` besides its
/// `Host`.
#[track_caller]
fn post_capture(
    pages: &Pages,
    headers: &[(&str, &str)],
    into: Chosen<'_>,
    name: &str,
    criteria: &str,
) -> impl Future<Output = (u16, Html)> + use<> {
    let (benchmark, new_benchmark) = match into {
        Chosen::Benchmark(benchmark) => (benchmark, ""),
        Chosen::New(benchmark) => ("", benchmark),
    };
    post(
        pages,
        RUN_PAGE,
        headers,
        &[
            ("suite", benchmark),
            ("new-suite", new_benchmark),
            ("name", name),
            ("criteria", criteria),
        ],
    )
}

#[test_log::test(tokio::test)]
async fn submitting_the_form_writes_the_task_and_says_where() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit(&pages, "gadgets", "A good result ships gh.\n").await;

    assert_eq!(status, 200, "{}", said(&answer));
    let dir = pages.path("cases/gadgets");
    assert_eq!(
        said(&answer),
        format!(
            "Captured 1/implement-1 as the case `{}` in the suite implements",
            dir.display()
        )
    );
    assert_eq!(tasks(&pages), ["gadgets"]);
    assert_eq!(
        std::fs::read_to_string(dir.join("criteria.md")).unwrap(),
        "A good result ships gh.\n"
    );
}

#[test_log::test(tokio::test)]
async fn a_capture_with_blank_criteria_is_refused_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit(&pages, "gadgets", " \n\t\n").await;

    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: the criteria are required: what a good result is, not only white space"
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_taken_name_is_refused_and_says_so() {
    let pages = implemented(Recorded::Whole).await;
    submit(&pages, "gadgets", CRITERIA).await;

    let (status, answer) = submit(&pages, "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: a case is already named gadgets"
    );
}

#[test_log::test(tokio::test)]
async fn a_name_that_is_not_a_tasks_is_refused_and_says_so() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit(&pages, "../Gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: \"../Gadgets\" is not a case's name: it is lowercase letters, digits and hyphens, starting with a letter or a digit"
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_failed_capture_writes_nothing_and_says_why() {
    let pages = implemented(Recorded::Nothing).await;
    let missing = "0123456789abcdef0123456789abcdef01234567";
    record_input(&pages, missing, Some(properties())).await;

    let (status, answer) = submit(&pages, "gadgets", CRITERIA).await;

    assert_eq!(status, 500);
    let said = said(&answer);
    assert!(said.starts_with("Not captured: "), "{said}");
    assert!(said.contains(missing), "{said}");
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_cross_site_capture_is_refused_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, _) = post_capture(
        &pages,
        &[CROSS_SITE],
        Chosen::Benchmark(IMPLEMENTS),
        "gadgets",
        CRITERIA,
    )
    .await;

    assert_eq!(status, 403);
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn before_the_first_poll_the_form_says_the_capture_cant_run_yet() {
    let pages = implemented(Recorded::Whole).await;
    let view = pages.served.send_replace(None).unwrap().view();

    let html = pages.page(RUN_PAGE).await;
    let form = capture_form(&html).unwrap();
    assert_eq!(
        text(select_in(form, "#capture-ready span")[0]),
        "the capture can't run before Linewright's first poll"
    );
    assert!(
        select_in(form, "button[type=submit]")[0]
            .attr("disabled")
            .is_some()
    );
    let (status, answer) = submit(&pages, "gadgets", CRITERIA).await;
    assert_eq!(status, 503);
    assert_eq!(
        said(&answer),
        "Not captured: the capture can't run before Linewright's first poll"
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());

    let mut events = stream(&pages, "/events/runs/1/implement-1").await;
    events.recv().await.unwrap();
    pages.publish(view);
    let ready = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, ready)) = regions(&event)
            .into_iter()
            .find(|(id, _)| id == "capture-ready")
        {
            break ready;
        }
    };
    assert_eq!(select(&ready, "#capture-ready span").len(), 0);
    assert!(
        select(&ready, "button[type=submit]")[0]
            .attr("disabled")
            .is_none()
    );
}

#[test_log::test(tokio::test)]
async fn the_form_offers_the_benchmarks_of_the_runs_stage_or_a_new_one() {
    let pages = implemented(Recorded::Whole).await;
    keep_benchmark(&pages.path(""), "board", "implement").await;

    let html = pages.page(RUN_PAGE).await;

    let form = capture_form(&html).expect("a capture form");
    let options: Vec<(Option<&str>, String)> = select_in(form, "select[name=suite] option")
        .into_iter()
        .map(|option| (option.attr("value"), text(option)))
        .collect();
    assert_eq!(
        options,
        [
            (Some("board"), "board".to_string()),
            (Some(IMPLEMENTS), IMPLEMENTS.to_string()),
            (Some(""), "a new suite".to_string()),
        ]
    );
    let new_benchmark = select_in(form, "input[name=new-suite]");
    assert_eq!(new_benchmark[0].attr("pattern"), Some(NAME_PATTERN));
}

#[test_log::test(tokio::test)]
async fn a_capture_into_a_new_benchmark_makes_it_and_says_so() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit_into(&pages, Chosen::New("fresh"), "gadgets", CRITERIA).await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(
        said(&answer),
        format!(
            "Captured 1/implement-1 as the case `{}` in the new suite fresh",
            pages.path("cases/gadgets").display()
        )
    );
    let refreshed = select(&answer, "select#capture-suite[hx-swap-oob]");
    assert_eq!(refreshed.len(), 1, "the form's suites refreshed");
    let options: Vec<(Option<&str>, bool)> = select_in(refreshed[0], "option")
        .into_iter()
        .map(|option| (option.attr("value"), option.attr("selected").is_some()))
        .collect();
    assert_eq!(
        options,
        [
            (Some("fresh"), true),
            (Some(IMPLEMENTS), false),
            (Some(""), false)
        ]
    );
    let html = pages.page(RUN_PAGE).await;
    let form = capture_form(&html).unwrap();
    assert_eq!(
        select_in(form, "select[name=suite] option[value=fresh]").len(),
        1
    );
}

#[test_log::test(tokio::test)]
async fn a_capture_into_a_benchmark_of_another_stage_is_refused_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) =
        submit_into(&pages, Chosen::Benchmark(DESIGNS), "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: the suite designs holds cases of design, and the run 1/implement-1 is of \
         implement"
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_new_benchmark_without_a_name_is_refused_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit_into(&pages, Chosen::New(""), "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(said(&answer), "Not captured: the new suite needs a name");
    assert_eq!(tasks(&pages), Vec::<String>::new());
}
