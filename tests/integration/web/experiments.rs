use super::suites::with_suites;
use super::*;

use std::panic::Location;

use serde_json::json;

/// Launches an experiment on the suite `suite` of `pages` under `as-is` and
/// a configuration setting the model for each of `configurations`, each
/// replayed `repetitions` times, as `lw experiment launch` does, and sets
/// its launch to `at`: its ID.
#[track_caller]
pub(super) fn launched<'a>(
    pages: &'a Pages,
    suite: &str,
    configurations: &[&str],
    repetitions: u32,
    at: &str,
) -> impl Future<Output = i64> + 'a {
    let caller = Location::caller();
    let suite = suite.to_string();
    let configurations: Vec<_> = configurations
        .iter()
        .map(|name| {
            let files = std::collections::BTreeMap::from([(
                "config.toml".to_string(),
                b"model = \"haiku\"\n".to_vec(),
            )]);
            (
                name.parse().unwrap(),
                linewright::replay::Configuration::parse(files).unwrap(),
            )
        })
        .collect();
    let at = at.parse().unwrap();
    async move {
        let experiments =
            linewright::benchmarks::Experiments::new(pages.sources.runs.database().clone());
        let launched = experiments
            .launch(
                &suite.parse().unwrap(),
                configurations,
                std::num::NonZeroU32::new(repetitions).unwrap(),
                at,
            )
            .await
            .unwrap_or_else(|e| panic!("launching at {caller}: {e:#}"));
        launched.id.to_string().parse().unwrap()
    }
}

