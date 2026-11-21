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

/// The capture's form as a reader meets it: the value of each field, a
/// select's its chosen option's, and the refusal above its buttons, if any.
type FormShown<'a> = (Vec<(&'a str, String)>, Option<String>);

/// [`FormShown`] of the capture's form `form`.
#[track_caller]
fn form_shown(form: &Html) -> FormShown<'_> {
    let refusals = select(form, ".alert-error");
    assert!(refusals.len() <= 1, "{}", form.html());
    let mut fields = field_values(select(form, "input[name], select[name]"));
    fields.extend(
        select(form, "textarea[name]")
            .into_iter()
            .map(|area| (area.attr("name").unwrap(), area.text().collect())),
    );
    (fields, refusals.first().map(|refusal| text(*refusal)))
}

/// The capture's form with `benchmark` chosen, `new_benchmark` named,
/// `name` and `criteria`, and `refusal`, if any.
fn shown<'a>(
    benchmark: &str,
    new_benchmark: &str,
    name: &str,
    criteria: &str,
    refusal: Option<&str>,
) -> FormShown<'a> {
    (
        vec![
            ("benchmark", benchmark.to_string()),
            ("new-benchmark", new_benchmark.to_string()),
            ("name", name.to_string()),
            ("criteria", criteria.to_string()),
        ],
        refusal.map(str::to_string),
    )
}

/// [`placed`] for a refusal of field `name` alone, for `reason`.
fn under<'a>(name: &'a str, reason: &str) -> (Vec<(&'a str, String)>, Option<String>) {
    (vec![(name, reason.to_string())], None)
}

/// [`placed`] for a refusal of no field, for `reason`.
fn above_buttons<'a>(reason: &str) -> (Vec<(&'a str, String)>, Option<String>) {
    (vec![], Some(reason.to_string()))
}

/// `docs/specs/runs.md`, Run, Capturing; `docs/specs/design_system.md`,
/// Making, The dialog.
#[test_log::test(tokio::test)]
async fn add_task_opens_a_dialog_titled_add_task_to_a_benchmark_each_field_under_its_label() {
    let pages = implemented(Recorded::Whole).await;

    let html = pages.page(RUN_PAGE).await;

    let opening = select(&html, "#buttons button[command=show-modal]");
    assert_eq!(opening.len(), 1, "{}", html.html());
    assert_eq!(text(opening[0]), "Add task");
    assert_eq!(opening[0].attr("commandfor"), Some("capture"));
    let dialog = select(&html, "dialog#capture.modal")[0];
    let legend = select_in(dialog, "form fieldset > legend.fieldset-legend");
    assert_eq!(text(legend[0]), "Add task to a benchmark");
    assert_eq!(dialog.attr("aria-labelledby"), legend[0].attr("id"));
    let form = capture_form(&html).expect("a capture form");
    assert_eq!(select_in(dialog, "form.capture").first(), Some(&form));
    assert_eq!(form.attr("hx-post"), Some(RUN_PAGE));
    assert_eq!(form.attr("hx-target"), Some("this"));
    let labels: Vec<String> = select_in(form, "fieldset label")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(labels, ["Benchmark", "Benchmark name", "Name", "Criteria"]);
    for label in select_in(form, "fieldset label") {
        let field = label.attr("for").unwrap();
        assert_eq!(
            select_in(form, &format!("[id='{field}']")).len(),
            1,
            "{field}"
        );
    }
    let fragment = Html::parse_fragment(&form.inner_html());
    assert_eq!(
        form_shown(&fragment),
        shown(IMPLEMENTS, "", "1-implement-1", "", None)
    );
    assert_eq!(placed(&fragment), (vec![], None));
    let required: Vec<(&str, Option<&str>)> = select(&fragment, "[name][required]")
        .into_iter()
        .map(|field| (field.attr("name").unwrap(), field.attr("pattern")))
        .collect();
    assert_eq!(
        required,
        [
            ("new-benchmark", Some(r".*\S.*")),
            ("name", Some(r".*\S.*")),
            ("criteria", None),
        ]
    );
    let hints: Vec<String> = select(&fragment, ".validator-hint")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(hints, ["Required."; 4]);
    assert_eq!(select(&fragment, ".optional").len(), 0);
    let actions: Vec<String> = select_in(form, ".modal-action button")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(actions, ["Cancel", "Add task"]);
}

