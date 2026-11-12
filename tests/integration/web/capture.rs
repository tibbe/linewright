use super::*;

use std::panic::Location;

use serde_json::json;

/// The run page of #1's implement run.
const RUN_PAGE: &str = "/runs/1/implement-1";

/// #1, `Ship gh`, with its implement run ended, recorded as run 1, which
/// recorded its input when `recorded_input`, the default branch's head in
/// the store among it.
#[track_caller]
fn implemented(recorded_input: bool) -> impl Future<Output = Pages> {
    let caller = Location::caller();
    async move {
        let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
        pages.record(&json!({
            "item": 1,
            "stage": "implement",
            "n": 1,
            "started_at": "2026-10-13T10:09:00Z",
            "end": {"at": "2026-10-13T10:13:00Z", "ending": "done"},
            "sessions": [],
        }));
        if recorded_input {
            let head = pages
                .store
                .commit("maintainer", "main", &[("README.md", "Gadgets\n")], "Start")
                .await
                .unwrap_or_else(|e| panic!("committing on main at {caller}: {e:#}"));
            record_input(&pages, &head);
        }
        pages
    }
}

/// Writes the input #1's implement run 1 recorded: `main` at `head`.
#[track_caller]
fn record_input(pages: &Pages, head: &str) {
    let input = json!({
        "api_version": linewright::store::API_VERSION,
        "start": "2026-10-13T10:09:00Z",
        "items": [{"number": 1, "resources": {"issues/1": {"number": 1, "title": "Ship gh"}}}],
        "heads": {"main": head},
    });
    pages.write(&[("runs/items/1/implement-1.input.json", &input.to_string())]);
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
    let pages = implemented(true).await;

    let html = pages.page(RUN_PAGE).await;

    let form = capture_form(&html).expect("a capture form");
    assert_eq!(form.attr("hx-post"), Some(RUN_PAGE));
    let name = select_in(form, "input[name=name]");
    assert_eq!(name[0].attr("value"), Some("1-implement-1"));
    assert_eq!(select_in(form, "textarea[name=criteria]").len(), 1);
}

#[test_log::test(tokio::test)]
async fn a_run_with_no_input_offers_no_capture() {
    let pages = implemented(false).await;

    let html = pages.page(RUN_PAGE).await;

    assert!(capture_form(&html).is_none());
}

/// Submits the capture form of #1's implement run, naming the case `name`
/// with `criteria`, as a page of the web UI's own does: the status and the
/// fragment it answers with.
#[track_caller]
fn submit<'a>(
    pages: &'a Pages,
    name: &'a str,
    criteria: &'a str,
) -> impl Future<Output = (u16, Html)> + 'a {
    let posted = post(pages, &[("sec-fetch-site", "same-origin")], name, criteria);
    async move {
        let (status, body) = posted.await;
        (status, Html::parse_fragment(&body))
    }
}

/// Posts the capture form of #1's implement run with `headers` besides its
/// `Host`.
#[track_caller]
fn post<'a>(
    pages: &'a Pages,
    headers: &[(&str, &str)],
    name: &str,
    criteria: &str,
) -> impl Future<Output = (u16, String)> + 'a {
    let caller = Location::caller();
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("name", name)
        .append_pair("criteria", criteria)
        .finish();
    let mut request = axum::http::Request::builder()
        .method(hyper::Method::POST)
        .uri(RUN_PAGE)
        .header("Host", "localhost:8080")
        .header("Content-Type", "application/x-www-form-urlencoded");
    for (header, value) in headers {
        request = request.header(*header, *value);
    }
    let request = request
        .body(Body::from(body))
        .unwrap_or_else(|e| panic!("building the request at {caller}: {e}"));
    let router = pages.router.clone();
    async move {
        let Ok(response) = router.oneshot(request).await;
        status_and_body(response).await
    }
}

/// The cases under `pages`' work directory, by name.
#[track_caller]
fn cases(pages: &Pages) -> Vec<String> {
    match std::fs::read_dir(pages.path("cases")) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => panic!("reading the cases: {e}"),
    }
}

/// What a fragment says, as a reader reads it.
fn said(html: &Html) -> String {
    text(html.root_element())
}

#[test_log::test(tokio::test)]
async fn submitting_the_form_writes_the_case_and_says_where() {
    let pages = implemented(true).await;

    let (status, answer) = submit(&pages, "gadgets", "A good result ships gh.\n").await;

    assert_eq!(status, 200, "{}", said(&answer));
    let dir = pages.path("cases/gadgets");
    assert_eq!(
        said(&answer),
        format!("Captured 1/implement-1 as the case `{}`", dir.display())
    );
    assert_eq!(cases(&pages), ["gadgets"]);
    assert_eq!(
        std::fs::read_to_string(dir.join("criteria.md")).unwrap(),
        "A good result ships gh.\n"
    );
}

#[test_log::test(tokio::test)]
async fn a_capture_without_criteria_writes_none() {
    let pages = implemented(true).await;

    let (status, answer) = submit(&pages, "gadgets", "").await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert!(!pages.path("cases/gadgets/criteria.md").exists());
}

#[test_log::test(tokio::test)]
async fn a_taken_name_is_refused_and_says_so() {
    let pages = implemented(true).await;
    submit(&pages, "gadgets", "").await;

    let (status, answer) = submit(&pages, "gadgets", "").await;

    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: a case is already named gadgets"
    );
}

#[test_log::test(tokio::test)]
async fn a_name_that_is_not_a_cases_is_refused_and_says_so() {
    let pages = implemented(true).await;

    let (status, answer) = submit(&pages, "../Gadgets", "").await;

    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: \"../Gadgets\" is not a case's name: it is lowercase letters, digits and hyphens"
    );
    assert_eq!(cases(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_failed_capture_writes_nothing_and_says_why() {
    let pages = implemented(false).await;
    let missing = "0123456789abcdef0123456789abcdef01234567";
    record_input(&pages, missing);

    let (status, answer) = submit(&pages, "gadgets", "").await;

    assert_eq!(status, 500);
    let said = said(&answer);
    assert!(said.starts_with("Not captured: "), "{said}");
    assert!(said.contains(missing), "{said}");
    assert_eq!(cases(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_cross_site_capture_is_refused_and_writes_nothing() {
    let pages = implemented(true).await;

    let (status, _) = post(&pages, &[("sec-fetch-site", "cross-site")], "gadgets", "").await;

    assert_eq!(status, 403);
    assert_eq!(cases(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn before_the_first_poll_the_form_says_the_capture_cant_run_yet() {
    let pages = implemented(true).await;
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
    let (status, answer) = submit(&pages, "gadgets", "").await;
    assert_eq!(status, 503);
    assert_eq!(
        said(&answer),
        "Not captured: the capture can't run before Linewright's first poll"
    );
    assert_eq!(cases(&pages), Vec::<String>::new());

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