/// [`with_suites`]'s pages, with experiment 1 on the suite `board`, its
/// cases `columns` and `lanes` replayed twice under `as-is` and `short`,
/// launched at noon: of its eight replays, six have ended, with passes,
/// failures, a leak and two without a judgement, one replays and one is
/// pending. The first replay's output holds its run and its judgement.
#[track_caller]
pub(super) fn with_experiment() -> impl Future<Output = Pages> {
    let caller = Location::caller();
    async move {
        let pages = with_suites().await;
        launched(&pages, "board", &["short"], 2, "2026-11-17T12:00:00Z").await;
        let work_dir = pages.path("");
        let trials: [((i64, i64, i64), &'static str); 7] = [
            (
                (1, 0, 0),
                "state = 'judged', outcome = 'done', money = 0.3, seconds = 90, \
             correctness = 'pass', leak = 0, judge_money = 0.05",
            ),
            (
                (1, 0, 1),
                "state = 'judged', outcome = 'handed-back', money = 0.1, seconds = 10, \
             correctness = 'fail', leak = 1, judge_money = 0.02",
            ),
            (
                (1, 1, 0),
                "state = 'unjudged', outcome = 'failed', seconds = 30, unjudged = 'outcome'",
            ),
            (
                (1, 1, 1),
                "state = 'judged', outcome = 'done', money = 0.2, seconds = 20, \
             correctness = 'fail', leak = 0, judge_money = 0.03",
            ),
            (
                (2, 0, 0),
                "state = 'judged', outcome = 'done', money = 0.5, seconds = 30, \
             correctness = 'pass', leak = 0",
            ),
            (
                (2, 0, 1),
                "state = 'unjudged', outcome = 'done', money = 0.4, seconds = 40, \
             unjudged = 'judge', reason = 'it gave up', judge_money = 0.01",
            ),
            ((2, 1, 0), "state = 'replaying'"),
        ];
        for (key, set) in trials {
            trial(&work_dir, 1, key, set).await;
        }
        let ran = json!({
            "model": null,
            "effort": null,
            "layers": {"settings": {"model": "harness"}, "prompts": {}},
        });
        let mut record = json!({
            "item": 1,
            "stage": "implement",
            "n": 1,
            "started_at": "2026-11-17T12:00:05Z",
            "end": {"at": "2026-11-17T12:01:35Z", "ending": "done"},
            "sessions": [{"dir": "sessions/issue-1/20261117T120005Z-implement"}],
        });
        record["sessions"][0]
            .as_object_mut()
            .unwrap_or_else(|| panic!("a JSON object at {caller}"))
            .extend(ran.as_object().unwrap().clone());
        let mut session = json!({"record": "judge/20261117T120200Z-judge", "cost": {}});
        session
            .as_object_mut()
            .unwrap_or_else(|| panic!("a JSON object at {caller}"))
            .extend(ran.as_object().unwrap().clone());
        let judgement = json!({
            "correctness": {"reasoning": "It names the columns.", "verdict": "pass"},
            "leak": {"reasoning": "I read every transcript.", "found": false, "calls": []},
            "session": session,
            "linewright": "0.1.0",
        });
        let output = "experiments/1/as-is/columns/1";
        let turn = include_str!("../fixtures/transcript/turn-1.jsonl");
        pages.write(&[
            (&format!("{output}/case"), "1/implement-1\n"),
            (&format!("{output}/outcome"), "done\n"),
            (
                &format!("{output}/runs/items/1/implement-1.json"),
                &record.to_string(),
            ),
            (
                &format!("{output}/sessions/issue-1/20261117T120005Z-implement/output.jsonl"),
                turn,
            ),
            (
                &format!("{output}/sessions/issue-1/20261117T120005Z-implement/brief.md"),
                "# Your task\n\nImplement the ticket below.\n",
            ),
            (&format!("{output}/judgement.json"), &judgement.to_string()),
            (
                &format!("{output}/judge/20261117T120200Z-judge/output.jsonl"),
                turn,
            ),
            (
                &format!("{output}/judge/20261117T120200Z-judge/brief.md"),
                "# Your task\n\nJudge the replay.\n",
            ),
        ]);
        pages
    }
}

/// `docs/specs/benchmarks.md`, Suite, Experiments; `docs/specs/design_system.md`,
/// Tables and Status.
#[test_log::test(tokio::test)]
async fn the_suite_page_lists_its_experiments_latest_first_linking_to_each() {
    let pages = with_experiment().await;
    launched(&pages, "board", &[], 1, "2026-11-17T13:00:00Z").await;
    linewright::benchmarks::Experiments::new(pages.sources.runs.database().clone())
        .cancel(
            "2".parse().unwrap(),
            "2026-11-17T14:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    let html = pages.page("/suites/board").await;

    let heads: Vec<String> = select(&html, "#experiments thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(
        heads,
        ["", "State", "Cases", "Configurations", "Duration", "Cost"]
    );
    assert_eq!(
        rows(&html, "#experiments"),
        [
            [
                "2026-11-17 13:00:00 UTC",
                "cancelled",
                "2",
                "1",
                "1h",
                "$0.00"
            ],
            [
                "2026-11-17 12:00:00 UTC",
                "waiting",
                "2",
                "2",
                "0s",
                "$1.61, 1 without a cost",
            ],
        ]
    );
    let launches: Vec<&str> = select(&html, "#experiments tbody tr > th:first-child a")
        .into_iter()
        .filter_map(|link| link.attr("href"))
        .collect();
    assert_eq!(launches, ["/experiments/2", "/experiments/1"]);
    assert_eq!(
        select(&html, "#experiments tbody tr:nth-child(2) time[data-since]")[0].attr("data-since"),
        Some("2026-11-17T12:00:00Z")
    );
    for state in select(&html, "#experiments tbody td:nth-child(2)") {
        assert_eq!(select_in(state, ".badge").len(), 1, "{}", state.html());
    }
    assert_no_status_by_colour_alone(&html);
    let alignments = alignments(&html, "#experiments");
    assert!(
        alignments
            .iter()
            .all(|row| row == &["start", "start", "start", "start", "end", "end"]),
        "{alignments:?}"
    );
}

/// `docs/specs/benchmarks.md`, Suite, Experiments, and New experiment.
#[test_log::test(tokio::test)]
async fn new_experiment_shows_in_the_experiments_bar_only_on_a_suite_with_cases() {
    let pages = with_suites().await;

    let board = pages.page("/suites/board").await;
    let empty = pages.page("/suites/triage-basics").await;

    let new = select(&board, "#new-experiment-offer button[command=show-modal]");
    assert_eq!(new.len(), 1, "{}", board.html());
    assert_eq!(text(new[0]), "New");
    let dialog = new[0].attr("commandfor").unwrap();
    assert_eq!(
        select(&board, &format!("dialog[id='{dialog}'] form.launch")).len(),
        1
    );
    assert_eq!(
        select(&empty, "#new-experiment-offer button").len(),
        0,
        "{}",
        empty.html()
    );
}

#[test_log::test(tokio::test)]
async fn the_experiment_pages_heading_shows_its_suite_its_launch_its_state_and_its_progress() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    assert_eq!(text(select(&html, "#experiment h1")[0]), "Experiment 1");
    assert_eq!(
        text(select(&html, "#experiment .facts")[0]),
        "of the suite board, launched 2026-11-17 12:00:00 UTC waiting waiting for serve \
         6 of 8 replays ended"
    );
    assert_eq!(
        hrefs(select(&html, "#experiment .facts a")),
        ["/suites/board"]
    );
    let state = select(&html, "#experiment .facts .badge");
    assert!(
        has_classes(state[0], &["badge-outline"]),
        "{}",
        state[0].html()
    );
    assert_eq!(events_path(&html), Some("/events/experiments/1"));
    assert_no_status_by_colour_alone(&html);
}

#[test_log::test(tokio::test)]
async fn the_experiment_page_offers_to_cancel_an_open_experiment_and_cancelling_ends_it() {
    let pages = with_experiment().await;
    let html = pages.page("/experiments/1").await;
    assert_eq!(
        confirmations(&html),
        [Confirmation {
            asking: "Cancel the experiment".into(),
            question: "Cancel experiment 1? This stops its running replays and judges.".into(),
            turn_down: "Keep running".into(),
            action: "/experiments/1/cancel".into(),
            fields: vec![],
        }]
    );
    assert!(select(&html, "[hx-confirm]").is_empty(), "{}", html.html());

    let (status, answer) = post(&pages, "/experiments/1/cancel", &[SAME_ORIGIN], &[]).await;
    let (again, refused) = post(&pages, "/experiments/1/cancel", &[SAME_ORIGIN], &[]).await;
    let (other_site, _) = post(&pages, "/experiments/1/cancel", &[CROSS_SITE], &[]).await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(said(&answer), "Cancelled experiment 1");
    assert_eq!(again, 422);
    assert_eq!(said(&refused), "Not cancelled: experiment 1 is cancelled");
    assert_eq!(other_site, 403);
    let cancelled = pages.page("/experiments/1").await;
    assert!(
        text(select(&cancelled, "#experiment .facts")[0]).contains(" cancelled "),
        "{}",
        cancelled.html()
    );
    assert_eq!(confirmations(&cancelled), []);
}

#[test_log::test(tokio::test)]
async fn the_experiment_pages_chart_puts_each_configuration_at_its_mean_cost_and_pass_rate() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    let points = select(&html, "#chart svg .point");
    let shown: Vec<String> = points
        .iter()
        .map(|point| text(select_in(*point, "title")[0]))
        .collect();
    assert_eq!(
        shown,
        [
            "as-is: pass rate 100%, mean cost $0.40",
            "short: pass rate 0%, mean cost $0.23"
        ]
    );
    let at = |point: ElementRef| -> (f64, f64) {
        let circle = select_in(point, "circle")[0];
        (
            circle.attr("cx").unwrap().parse().unwrap(),
            circle.attr("cy").unwrap().parse().unwrap(),
        )
    };
    let (as_is, short) = (at(points[0]), at(points[1]));
    assert!(as_is.0 > short.0, "the dearer is to the right");
    assert!(as_is.1 < short.1, "the higher pass rate is higher up");
    assert_eq!(
        text(select(&html, "#chart figcaption")[0]),
        "Mean cost of a replay against pass rate, a point for each configuration"
    );
}

