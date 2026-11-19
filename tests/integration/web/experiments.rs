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

/// A web UI over the pages of a [`Pages`], with `serve` showing their
/// experiments as it was told.
struct Showing {
    router: axum::Router,
}

impl Routed for Showing {
    fn router(&self) -> &axum::Router {
        &self.router
    }
}

/// `pages`' web UI, with `serve` showing experiment 1 as `shown`.
#[track_caller]
fn showing(pages: &Pages, shown: linewright::benchmarks::Shown) -> Showing {
    let (publish, links) = linewright::api::Publish::new();
    publish
        .experiments
        .shown(linewright::experiments::runner::ExperimentsShown {
            experiments: std::collections::BTreeMap::from([("1".parse().unwrap(), shown)]),
            in_flight: 0,
            max_replays: 2,
        });
    let sources = Sources {
        experiments: links.experiments,
        ..pages.sources.clone()
    };
    Showing {
        router: web::router(sources, UiHosts::default(), pages.stopping.clone()),
    }
}

/// The experiment page's heading as a reader reads it: its name, its state's
/// badge, and the labels of the buttons at its right.
#[track_caller]
fn experiment_heading(html: &Html) -> (String, Vec<String>, Vec<String>) {
    let heading = select(html, "#experiment header")[0];
    (
        text(select_in(heading, "h1")[0]),
        select_in(heading, ".badge").into_iter().map(text).collect(),
        select_in(heading, ":scope > div > button")
            .into_iter()
            .map(text)
            .collect(),
    )
}

/// The experiment page's facts (`docs/specs/web/benchmarks.md`,
/// Experiment), each its `stat-title` and its `stat-value`.
#[track_caller]
fn experiment_facts(html: &Html) -> Vec<(String, String)> {
    select(html, "#experiment .stats > .stat")
        .into_iter()
        .map(|stat| {
            (
                text(select_in(stat, ".stat-title")[0]),
                text(select_in(stat, ".stat-value")[0]),
            )
        })
        .collect()
}

/// `docs/specs/benchmarks.md`, Experiment, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn a_waiting_experiments_heading_and_facts_say_what_it_waits_for() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    assert_eq!(
        experiment_heading(&html),
        (
            "Experiment 1".into(),
            vec!["waiting".into()],
            vec!["Cancel".into()]
        )
    );
    assert_eq!(
        experiment_facts(&html),
        [
            ("Launched".into(), "2026-11-17 12:00:00 UTC".into()),
            ("Duration".into(), "0s".into()),
            ("Replays".into(), "6 of 8".into()),
            ("Cost".into(), "$1.61, 1 without a cost".into()),
            ("Judges".into(), "$0.11".into()),
            ("Waiting for".into(), "serve".into()),
        ]
    );
    assert_eq!(
        select(&html, "#experiment .stat time[data-since]")[0].attr("data-since"),
        Some("2026-11-17T12:00:00Z"),
        "an open experiment's duration is kept up"
    );
    assert_eq!(
        select(&html, "#experiment .stats .badge"),
        [] as [scraper::ElementRef<'_>; 0]
    );
    let state = select(&html, "#experiment header .badge")[0];
    assert!(has_classes(state, &["badge-outline"]), "{}", state.html());
    assert_eq!(events_path(&html), Some("/events/experiments/1"));
    assert_no_status_by_colour_alone(&html);

    let limited = showing(
        &pages,
        linewright::benchmarks::Shown::Waiting(linewright::benchmarks::WaitingFor::Hold(
            linewright::holds::ExperimentHold::UsageLimit {
                resets_at: "2026-11-17T14:30:00Z".parse().unwrap(),
            },
        )),
    )
    .page("/experiments/1")
    .await;
    assert_eq!(
        experiment_facts(&limited).last().unwrap().1,
        "the usage limit, until 2026-11-17 14:30:00 UTC"
    );
}