/// `docs/specs/runs.md`, Run, Capturing: "Benchmark name" shows while "New
/// benchmark" is chosen, as the stylesheet hides the `.new-benchmark-name`
/// of a capture's form without it, its label, hint and all; and it is
/// disabled while hidden, so that the browser neither checks nor sends it.
#[test_log::test(tokio::test)]
async fn the_benchmark_name_is_hidden_and_disabled_but_with_new_benchmark_chosen() {
    let pages = implemented(Recorded::Whole).await;

    let html = pages.page(RUN_PAGE).await;
    let (_, refused) = submit_into(&pages, Chosen::New("Fresh"), "gadgets", CRITERIA).await;

    let form = capture_form(&html).unwrap();
    let hidden = select_in(form, ".new-benchmark-name");
    assert_eq!(hidden.len(), 1, "{}", form.html());
    let held: Vec<&str> = select_in(hidden[0], "label, [name], .validator-hint")
        .into_iter()
        .map(|element| element.value().name())
        .collect();
    assert_eq!(held, ["label", "input", "p"]);
    let new_benchmark = select_in(form, "select[name=benchmark] option.new-benchmark");
    assert_eq!(new_benchmark.len(), 1);
    assert_eq!(new_benchmark[0].attr("value"), Some(""));
    let disabled = |holder: ElementRef<'_>| {
        select_in(holder, "input[name=new-benchmark]")[0]
            .attr("disabled")
            .is_some()
    };
    assert!(disabled(form));
    assert!(!disabled(refused.root_element()), "{}", refused.html());
}

#[test_log::test(tokio::test)]
async fn the_capture_dialog_is_drawn_outside_every_region_the_stream_sends() {
    let pages = implemented(Recorded::Whole).await;
    let html = pages.page(RUN_PAGE).await;
    let mut events = stream(&pages, events_path(&html).unwrap()).await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let first = regions(&first);
    for (id, region) in &first {
        assert_eq!(select(region, "dialog, form.capture").len(), 0, "{id}");
    }
    let (_, buttons) = first.iter().find(|(id, _)| id == "buttons").unwrap();
    assert_eq!(
        select(buttons, "button[commandfor=capture]")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["Add task"]
    );
}