#[test_log::test(tokio::test)]
async fn the_experiment_pages_grid_shows_the_passes_leaks_and_unjudged_of_each_case_and_configuration()
 {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    let heads: Vec<String> = select(&html, "#grid thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["Case", "as-is", "short"]);
    assert_eq!(
        rows(&html, "#grid"),
        [
            ["columns", "2 of 2", "–, 1 leak, 1 unjudged"],
            ["lanes", "–, 1 unjudged", "0 of 1"],
        ]
    );
}

#[test_log::test(tokio::test)]
async fn the_experiment_pages_table_shows_each_configurations_figures_and_the_judges_cost() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    let figures: Vec<Vec<String>> = select(&html, "#configurations details.configuration")
        .into_iter()
        .map(|configuration| {
            select_in(configuration, ":scope > summary > span")
                .into_iter()
                .map(text)
                .collect()
        })
        .collect();
    assert_eq!(
        figures,
        [
            [
                "as-is",
                "100%",
                "mean $0.40, total $0.80, 1 without a cost",
                "mean 50s, total 2m 30s",
                "0",
                "1"
            ],
            [
                "short",
                "0%",
                "mean $0.23, total $0.70",
                "mean 23s, total 1m 10s",
                "1",
                "1"
            ],
        ]
    );
    assert_eq!(
        text(select(&html, "#configurations .judges")[0]),
        "The judges cost $0.11, 1 without a cost."
    );
}