/// `docs/specs/benchmarks.md`, Experiment, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn a_running_experiments_heading_offers_to_cancel_it_and_its_facts_wait_for_nothing() {
    let pages = with_experiment().await;

    let html = showing(&pages, linewright::benchmarks::Shown::Running)
        .page("/experiments/1")
        .await;

    assert_eq!(
        experiment_heading(&html),
        (
            "Experiment 1".into(),
            vec!["running".into()],
            vec!["Cancel".into()]
        )
    );
    let labels: Vec<String> = experiment_facts(&html)
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    assert_eq!(
        labels,
        ["Launched", "Duration", "Replays", "Cost", "Judges"]
    );
}

/// `docs/specs/benchmarks.md`, Experiment, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn an_ended_experiment_says_when_it_ended_and_how_long_it_took() {
    let pages = with_experiment().await;
    for key in [(2, 1, 0), (2, 1, 1)] {
        trial(
            &pages.path(""),
            1,
            key,
            "state = 'unjudged', outcome = 'failed', seconds = 5, unjudged = 'outcome'",
        )
        .await;
    }
    linewright::benchmarks::Experiments::new(pages.sources.runs.database().clone())
        .end(
            "1".parse().unwrap(),
            "2026-11-17T13:30:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    let html = pages.page("/experiments/1").await;

    assert_eq!(
        experiment_heading(&html),
        ("Experiment 1".into(), vec!["ended".into()], vec![])
    );
    assert_eq!(
        experiment_facts(&html),
        [
            ("Launched".into(), "2026-11-17 12:00:00 UTC".into()),
            ("Ended".into(), "2026-11-17 13:30:00 UTC".into()),
            ("Duration".into(), "1h 30m".into()),
            ("Replays".into(), "8 of 8".into()),
            ("Cost".into(), "$1.61, 3 without a cost".into()),
            ("Judges".into(), "$0.11".into()),
        ]
    );
    assert_eq!(
        select(&html, "#experiment .stat time[data-since]"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

/// `docs/specs/design_system.md`, Screen states, Absent: an experiment that
/// ended before its end was kept.
#[test_log::test(tokio::test)]
async fn an_experiment_whose_end_was_not_kept_shows_its_end_and_duration_absent() {
    let pages = with_experiment().await;
    let pool = back_door(&pages.path("")).await;
    sqlx::query("UPDATE experiments SET state = 'ended', ended_at = NULL WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();

    let html = pages.page("/experiments/1").await;

    let facts = experiment_facts(&html);
    assert_eq!(facts[1], ("Ended".into(), "–".into()));
    assert_eq!(facts[2], ("Duration".into(), "–".into()));
}

/// `docs/specs/benchmarks.md`, Experiment, Heading, and Cancel experiment;
/// `docs/specs/design_system.md`, Confirming.
#[test_log::test(tokio::test)]
async fn the_experiment_page_offers_to_cancel_an_open_experiment_and_cancelling_ends_it() {
    let pages = with_experiment().await;
    let html = pages.page("/experiments/1").await;
    assert_eq!(
        confirmations(&html),
        [Confirmation {
            asking: "Cancel".into(),
            question: "Cancel experiment 1? This stops its running replays and judges.".into(),
            turn_down: "Keep running".into(),
            action: "/experiments/1/cancel".into(),
            fields: vec![],
        }]
    );

    let (status, answer) = post(&pages, "/experiments/1/cancel", &[SAME_ORIGIN], &[]).await;
    let (again, refused) = post(&pages, "/experiments/1/cancel", &[SAME_ORIGIN], &[]).await;
    let (other_site, _) = post(&pages, "/experiments/1/cancel", &[CROSS_SITE], &[]).await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(said(&answer), "Cancelled experiment 1");
    assert_eq!(again, 422);
    assert_eq!(said(&refused), "Not cancelled: experiment 1 is cancelled");
    assert_eq!(other_site, 403);
    let cancelled = pages.page("/experiments/1").await;
    assert_eq!(
        experiment_heading(&cancelled),
        ("Experiment 1".into(), vec!["cancelled".into()], vec![])
    );
    assert_eq!(confirmations(&cancelled), []);
}

/// `docs/specs/benchmarks.md`, Experiment, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn a_cancelled_experiment_says_when_it_was_cancelled_and_how_long_it_took() {
    let pages = with_experiment().await;
    linewright::benchmarks::Experiments::new(pages.sources.runs.database().clone())
        .cancel(
            "1".parse().unwrap(),
            "2026-11-17T14:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    let html = pages.page("/experiments/1").await;

    assert_eq!(
        experiment_heading(&html),
        ("Experiment 1".into(), vec!["cancelled".into()], vec![])
    );
    assert_eq!(
        experiment_facts(&html),
        [
            ("Launched".into(), "2026-11-17 12:00:00 UTC".into()),
            ("Ended".into(), "2026-11-17 14:00:00 UTC".into()),
            ("Duration".into(), "2h".into()),
            ("Replays".into(), "6 of 8".into()),
            ("Cost".into(), "$1.61, 1 without a cost".into()),
            ("Judges".into(), "$0.11".into()),
        ]
    );
}

/// A point of the chart as a reader meets it: its id, its name, and the
/// details its tooltip shows.
#[track_caller]
fn points(html: &Html, axis: &str) -> Vec<(String, String, String)> {
    select(html, &format!("#chart [data-axis={axis}] button.point"))
        .into_iter()
        .map(|point| {
            (
                point.attr("id").unwrap().to_string(),
                text(point),
                point.attr("data-tip").unwrap().to_string(),
            )
        })
        .collect()
}

/// Where the point `id` is drawn in its chart's SVG: the `x` and the `y`
/// of the `foreignObject` that holds it.
#[track_caller]
fn drawn_at(html: &Html, id: &str) -> (f64, f64) {
    let drawn = select(html, "#chart foreignObject")
        .into_iter()
        .find(|drawn| !select_in(*drawn, &format!("#{id}")).is_empty())
        .unwrap_or_else(|| panic!("no point {id}: {}", html.html()));
    let at = |attribute| drawn.attr(attribute).unwrap().parse::<f64>().unwrap();
    (at("x"), at("y"))
}

/// `docs/specs/benchmarks.md`, Experiment, The chart and A point;
/// `docs/specs/web/benchmarks.md`, Experiment.
#[test_log::test(tokio::test)]
async fn the_chart_puts_each_configuration_at_its_pass_rate_against_the_mean_the_reader_chooses() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    let chooser = select(&html, "#experiment-chart select")[0];
    let options: Vec<(String, bool)> = select_in(chooser, "option")
        .into_iter()
        .map(|option| (text(option), option.attr("selected").is_some()))
        .collect();
    assert_eq!(
        options,
        [("Mean cost".into(), true), ("Mean time".into(), false)]
    );
    let label = select(&html, "#experiment-chart label")[0];
    assert!(text(label).starts_with("Pass rate vs"), "{}", label.html());
    assert!(
        select(
            &html,
            "#chart select, #experiment select, #configurations select"
        )
        .is_empty(),
        "the choice is outside every region an update morphs"
    );
    assert_eq!(
        points(&html, "cost"),
        [
            (
                "point-cost-as-is".into(),
                "as-is".into(),
                "as-is: pass rate 100%, mean cost $0.40".into()
            ),
            (
                "point-cost-short".into(),
                "short".into(),
                "short: pass rate 0%, mean cost $0.23".into()
            ),
        ]
    );
    assert_eq!(
        points(&html, "time"),
        [
            (
                "point-time-as-is".into(),
                "as-is".into(),
                "as-is: pass rate 100%, mean time 50s".into()
            ),
            (
                "point-time-short".into(),
                "short".into(),
                "short: pass rate 0%, mean time 23s".into()
            ),
        ]
    );
    for axis in ["cost", "time"] {
        let (as_is, short) = (
            drawn_at(&html, &format!("point-{axis}-as-is")),
            drawn_at(&html, &format!("point-{axis}-short")),
        );
        assert!(
            as_is.0 > short.0,
            "the higher mean is to the right on {axis}"
        );
        assert!(
            as_is.1 < short.1,
            "the higher pass rate is higher up on {axis}"
        );
        assert!(
            !select(&html, &format!("#chart [data-axis={axis}] svg text")).is_empty(),
            "its ticks"
        );
    }
    for point in select(&html, "#chart button.point") {
        assert_eq!(point.attr("type"), Some("button"));
    }
    assert!(
        select(&html, "main [style]").is_empty(),
        "the page's Content-Security-Policy refuses inline styles"
    );
}

/// `docs/specs/benchmarks.md`, Experiment, A point.
#[test_log::test(tokio::test)]
async fn a_chart_with_no_points_says_no_results_with_no_ticks() {
    let pages = with_experiment().await;
    launched(&pages, "board", &["short"], 1, "2026-11-17T13:00:00Z").await;

    let html = pages.page("/experiments/2").await;

    for axis in ["cost", "time"] {
        let plot = select(&html, &format!("#chart [data-axis={axis}]"))[0];
        assert_eq!(text(plot), "No results");
        assert!(
            select_in(plot, "svg text, button").is_empty(),
            "{}",
            plot.html()
        );
    }
}

/// The configurations table's rows that are not under another, and those
/// under the row `of`, each its id, whether it is hidden and its cells'
/// text.
#[track_caller]
fn fold_rows(html: &Html, of: Option<&str>) -> Vec<(String, bool, Vec<String>)> {
    let selector = match of {
        None => "#configurations tbody tr:not([data-fold-of])".to_string(),
        Some(of) => format!("#configurations tbody tr[data-fold-of='{of}']"),
    };
    select(html, &selector)
        .into_iter()
        .map(|row| {
            (
                row.attr("id").unwrap().to_string(),
                row.attr("hidden").is_some(),
                select_in(row, ":scope > th, :scope > td")
                    .into_iter()
                    .map(text)
                    .collect(),
            )
        })
        .collect()
}

/// `docs/specs/benchmarks.md`, Experiment, The table;
/// `docs/specs/design_system.md`, Tables.
#[test_log::test(tokio::test)]
async fn the_configurations_table_shows_each_configurations_figures() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    assert!(
        table_bars(&html).contains(&("Configurations".into(), vec![])),
        "{:?}",
        table_bars(&html)
    );
    let heads: Vec<String> = select(&html, "#configurations thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(
        heads,
        [
            "",
            "Pass rate",
            "Mean cost",
            "Total cost",
            "Mean time",
            "Total time",
            "Leaks",
            "Unjudged"
        ]
    );
    assert_eq!(
        fold_rows(&html, None),
        [
            (
                "configuration-as-is".into(),
                false,
                vec![
                    "as-is".into(),
                    "100%".into(),
                    "$0.40".into(),
                    "$0.80, 1 without a cost".into(),
                    "50s".into(),
                    "2m 30s".into(),
                    "0".into(),
                    "1".into()
                ]
            ),
            (
                "configuration-short".into(),
                false,
                vec![
                    "short".into(),
                    "0%".into(),
                    "$0.23".into(),
                    "$0.70".into(),
                    "23s".into(),
                    "1m 10s".into(),
                    "1".into(),
                    "1".into()
                ]
            ),
        ]
    );
    let alignments = alignments(&html, "#configurations");
    let expected = [
        "start", "start", "end", "end", "end", "end", "start", "start",
    ];
    assert_eq!(alignments[0], expected);
    assert_eq!(alignments[1], expected);
    assert_eq!(
        select(&html, "#configuration-as-is > th:first-child > button")[0].attr("aria-controls"),
        Some("configuration-as-is_case-columns configuration-as-is_case-lanes")
    );
    assert_eq!(select(&html, "#configurations details").len(), 0);
}

/// `docs/specs/benchmarks.md`, Experiment, The table: a configuration opens
/// to its cases, and a case to its replays, in the order the experiment
/// starts them.
#[test_log::test(tokio::test)]
async fn a_configuration_opens_to_its_cases_and_a_case_to_its_replays() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    let row = |id: &str, cells: &[&str]| {
        (
            id.to_string(),
            true,
            cells.iter().map(ToString::to_string).collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        fold_rows(&html, Some("configuration-as-is")),
        [
            row(
                "configuration-as-is_case-columns",
                &["columns", "2 of 2", "$0.40", "$0.80", "1m", "2m", "0", "0"]
            ),
            row(
                "configuration-as-is_case-lanes",
                &[
                    "lanes",
                    "–",
                    "–",
                    "$0.00, 1 without a cost",
                    "30s",
                    "30s",
                    "0",
                    "1"
                ]
            ),
        ]
    );
    assert_eq!(
        fold_rows(&html, Some("configuration-short")),
        [
            row(
                "configuration-short_case-columns",
                &["columns", "–", "$0.25", "$0.50", "25s", "50s", "1", "1"]
            ),
            row(
                "configuration-short_case-lanes",
                &["lanes", "0 of 1", "$0.20", "$0.20", "20s", "20s", "0", "0"]
            ),
        ]
    );
    let replay = |id: &str, name: &str, said: &str, cost: &str, time: &str| {
        row(id, &[name, said, cost, "", time, "", "", ""])
    };
    let replays = |of: &str| fold_rows(&html, Some(of));
    assert_eq!(
        replays("configuration-as-is_case-columns"),
        [
            replay(
                "configuration-as-is_case-columns_replay-1",
                "Replay 1",
                "pass",
                "$0.30",
                "1m 30s"
            ),
            replay(
                "configuration-as-is_case-columns_replay-2",
                "Replay 2",
                "pass",
                "$0.50",
                "30s"
            ),
        ]
    );
    assert_eq!(
        replays("configuration-as-is_case-lanes"),
        [
            replay(
                "configuration-as-is_case-lanes_replay-1",
                "Replay 1",
                "no judgement: its run is not done or handed back",
                "–",
                "30s"
            ),
            replay(
                "configuration-as-is_case-lanes_replay-2",
                "Replay 2",
                "replaying",
                "–",
                "–"
            ),
        ]
    );
    assert_eq!(
        replays("configuration-short_case-columns"),
        [
            replay(
                "configuration-short_case-columns_replay-1",
                "Replay 1",
                "fail, with a leak",
                "$0.10",
                "10s"
            ),
            replay(
                "configuration-short_case-columns_replay-2",
                "Replay 2",
                "no judgement: the judge made none: it gave up",
                "$0.40",
                "40s"
            ),
        ]
    );
    assert_eq!(
        replays("configuration-short_case-lanes"),
        [
            replay(
                "configuration-short_case-lanes_replay-1",
                "Replay 1",
                "fail",
                "$0.20",
                "20s"
            ),
            replay(
                "configuration-short_case-lanes_replay-2",
                "Replay 2",
                "waiting",
                "–",
                "–"
            ),
        ]
    );
    assert_eq!(
        text(select(&html, "#configuration-as-is_case-columns_replay-1 > th")[0]),
        "Replay 1"
    );
    let waiting = select(&html, "#configuration-short_case-lanes_replay-2 .badge")[0];
    assert!(
        has_classes(waiting, &["badge-outline"]),
        "{}",
        waiting.html()
    );
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/benchmarks.md`, Experiment, A replay.
#[test_log::test(tokio::test)]
async fn a_replay_opens_to_its_judges_reasoning_and_the_transcripts_that_have_started() {
    let pages = with_experiment().await;
    pages.write(&[("experiments/1/short/columns/2/case", "1/implement-1\n")]);

    let html = pages.page("/experiments/1").await;

    let opens = select(
        &html,
        "#configuration-as-is_case-columns_replay-1 > th > button",
    );
    assert_eq!(
        opens[0].attr("aria-controls"),
        Some("configuration-as-is_case-columns_replay-1_details")
    );
    let details = select(&html, "#configuration-as-is_case-columns_replay-1_details")[0];
    assert_eq!(
        details.attr("data-fold-of"),
        Some("configuration-as-is_case-columns_replay-1")
    );
    assert!(details.attr("hidden").is_some());
    let reasoning: Vec<String> = select_in(details, ".reasoning p")
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
        hrefs(select_in(details, "a")),
        [
            "/experiments/1/as-is/columns/1/run",
            "/experiments/1/as-is/columns/1/judge"
        ]
    );
    let judge_made_none = select(&html, "#configuration-short_case-columns_replay-2_details")[0];
    assert_eq!(
        hrefs(select_in(judge_made_none, "a")),
        [
            "/experiments/1/short/columns/2/run",
            "/experiments/1/short/columns/2/judge"
        ],
        "a judge that made no judgement had started"
    );
    assert!(
        select(&html, "#configuration-short_case-lanes_replay-2 button").is_empty(),
        "a replay with nothing to open to has no button"
    );
    assert_eq!(
        select(&html, "#configuration-short_case-lanes_replay-2_details"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

/// `docs/specs/benchmarks.md`, Experiment, The table and The grid: highest
/// pass rate first, ties in the experiment's order, and those with none
/// last.
#[test_log::test(tokio::test)]
async fn configurations_are_in_the_order_of_their_pass_rates_in_the_table_and_the_grid() {
    let pages = with_suites().await;
    launched(&pages, "board", &["a", "b", "c"], 1, "2026-11-17T12:00:00Z").await;
    let work_dir = pages.path("");
    let pass = "state = 'judged', outcome = 'done', seconds = 10, correctness = 'pass', leak = 0";
    let fail = "state = 'judged', outcome = 'done', seconds = 10, correctness = 'fail', leak = 0";
    for (key, set) in [
        ((1, 0, 1), pass),
        ((1, 1, 1), fail),
        ((1, 0, 2), pass),
        ((1, 1, 2), pass),
        ((1, 0, 3), fail),
        ((1, 1, 3), pass),
    ] {
        trial(&work_dir, 1, key, set).await;
    }

    let html = pages.page("/experiments/1").await;

    let order: Vec<String> = fold_rows(&html, None)
        .into_iter()
        .map(|(_, _, cells)| format!("{} {}", cells[0], cells[1]))
        .collect();
    assert_eq!(order, ["b 100%", "a 50%", "c 50%", "as-is –"]);
    let heads: Vec<String> = select(&html, "#grid thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["", "b", "a", "c", "as-is"]);
    assert_eq!(
        rows(&html, "#grid"),
        [
            ["columns", "1 of 1", "1 of 1", "0 of 1", "–"],
            ["lanes", "1 of 1", "0 of 1", "1 of 1", "–"],
        ]
    );
    assert_eq!(
        points(&html, "cost"),
        Vec::<(String, String, String)>::new(),
        "no configuration has a cost"
    );
    let named: Vec<String> = points(&html, "time")
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();
    assert_eq!(named, ["b", "a", "c"]);
}

/// `docs/specs/benchmarks.md`, Experiment, The grid.
#[test_log::test(tokio::test)]
async fn the_grid_shows_the_passes_leaks_and_unjudged_of_each_case_and_configuration() {
    let pages = with_experiment().await;

    let html = pages.page("/experiments/1").await;

    assert!(
        table_bars(&html).contains(&("Passes by case".into(), vec![])),
        "{:?}",
        table_bars(&html)
    );
    let heads: Vec<String> = select(&html, "#grid thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["", "as-is", "short"]);
    assert_eq!(
        rows(&html, "#grid"),
        [
            ["columns", "2 of 2", "–, 1 leak, 1 unjudged"],
            ["lanes", "–, 1 unjudged", "0 of 1"],
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
        let sent = regions(&event);
        let replays = sent
            .iter()
            .filter(|(id, _)| id == "experiment")
            .any(|(_, experiment)| {
                experiment_facts(experiment).contains(&("Replays".into(), "7 of 8".into()))
            });
        let row = sent
            .iter()
            .filter(|(id, _)| id == "configurations")
            .any(|(_, table)| {
                text(select(table, "#configuration-as-is_case-lanes_replay-2 td")[0])
                    .starts_with("no judgement")
            });
        if replays && row {
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
    assert_eq!(heads, ["", "as-is", "short", "careful"]);
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