/// "Add task" opens the dialog its page drew, so a stream offers it only to
/// a page that drew it.
#[test_log::test(tokio::test)]
async fn the_stream_of_a_run_page_without_the_capture_offers_no_add_task() {
    let pages = implemented(Recorded::Whole).await;
    let mut events = stream(&pages, &format!("/events{RUN_PAGE}")).await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let first = regions(&first);
    let (_, buttons) = first.iter().find(|(id, _)| id == "buttons").unwrap();
    assert_eq!(select(buttons, "button"), [] as [ElementRef<'_>; 0]);
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
        placed(&answer),
        above_buttons("The record of run 1/implement-1 lacks properties.")
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

impl<'a> Chosen<'a> {
    /// The form's `benchmark` and `new-benchmark` that choose it.
    fn fields(self) -> (&'a str, &'a str) {
        match self {
            Chosen::Benchmark(benchmark) => (benchmark, ""),
            Chosen::New(benchmark) => ("", benchmark),
        }
    }
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
    post(pages, RUN_PAGE, headers, &fields(into, name, criteria))
}

/// The capture form's fields, into `into`, naming the task `name` with
/// `criteria`.
fn fields<'a>(into: Chosen<'a>, name: &'a str, criteria: &'a str) -> [(&'a str, &'a str); 4] {
    let (benchmark, new_benchmark) = into.fields();
    [
        ("benchmark", benchmark),
        ("new-benchmark", new_benchmark),
        ("name", name),
        ("criteria", criteria),
    ]
}

/// What a capture into `into` answers once made: its status, whether it
/// closes the dialog, the form it answers with beside the message it sends
/// to the screen, and that message, a success alert announced as a status,
/// with where its link leads.
#[track_caller]
fn made(
    pages: &Pages,
    into: Chosen<'_>,
) -> impl Future<Output = (u16, bool, Html, Vec<(String, Option<String>)>)> + use<> {
    let answer = post_answer(
        pages,
        RUN_PAGE,
        &[SAME_ORIGIN],
        &fields(into, "gadgets", CRITERIA),
    );
    async move {
        let (status, headers, answer) = answer.await;
        let closes = headers.get("hx-trigger").is_some_and(|made| made == "made");
        let messages = select(
            &answer,
            r#"#made[hx-swap-oob="innerHTML"] > .alert.alert-success[role=status]"#,
        )
        .into_iter()
        .map(|alert| {
            let link = select_in(alert, "a").first().and_then(|a| a.attr("href"));
            (text(alert), link.map(str::to_string))
        })
        .collect();
        (status, closes, answer, messages)
    }
}

/// `docs/specs/runs.md`, Run, Capturing, What was made;
/// `docs/specs/design_system.md`, Making.
#[test_log::test(tokio::test)]
async fn a_capture_closes_the_dialog_and_says_under_the_heading_what_it_made_linking_to_the_benchmarks_tasks()
 {
    let pages = implemented(Recorded::Whole).await;

    let (status, closes, form, messages) = made(&pages, Chosen::Benchmark(IMPLEMENTS)).await;

    assert_eq!(status, 200, "{}", form.html());
    assert!(closes);
    assert_eq!(
        messages,
        [(
            "Captured run 1/implement-1 as the task gadgets in the benchmark implements.".into(),
            Some("/benchmarks/implements/tasks".into())
        )]
    );
    assert_eq!(
        form_shown(&form),
        shown(IMPLEMENTS, "", "1-implement-1", "", None)
    );
    assert_eq!(tasks(&pages), ["gadgets"]);
    assert_eq!(
        std::fs::read_to_string(pages.path("tasks/gadgets").join("criteria.md")).unwrap(),
        CRITERIA
    );
}

#[test_log::test(tokio::test)]
async fn a_capture_with_blank_criteria_is_refused_under_them_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit(&pages, "gadgets", " \n\t\n").await;

    assert_eq!(status, 422);
    assert_eq!(placed(&answer), under("criteria", "Required."));
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_taken_name_is_refused_under_the_name() {
    let pages = implemented(Recorded::Whole).await;
    submit(&pages, "gadgets", CRITERIA).await;

    let (status, answer) = submit(&pages, "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        placed(&answer),
        under("name", "A task named gadgets already exists.")
    );
}

#[test_log::test(tokio::test)]
async fn a_name_that_is_not_a_tasks_is_refused_under_the_name() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit(&pages, "../Gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(placed(&answer), under("name", NOT_A_NAME));
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

/// `docs/specs/design_system.md`, Making, Required: every empty required
/// field is refused at once, each under it.
#[test_log::test(tokio::test)]
async fn empty_fields_are_each_refused_as_required_at_once() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit_into(&pages, Chosen::New(" "), "\t", "").await;

    assert_eq!(status, 422);
    assert_eq!(
        placed(&answer),
        (
            vec![
                ("new-benchmark", "Required.".to_string()),
                ("name", "Required.".to_string()),
                ("criteria", "Required.".to_string()),
            ],
            None
        )
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
    let (refused, whole) = placed(&answer);
    assert_eq!(refused, []);
    let whole = whole.unwrap();
    assert!(whole.contains(missing), "{whole}");
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_cross_site_capture_is_refused_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, form) = post_capture(
        &pages,
        &[CROSS_SITE],
        Chosen::Benchmark(IMPLEMENTS),
        "gadgets",
        CRITERIA,
    )
    .await;

    assert_eq!(status, 403);
    assert_eq!(
        form_shown(&form),
        shown(
            IMPLEMENTS,
            "",
            "gadgets",
            CRITERIA,
            Some("a capture is taken only from the web UI's own pages")
        )
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

/// `docs/specs/benchmarks.md`, Capturing, Before the first poll.
#[test_log::test(tokio::test)]
async fn before_the_first_poll_add_task_is_offered_and_the_capture_refused() {
    let pages = implemented(Recorded::Whole).await;
    pages.served.send_replace(None);

    let html = pages.page(RUN_PAGE).await;
    let (status, answer) = submit(&pages, "gadgets", CRITERIA).await;

    let form = capture_form(&html).unwrap();
    assert!(
        select_in(form, "button[type=submit]")[0]
            .attr("disabled")
            .is_none()
    );
    assert_eq!(status, 503);
    assert_eq!(
        placed(&answer),
        above_buttons("Linewright can't capture a run before its first poll.")
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn the_form_offers_the_benchmarks_of_the_runs_stage_or_a_new_one() {
    let pages = implemented(Recorded::Whole).await;
    keep_benchmark(&pages.path(""), "board", "implement").await;

    let html = pages.page(RUN_PAGE).await;

    let form = capture_form(&html).expect("a capture form");
    let options: Vec<(Option<&str>, String)> = select_in(form, "select[name=benchmark] option")
        .into_iter()
        .map(|option| (option.attr("value"), text(option)))
        .collect();
    assert_eq!(
        options,
        [
            (Some("board"), "board".to_string()),
            (Some(IMPLEMENTS), IMPLEMENTS.to_string()),
            (Some(""), "New benchmark".to_string()),
        ]
    );
}

/// `docs/specs/runs.md`, Run, Capturing, What was made: the dialog opens
/// again offering the benchmark the capture made.
#[test_log::test(tokio::test)]
async fn a_capture_into_a_new_benchmark_makes_it_says_so_and_offers_it_next_time() {
    let pages = implemented(Recorded::Whole).await;

    let (status, closes, form, messages) = made(&pages, Chosen::New("fresh")).await;

    assert_eq!(status, 200, "{}", form.html());
    assert!(closes);
    assert_eq!(
        messages,
        [(
            "Captured run 1/implement-1 as the task gadgets in the new benchmark fresh.".into(),
            Some("/benchmarks/fresh/tasks".into())
        )]
    );
    assert_eq!(
        form_shown(&form),
        shown("fresh", "", "1-implement-1", "", None)
    );
    let offered = |answered: &Html| -> Vec<String> {
        select(answered, "select[name=benchmark] option")
            .into_iter()
            .map(text)
            .collect()
    };
    assert_eq!(offered(&form), ["fresh", IMPLEMENTS, "New benchmark"]);
    let html = pages.page(RUN_PAGE).await;
    let page_form = Html::parse_fragment(&capture_form(&html).unwrap().inner_html());
    assert_eq!(offered(&page_form), ["fresh", IMPLEMENTS, "New benchmark"]);
}

/// `docs/specs/design_system.md`, Making, A refusal: the dialog stays open,
/// the reason of a refusal of no field above its buttons, and the fields
/// keep what was typed and chosen.
#[test_log::test(tokio::test)]
async fn a_refused_capture_answers_the_form_as_sent_with_the_reason_and_leaves_the_dialog_open() {
    let pages = implemented(Recorded::Nothing).await;

    let (status, headers, form) = post_answer(
        &pages,
        RUN_PAGE,
        &[SAME_ORIGIN],
        &fields(Chosen::Benchmark(IMPLEMENTS), "gadgets", "Ships.\n"),
    )
    .await;

    assert_eq!(status, 422);
    assert!(headers.get("hx-trigger").is_none());
    assert_eq!(select(&form, "#made").len(), 0, "{}", form.html());
    let above_buttons = select(&form, ".modal-action")[0]
        .prev_siblings()
        .find_map(ElementRef::wrap)
        .unwrap();
    assert!(
        has_classes(above_buttons, &["alert-error"]),
        "{}",
        form.html()
    );
    assert_eq!(
        form_shown(&form),
        shown(
            IMPLEMENTS,
            "",
            "gadgets",
            "Ships.\n",
            Some("Run 1/implement-1 recorded no input.")
        )
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_capture_into_a_benchmark_of_another_stage_is_refused_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) =
        submit_into(&pages, Chosen::Benchmark(DESIGNS), "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        placed(&answer),
        above_buttons("Run 1/implement-1 is of implement, and designs holds tasks of design.")
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_new_benchmark_without_a_name_is_refused_under_it_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit_into(&pages, Chosen::New(""), "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(placed(&answer), under("new-benchmark", "Required."));
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_new_benchmark_whose_name_is_not_a_name_is_refused_under_it_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit_into(&pages, Chosen::New("Fresh"), "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(placed(&answer), under("new-benchmark", NOT_A_NAME));
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_new_benchmark_named_as_one_that_exists_is_refused_under_its_name_and_writes_nothing() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = submit_into(&pages, Chosen::New(DESIGNS), "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        placed(&answer),
        under("new-benchmark", "A benchmark named designs already exists.")
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_run_that_recorded_no_input_is_refused_and_says_so() {
    let pages = implemented(Recorded::Nothing).await;

    let (status, answer) = submit(&pages, "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        placed(&answer),
        above_buttons("Run 1/implement-1 recorded no input.")
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn a_run_that_does_not_exist_is_refused_and_says_so() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) = post(
        &pages,
        "/runs/1/implement-9",
        &[SAME_ORIGIN],
        &[
            ("benchmark", IMPLEMENTS),
            ("name", "gadgets"),
            ("criteria", CRITERIA),
        ],
    )
    .await;

    assert_eq!(status, 404);
    assert_eq!(placed(&answer), above_buttons("No such run 1/implement-9."));
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

/// `docs/specs/runs.md`, Run, Capturing: with no benchmark offered, "New
/// benchmark" is chosen and its name enabled; and Benchmark has no
/// `required`, which the browser would take its empty choice to fail.
#[test_log::test(tokio::test)]
async fn with_no_benchmark_offered_new_benchmark_is_chosen_and_its_name_enabled() {
    let pages = implemented(Recorded::Whole).await;

    let (_, answer) = post(
        &pages,
        "/runs/1/rework-1",
        &[SAME_ORIGIN],
        &fields(Chosen::Benchmark(IMPLEMENTS), "gadgets", CRITERIA),
    )
    .await;

    let benchmark = select(&answer, "select[name=benchmark]")[0];
    assert_eq!(benchmark.attr("required"), None);
    let chosen: Vec<String> = select_in(benchmark, "option[selected]")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(chosen, ["New benchmark"]);
    let name = select(&answer, "input[name=new-benchmark]")[0];
    assert_eq!(name.attr("disabled"), None, "{}", answer.html());
}

/// `docs/specs/benchmarks.md`, Capturing: a run that is not a triage, design
/// or implement run is refused above the buttons.
#[test_log::test(tokio::test)]
async fn a_run_that_cannot_be_captured_is_refused_above_the_buttons() {
    let pages = implemented(Recorded::Whole).await;
    pages
        .record(&json!({
            "item": 1,
            "stage": "rework",
            "n": 1,
            "started_at": "2026-10-13T10:20:00Z",
            "end": {"at": "2026-10-13T10:23:00Z", "ending": "done"},
            "sessions": [],
        }))
        .await;

    let (status, answer) = post(
        &pages,
        "/runs/1/rework-1",
        &[SAME_ORIGIN],
        &fields(Chosen::Benchmark(IMPLEMENTS), "gadgets", CRITERIA),
    )
    .await;

    assert_eq!(status, 422);
    let (refused, whole) = placed(&answer);
    assert_eq!(refused, []);
    assert!(whole.is_some_and(|whole| whole.contains("1/rework-1")));
    assert_eq!(tasks(&pages), Vec::<String>::new());
}

/// `docs/specs/runs.md`, Run, Capturing: a benchmark chosen that no longer
/// exists is refused under "Benchmark".
#[test_log::test(tokio::test)]
async fn a_benchmark_that_no_longer_exists_is_refused_under_benchmark() {
    let pages = implemented(Recorded::Whole).await;

    let (status, answer) =
        submit_into(&pages, Chosen::Benchmark("gone"), "gadgets", CRITERIA).await;

    assert_eq!(status, 422);
    assert_eq!(
        placed(&answer),
        under("benchmark", "there is no benchmark gone")
    );
    assert_eq!(tasks(&pages), Vec::<String>::new());
}