#[test_log::test(tokio::test)]
async fn a_configuration_opens_to_its_cases_and_a_case_to_its_replays_with_their_judgements() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    let configurations = select(&html, "#configurations details.configuration");
    let cases: Vec<String> = select_in(configurations[1], "details.case > summary")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(cases, ["columns: –", "lanes: 0 of 1"]);
    let replays = |configuration: ElementRef, case: usize| -> Vec<String> {
        let case = select_in(configuration, "details.case")[case];
        select_in(case, ".replay .replay-line")
            .into_iter()
            .map(text)
            .collect()
    };
    assert_eq!(
        replays(configurations[0], 0),
        [
            "repetition 1 judged pass, cost $0.30, took 1m 30s run's transcript judge's transcript",
            "repetition 2 judged pass, cost $0.50, took 30s",
        ]
    );
    assert_eq!(
        replays(configurations[1], 0),
        [
            "repetition 1 judged fail, with a leak, cost $0.10, took 10s",
            "repetition 2 unjudged no judgement: the judge made none: it gave up, cost $0.40, \
             took 40s",
        ]
    );
    assert_eq!(
        replays(configurations[0], 1),
        [
            "repetition 1 unjudged no judgement: its run is not done or handed back, \
             no cost, took 30s",
            "repetition 2 replaying",
        ]
    );
    assert_eq!(replays(configurations[1], 1)[1], "repetition 2 waiting");
    let waiting = select_in(
        select_in(configurations[1], "details.case")[1],
        ".replay .badge",
    )[1];
    assert!(
        has_classes(waiting, &["badge-outline"]),
        "{}",
        waiting.html()
    );
    let first = select_in(select_in(configurations[0], "details.case")[0], ".replay")[0];
    let reasoning: Vec<String> = select_in(first, ".reasoning p")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(
        reasoning,
        [
            "Correctness: It names the columns.",
            "Leak: I read every transcript."
        ]
    );
    assert_eq!(
        hrefs(select_in(first, "a")),
        [
            "/experiments/1/as-is/columns/1/run",
            "/experiments/1/as-is/columns/1/judge"
        ]
    );
}

#[test_log::test(tokio::test)]
async fn a_replay_opens_to_the_transcripts_of_its_run_and_of_its_judge() {
    let pages = with_experiment().await;

    let run = pages.page("/experiments/1/as-is/columns/1/run").await;
    let judge = pages.page("/experiments/1/as-is/columns/1/judge").await;

    assert_eq!(text(select(&run, "#transcript h1")[0]), "1/implement-1");
    assert_eq!(
        text(select(&run, "#replay")[0]),
        "Replay as-is/columns/1 of experiment 1: its run's transcript, or its judge's"
    );
    assert_eq!(
        hrefs(select(&run, "#replay a")),
        ["/experiments/1", "/experiments/1/as-is/columns/1/judge"]
    );
    assert!(!select(&run, "#s1 .entry").is_empty(), "{}", run.html());
    assert_eq!(text(select(&judge, "#transcript h1")[0]), "judge");
    assert_eq!(
        text(select(&judge, "#transcript hgroup p")[0]),
        "the judge of replay as-is/columns/1"
    );
    assert!(!select(&judge, "#s1 .entry").is_empty(), "{}", judge.html());
    assert_eq!(
        events_path(&judge),
        Some("/events/experiments/1/as-is/columns/1/judge")
    );
}

#[test_log::test(tokio::test)]
async fn a_replay_with_no_output_or_judge_answers_404() {
    let pages = with_experiment().await;

    let (pending, _) = pages.get("/experiments/1/short/lanes/2/run").await;
    let (no_judge, _) = pages.get("/experiments/1/as-is/columns/2/judge").await;
    let (no_case, _) = pages.get("/experiments/1/as-is/rows/1/run").await;
    let (no_experiment, _) = pages.get("/experiments/9").await;

    assert_eq!([pending, no_judge, no_case, no_experiment], [404; 4]);
}

/// `docs/specs/design_system.md`, Screen states, Nothing there.
#[test_log::test(tokio::test)]
async fn the_page_of_no_experiment_says_there_is_none() {
    let pages = with_experiment().await;

    let (no_experiment, no_experiment_body) = pages.get("/experiments/9").await;
    let (no_id, no_id_body) = pages.get("/experiments/first").await;
    let (replay, replay_body) = pages.get("/experiments/9/as-is/columns/1/run").await;

    for (status, body, region, shown) in [
        (
            no_experiment,
            no_experiment_body,
            "experiment",
            "No such experiment 9",
        ),
        (no_id, no_id_body, "experiment", "No such experiment first"),
        (replay, replay_body, "transcript", "No such experiment 9"),
    ] {
        assert_eq!(status, 404);
        let html = Html::parse_document(&body);
        assert_eq!(text(select(&html, &format!("main #{region}"))[0]), shown);
    }
}

#[test_log::test(tokio::test)]
async fn the_experiment_pages_stream_sends_the_experiment_as_it_changes() {
    let pages = with_experiment().await;
    let mut events = stream(&pages, "/events/experiments/1").await;
    events.recv().await.unwrap();

    trial(
        &pages.path(""),
        1,
        (2, 1, 0),
        "state = 'unjudged', outcome = 'failed', seconds = 5, unjudged = 'outcome'",
    )
    .await;

    loop {
        let event = events.recv().await.unwrap();
        let ended = regions(&event)
            .into_iter()
            .filter(|(id, _)| id == "experiment")
            .any(|(_, experiment)| {
                text(select(&experiment, ".facts")[0]).ends_with("7 of 8 replays ended")
            });
        if ended {
            break;
        }
    }
}

/// Linewright's own text of the brief's file `prompts/design.md`, which a
/// session gets as it is written.
const DESIGN: &str = include_str!("../../../briefs/prompts/design.md");

/// Linewright's own text of the brief's file `prompts/implement.md`.
const IMPLEMENT: &str = include_str!("../../../briefs/prompts/implement.md");

/// Linewright's own text of the brief's file `prompts/fix-ci.md`.
const FIX_CI: &str = include_str!("../../../briefs/prompts/fix-ci.md");

/// A configuration of the New experiment dialog but `as-is`, as a reader
/// meets it: its name, model, effort and session-timeout, and each file of
/// the brief it gives, with its text and the text it started with.
#[derive(Debug, PartialEq)]
struct ConfigurationShown {
    name: String,
    model: String,
    effort: String,
    session_timeout: String,
    files: Vec<(String, String, String)>,
}

/// The New experiment dialog's form as a reader meets it: its
/// configurations but `as-is`, its repetitions, how many replays they make,
/// and the refusal above its buttons, if any.
#[derive(Debug, PartialEq)]
struct LaunchShown {
    configurations: Vec<ConfigurationShown>,
    repetitions: String,
    count: String,
    refusal: Option<String>,
}

/// What the form `form` of the New experiment dialog shows.
#[track_caller]
fn launch_form(form: &Html) -> LaunchShown {
    let refusals = select(form, ".alert");
    assert!(refusals.len() <= 1, "{}", form.html());
    let configurations = select(form, "fieldset.configuration:not(.as-is)")
        .into_iter()
        .map(|fieldset| {
            let values = field_values(select_in(fieldset, "input[name], select[name]"));
            let value = |key: &str| {
                let found: Vec<&String> = values
                    .iter()
                    .filter(|(name, _)| name.split_once('.').map(|(_, k)| k) == Some(key))
                    .map(|(_, value)| value)
                    .collect();
                assert_eq!(found.len(), 1, "{key} in {}", fieldset.html());
                found[0].clone()
            };
            let files = select_in(fieldset, "textarea[name]")
                .into_iter()
                .map(|area| {
                    let name = area.attr("name").unwrap();
                    let (number, path) = name.split_once(".briefs/").unwrap();
                    let started_field = format!("{number}.started/{path}");
                    let started = values
                        .iter()
                        .find(|(field, _)| *field == started_field)
                        .map(|(_, text)| text.clone())
                        .unwrap_or_default();
                    (path.to_string(), area.text().collect(), started)
                })
                .collect();
            ConfigurationShown {
                name: value("name"),
                model: value("model"),
                effort: value("effort"),
                session_timeout: value("session-timeout"),
                files,
            }
        })
        .collect();
    LaunchShown {
        configurations,
        repetitions: field_values(select(form, "input[name=repetitions]"))[0]
            .1
            .clone(),
        count: text(select(form, ".replay-count")[0]),
        refusal: refusals.first().map(|refusal| text(*refusal)),
    }
}

/// What the reader reads in the New experiment dialog's form, in order,
/// leaving out what each configuration but `as-is` holds: its title, each
/// label, `as-is`, each button and how many replays it makes.
fn read_in_order(form: ElementRef) -> Vec<String> {
    select_in(
        form,
        "legend, label, .label, button, .replay-count, fieldset.configuration:not(.as-is)",
    )
    .into_iter()
    .filter(|element| {
        !element.ancestors().filter_map(ElementRef::wrap).any(|up| {
            up.value().name() == "fieldset"
                && up.value().classes().any(|c| c == "configuration")
                && !up.value().classes().any(|c| c == "as-is")
        })
    })
    .map(|element| {
        if element.value().name() == "fieldset" {
            "a configuration".to_string()
        } else {
            text(element)
        }
    })
    .collect()
}

/// `docs/specs/benchmarks.md`, New experiment; `docs/specs/design_system.md`,
/// Dialogs and Making.
#[test_log::test(tokio::test)]
async fn new_opens_the_new_experiment_dialog_with_as_is_first_the_repetitions_and_the_replays_they_make()
 {
    let pages = with_suites().await;

    let html = pages.page("/suites/board").await;

    let dialog = select(&html, "dialog#new-experiment");
    assert_eq!(dialog.len(), 1, "{}", html.html());
    let dialog = dialog[0];
    assert!(has_classes(dialog, &["modal"]));
    assert_eq!(select_in(dialog, ".modal-backdrop").len(), 0);
    assert_eq!(select(&html, "#experiments dialog").len(), 0);
    let legend = select_in(dialog, "form > fieldset > legend.fieldset-legend");
    assert_eq!(text(legend[0]), "New experiment on board");
    assert_eq!(dialog.attr("aria-labelledby"), legend[0].attr("id"));
    let form = select_in(dialog, "form[hx-post]")[0];
    assert_eq!(form.attr("hx-post"), Some("/suites/board/launch"));
    assert_eq!(form.attr("hx-target"), Some("this"));
    assert!(form.attr("data-new").is_some());
    assert_eq!(
        read_in_order(form),
        [
            "New experiment on board",
            "Configurations",
            "as-is",
            "Add a configuration",
            "Repetitions",
            "1 configuration × 2 cases × 1 repetition: 2 replays",
            "Cancel",
            "Run experiment",
        ]
    );
    assert!(has_classes(
        select_in(form, "fieldset.configuration")[0],
        &["as-is"]
    ));
    assert_eq!(
        select_in(form, "fieldset.as-is :is(input, select, textarea)").len(),
        0
    );
    assert_eq!(
        launch_form(&Html::parse_fragment(&form.inner_html())),
        LaunchShown {
            configurations: vec![],
            repetitions: "1".into(),
            count: "1 configuration × 2 cases × 1 repetition: 2 replays".into(),
            refusal: None,
        }
    );
    let buttons = select_in(form, ".modal-action button");
    assert_eq!(buttons[0].attr("command"), Some("close"));
    assert_eq!(buttons[0].attr("commandfor"), Some("new-experiment"));
    assert!(has_classes(buttons[1], &["btn-primary"]));
    assert_eq!(buttons[1].attr("type"), Some("submit"));

    let template = select(&html, "template#configuration-template");
    assert_eq!(template.len(), 1);
    let added = Html::parse_fragment(&template[0].inner_html());
    let labels: Vec<String> = select(&added, "label").into_iter().map(text).collect();
    assert_eq!(
        labels,
        [
            "Name",
            "model",
            "effort",
            "session-timeout",
            "Files of the brief"
        ]
    );
    for label in select(&added, "label") {
        let field = label.attr("for").unwrap();
        assert_eq!(
            select(&added, &format!("[id='{field}']")).len(),
            1,
            "{field}"
        );
    }
    let fields: Vec<&str> = select(&added, "[name]")
        .into_iter()
        .filter_map(|field| field.attr("name"))
        .collect();
    assert_eq!(
        fields,
        [
            "c__N__.name",
            "c__N__.model",
            "c__N__.effort",
            "c__N__.session-timeout"
        ]
    );
    let efforts: Vec<String> = select(&added, "select[name='c__N__.effort'] option")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(
        efforts,
        ["as it is", "low", "medium", "high", "xhigh", "max"]
    );
    let files: Vec<&str> = select(&added, "select.brief-file option")
        .into_iter()
        .filter_map(|option| option.attr("value"))
        .collect();
    assert!(files.contains(&"prompts/implement.md"), "{files:?}");
    assert_eq!(files[0], "");
    let removes: Vec<String> = select(&added, "button").into_iter().map(text).collect();
    assert_eq!(removes, ["Remove"]);
    let texts: std::collections::BTreeMap<String, String> =
        serde_json::from_str(select(&html, "#brief-texts")[0].attr("data-texts").unwrap()).unwrap();
    assert_eq!(texts["prompts/implement.md"], IMPLEMENT);
    assert_eq!(
        texts["plugin/skills/tdd/SKILL.md"],
        include_str!("../../../briefs/plugin/skills/tdd/SKILL.md")
    );
}

/// `docs/specs/benchmarks.md`, New experiment, Files of the brief: the
/// page's stream sends the texts a run of the stage uses, from which the
/// dialog starts a file.
#[test_log::test(tokio::test)]
async fn the_suite_pages_stream_sends_the_texts_of_the_files_of_the_brief() {
    let pages = with_suites().await;
    let mut events = stream(&pages, "/events/suites/board").await;

    let first = events.recv().await.unwrap();

    let texts = regions(&first)
        .into_iter()
        .find(|(id, _)| id == "brief-texts")
        .map(|(_, region)| region)
        .expect("the texts of the files of the brief");
    let texts: std::collections::BTreeMap<String, String> = serde_json::from_str(
        select(&texts, "#brief-texts")[0]
            .attr("data-texts")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(texts["prompts/design.md"], DESIGN);
}

#[test_log::test(tokio::test)]
async fn a_launch_on_a_suite_with_no_cases_is_refused() {
    let pages = with_suites().await;

    let (status, answer) = post(
        &pages,
        "/suites/triage-basics/launch",
        &[SAME_ORIGIN],
        &[("repetitions", "1")],
    )
    .await;

    assert_eq!(status, 422);
    assert_eq!(
        launch_form(&answer),
        LaunchShown {
            configurations: vec![],
            repetitions: "1".into(),
            count: "1 configuration × 0 cases × 1 repetition: 0 replays".into(),
            refusal: Some("The suite triage-basics has no cases.".into()),
        }
    );
    assert_eq!(
        text(select(&answer, "legend")[0]),
        "New experiment on triage-basics"
    );
    assert!(!pages.path("experiments/1").exists());
}

/// A configuration named `name` that sets nothing.
fn named(name: &str) -> ConfigurationShown {
    ConfigurationShown {
        name: name.into(),
        model: String::new(),
        effort: String::new(),
        session_timeout: String::new(),
        files: vec![],
    }
}

/// The configurations a launch sends, those its refused form shows, and the
/// start of its refusal.
type Refusal = (
    &'static [(&'static str, &'static str)],
    Vec<ConfigurationShown>,
    &'static str,
);

/// `docs/specs/benchmarks.md`, New experiment, Refusals;
/// `docs/specs/design_system.md`, Making, A refusal.
#[test_log::test(tokio::test)]
async fn a_refused_launch_answers_the_form_as_sent_naming_the_configuration_refused() {
    let pages = with_suites().await;
    let refusals: [Refusal; 6] = [
        (
            &[("c1.name", "short"), ("c1.session-timeout", "soon")],
            vec![ConfigurationShown {
                session_timeout: "soon".into(),
                ..named("short")
            }],
            "The configuration short cannot be used: config.toml: ",
        ),
        (
            &[
                ("c1.name", "short"),
                ("c1.briefs/prompts/implement.md", "Implement {{nothing}}.\n"),
            ],
            vec![ConfigurationShown {
                files: vec![(
                    "prompts/implement.md".into(),
                    "Implement {{nothing}}.\n".into(),
                    String::new(),
                )],
                ..named("short")
            }],
            "The configuration short cannot be used: briefs/prompts/implement.md: ",
        ),
        (
            &[("c1.name", "as-is")],
            vec![named("as-is")],
            "Two configurations are named as-is.",
        ),
        (
            &[("c1.name", "short"), ("c2.name", "short")],
            vec![named("short"), named("short")],
            "Two configurations are named short.",
        ),
        (
            &[("c1.name", "Short")],
            vec![named("Short")],
            "\"Short\" cannot name a configuration. A name holds only lowercase letters, digits \
             and hyphens, and starts with a letter or a digit.",
        ),
        (
            &[("c1.name", "short"), ("c2.name", " ")],
            vec![named("short"), named(" ")],
            "The 3rd configuration needs a name.",
        ),
    ];

    for (fields, configurations, refused) in refusals {
        let mut sent = vec![("repetitions", "1")];
        sent.extend_from_slice(fields);
        let (status, headers, answer) =
            post_answer(&pages, "/suites/board/launch", &[SAME_ORIGIN], &sent).await;
        assert_eq!(status, 422, "{fields:?}");
        assert!(headers.get("hx-trigger").is_none());
        let shown = launch_form(&answer);
        assert!(
            shown
                .refusal
                .as_deref()
                .unwrap_or_default()
                .starts_with(refused),
            "{shown:?}"
        );
        assert_eq!(shown.configurations, configurations, "{fields:?}");
    }
    let (no_repetitions, answer) = post(
        &pages,
        "/suites/board/launch",
        &[SAME_ORIGIN],
        &[("repetitions", "0"), ("c1.name", "short")],
    )
    .await;
    assert_eq!(no_repetitions, 422);
    assert_eq!(
        launch_form(&answer),
        LaunchShown {
            configurations: vec![named("short")],
            repetitions: "0".into(),
            count: "2 configurations × 2 cases × 1 repetition: 4 replays".into(),
            refusal: Some("The repetitions are a whole number, 1 or more.".into()),
        }
    );
    assert!(!pages.path("experiments/1").exists());
}

/// `docs/specs/design_system.md`, Making, A refusal: the fields keep every
/// configuration, its settings and its files' texts.
#[test_log::test(tokio::test)]
async fn a_refused_launch_keeps_every_configuration_with_its_settings_and_its_files() {
    let pages = with_suites().await;
    let design = format!("{DESIGN}\nCut the work finely.\n");

    let (status, answer) = post(
        &pages,
        "/suites/board/launch",
        &[SAME_ORIGIN],
        &[
            ("repetitions", "3"),
            ("c1.name", "short"),
            ("c1.model", "haiku"),
            ("c1.effort", "high"),
            ("c1.session-timeout", "30m"),
            ("c1.briefs/prompts/fix-ci.md", FIX_CI),
            ("c1.started/prompts/fix-ci.md", FIX_CI),
            ("c1.briefs/prompts/design.md", &design.replace('\n', "\r\n")),
            ("c1.started/prompts/design.md", DESIGN),
            ("c4.name", "Careful"),
            ("c4.effort", "max"),
        ],
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(
        launch_form(&answer),
        LaunchShown {
            configurations: vec![
                ConfigurationShown {
                    name: "short".into(),
                    model: "haiku".into(),
                    effort: "high".into(),
                    session_timeout: "30m".into(),
                    files: vec![
                        ("prompts/fix-ci.md".into(), FIX_CI.into(), FIX_CI.into()),
                        ("prompts/design.md".into(), design, DESIGN.into()),
                    ],
                },
                ConfigurationShown {
                    effort: "max".into(),
                    ..named("Careful")
                },
            ],
            repetitions: "3".into(),
            count: "3 configurations × 2 cases × 3 repetitions: 18 replays".into(),
            refusal: Some(
                "\"Careful\" cannot name a configuration. A name holds only lowercase letters, \
                 digits and hyphens, and starts with a letter or a digit."
                    .into()
            ),
        }
    );
    assert_eq!(
        select(&answer, "fieldset.configuration")[0]
            .select(&Selector::parse("legend").unwrap())
            .map(text)
            .collect::<Vec<_>>(),
        ["as-is"]
    );
    assert!(!pages.path("experiments/1").exists());
}

#[test_log::test(tokio::test)]
async fn a_launch_from_another_site_is_refused_keeping_the_form_as_sent() {
    let pages = with_suites().await;

    let (cross_site, cross_site_answer) = post(
        &pages,
        "/suites/board/launch",
        &[CROSS_SITE],
        &[
            ("repetitions", "2"),
            ("c1.name", "short"),
            ("c1.model", "haiku"),
        ],
    )
    .await;

    assert_eq!(cross_site, 403);
    assert_eq!(
        launch_form(&cross_site_answer),
        LaunchShown {
            configurations: vec![ConfigurationShown {
                model: "haiku".into(),
                ..named("short")
            }],
            repetitions: "2".into(),
            count: "2 configurations × 2 cases × 2 repetitions: 8 replays".into(),
            refusal: Some("a launch is taken only from the web UI's own pages".into()),
        }
    );
    assert!(!pages.path("experiments/1").exists());
}

/// `docs/specs/benchmarks.md`, New experiment; `docs/specs/design_system.md`,
/// Making.
#[test_log::test(tokio::test)]
async fn launching_answers_the_form_as_it_starts_closing_the_dialog_and_the_experiment_shows() {
    let pages = with_suites().await;
    let design = format!("{DESIGN}\nCut the work finely.\n");
    let implement = IMPLEMENT.replace('\n', "\r\n");
    let fix_ci = format!("{FIX_CI}\nAs the page started it.\n");
    let mut events = stream(&pages, "/events/suites/board").await;
    events.recv().await.unwrap();

    let (status, headers, answer) = post_answer(
        &pages,
        "/suites/board/launch",
        &[SAME_ORIGIN],
        &[
            ("repetitions", "2"),
            ("c3.name", "careful"),
            ("c3.effort", "high"),
            ("c1.name", "short"),
            ("c1.model", "haiku"),
            ("c1.effort", ""),
            ("c1.session-timeout", "30m"),
            ("c1.briefs/prompts/design.md", &design.replace('\n', "\r\n")),
            ("c1.briefs/prompts/implement.md", &implement),
            ("c1.briefs/prompts/fix-ci.md", &fix_ci),
            ("c1.started/prompts/fix-ci.md", &fix_ci),
        ],
    )
    .await;

    assert_eq!(status, 200, "{}", answer.html());
    assert_eq!(headers.get("hx-trigger").unwrap(), "made");
    assert_eq!(
        launch_form(&answer),
        LaunchShown {
            configurations: vec![],
            repetitions: "1".into(),
            count: "1 configuration × 2 cases × 1 repetition: 2 replays".into(),
            refusal: None,
        }
    );
    let experiments = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, region)) = regions(&event)
            .into_iter()
            .find(|(id, _)| id == "experiments")
        {
            break region;
        }
    };
    assert_eq!(
        hrefs(select(&experiments, "tbody tr > th a")),
        ["/experiments/1"]
    );
    let kept = pages.path("experiments/1/.kept/configurations");
    let files = |name: &str| -> Vec<String> {
        let dir = kept.join(name);
        let mut files: Vec<String> = files_under(&dir)
            .into_iter()
            .map(|path| path.strip_prefix(&dir).unwrap().display().to_string())
            .collect();
        files.sort();
        files
    };
    assert_eq!(files("as-is"), Vec::<String>::new());
    assert_eq!(files("short"), ["briefs/prompts/design.md", "config.toml"]);
    assert_eq!(
        std::fs::read_to_string(kept.join("short/config.toml")).unwrap(),
        "model = \"haiku\"\nsession-timeout = \"30m\"\n"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("short/briefs/prompts/design.md")).unwrap(),
        design
    );
    assert_eq!(files("careful"), ["config.toml"]);
    let html = pages.page("/experiments/1").await;
    let heads: Vec<String> = select(&html, "#grid thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["Case", "as-is", "short", "careful"]);
}

/// The files under `dir` and its subdirectories.
#[track_caller]
fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = vec![];
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}
