use super::benchmarks::with_benchmarks;
use super::*;

use std::panic::Location;

use serde_json::json;

/// Launches a benchmark run on the benchmark `benchmark` of `pages` under
/// `as-is` and a configuration setting the model for each of `configurations`,
/// each run `repetitions` times, as `lw benchmark-run launch` does, and sets its
/// launch to `at`: its ID.
#[track_caller]
pub(super) fn launched<'a>(
    pages: &'a Pages,
    benchmark: &str,
    configurations: &[&str],
    repetitions: u32,
    at: &str,
) -> impl Future<Output = i64> + 'a {
    let caller = Location::caller();
    let benchmark = benchmark.to_string();
    let configurations: Vec<_> = configurations
        .iter()
        .map(|name| {
            let files = std::collections::BTreeMap::from([(
                "config.toml".to_string(),
                b"model = \"haiku\"\n".to_vec(),
            )]);
            (
                name.parse().unwrap(),
                linewright::trial::Configuration::parse(files).unwrap(),
            )
        })
        .collect();
    let at = at.parse().unwrap();
    async move {
        let benchmark_runs =
            linewright::benchmarks::BenchmarkRuns::new(pages.sources.runs.database().clone());
        let launched = benchmark_runs
            .launch(
                &benchmark.parse().unwrap(),
                configurations,
                std::num::NonZeroU32::new(repetitions).unwrap(),
                at,
            )
            .await
            .unwrap_or_else(|e| panic!("launching at {caller}: {e:#}"));
        launched.id.to_string().parse().unwrap()
    }
}

/// [`with_benchmarks`]'s pages, with benchmark run 1 on the benchmark `board`,
/// its tasks `columns` and `lanes` run twice under `as-is` and `short`,
/// launched at noon: of its eight trials, six have ended, with passes,
/// failures, a leak and two without a score, one runs and one is pending. The
/// first trial's output holds its run and its score.
#[track_caller]
pub(super) fn with_benchmark_run() -> impl Future<Output = Pages> {
    let caller = Location::caller();
    async move {
        let pages = with_benchmarks().await;
        launched(&pages, "board", &["short"], 2, "2026-11-17T12:00:00Z").await;
        let work_dir = pages.path("");
        let trials: [((i64, i64, i64), &'static str); 7] = [
            (
                (1, 0, 0),
                "state = 'scored', outcome = 'done', money = 0.3, seconds = 90, \
             correctness = 'pass', leak = 0, scorer_money = 0.05",
            ),
            (
                (1, 0, 1),
                "state = 'scored', outcome = 'handed-back', money = 0.1, seconds = 10, \
             correctness = 'fail', leak = 1, scorer_money = 0.02",
            ),
            (
                (1, 1, 0),
                "state = 'unscored', outcome = 'failed', seconds = 30, unscored = 'outcome'",
            ),
            (
                (1, 1, 1),
                "state = 'scored', outcome = 'done', money = 0.2, seconds = 20, \
             correctness = 'fail', leak = 0, scorer_money = 0.03",
            ),
            (
                (2, 0, 0),
                "state = 'scored', outcome = 'done', money = 0.5, seconds = 30, \
             correctness = 'pass', leak = 0",
            ),
            (
                (2, 0, 1),
                "state = 'unscored', outcome = 'done', money = 0.4, seconds = 40, \
             unscored = 'scorer', reason = 'it gave up', scorer_money = 0.01",
            ),
            ((2, 1, 0), "state = 'running'"),
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
        let mut session = json!({"record": "scorer/20261117T120200Z-scorer", "cost": {}});
        session
            .as_object_mut()
            .unwrap_or_else(|| panic!("a JSON object at {caller}"))
            .extend(ran.as_object().unwrap().clone());
        let score = json!({
            "correctness": {"reasoning": "It names the columns.", "verdict": "pass"},
            "leak": {"reasoning": "I read every transcript.", "found": false, "calls": []},
            "session": session,
            "linewright": "0.1.0",
        });
        let output = "benchmark-runs/1/as-is/columns/1";
        let turn = include_str!("../fixtures/transcript/turn-1.jsonl");
        pages.write(&[
            (&format!("{output}/task"), "1/implement-1\n"),
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
            (&format!("{output}/score.json"), &score.to_string()),
            (
                &format!("{output}/scorer/20261117T120200Z-scorer/output.jsonl"),
                turn,
            ),
            (
                &format!("{output}/scorer/20261117T120200Z-scorer/brief.md"),
                "# Your task\n\nScore the trial.\n",
            ),
        ]);
        pages
    }
}

/// `docs/specs/benchmarks.md`, Benchmark, Runs; `docs/specs/design_system.md`,
/// Tables and Status.
#[test_log::test(tokio::test)]
async fn the_benchmark_page_lists_its_benchmark_runs_latest_first_linking_to_each() {
    let pages = with_benchmark_run().await;
    launched(&pages, "board", &[], 1, "2026-11-17T13:00:00Z").await;
    linewright::benchmarks::BenchmarkRuns::new(pages.sources.runs.database().clone())
        .cancel(
            "2".parse().unwrap(),
            "2026-11-17T14:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    let html = pages.page("/benchmarks/board").await;

    let heads: Vec<String> = select(&html, "#benchmark-runs thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(
        heads,
        ["", "State", "Tasks", "Configurations", "Duration", "Cost"]
    );
    assert_eq!(
        rows(&html, "#benchmark-runs"),
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
    let launches: Vec<&str> = select(&html, "#benchmark-runs tbody tr > th:first-child a")
        .into_iter()
        .filter_map(|link| link.attr("href"))
        .collect();
    assert_eq!(launches, ["/benchmark-runs/2", "/benchmark-runs/1"]);
    assert_eq!(
        select(
            &html,
            "#benchmark-runs tbody tr:nth-child(2) time[data-since]"
        )[0]
        .attr("data-since"),
        Some("2026-11-17T12:00:00Z")
    );
    for state in select(&html, "#benchmark-runs tbody td:nth-child(2)") {
        assert_eq!(select_in(state, ".badge").len(), 1, "{}", state.html());
    }
    assert_no_status_by_colour_alone(&html);
    let alignments = alignments(&html, "#benchmark-runs");
    assert!(
        alignments
            .iter()
            .all(|row| row == &["start", "start", "start", "start", "end", "end"]),
        "{alignments:?}"
    );
}

/// `docs/specs/benchmarks.md`, Benchmark, Runs, and New benchmark run.
#[test_log::test(tokio::test)]
async fn new_benchmark_run_shows_in_the_benchmark_runs_bar_only_on_a_benchmark_with_tasks() {
    let pages = with_benchmarks().await;

    let board = pages.page("/benchmarks/board").await;
    let empty = pages.page("/benchmarks/triage-basics").await;

    let new = select(
        &board,
        "#new-benchmark-run-offer button[command=show-modal]",
    );
    assert_eq!(new.len(), 1, "{}", board.html());
    assert_eq!(text(new[0]), "New");
    let dialog = new[0].attr("commandfor").unwrap();
    assert_eq!(
        select(&board, &format!("dialog[id='{dialog}'] form.launch")).len(),
        1
    );
    assert_eq!(
        select(&empty, "#new-benchmark-run-offer button").len(),
        0,
        "{}",
        empty.html()
    );
}

/// A web UI over the pages of a [`Pages`], with `serve` showing their
/// benchmark runs as it was told.
struct Showing {
    router: axum::Router,
}

impl Routed for Showing {
    fn router(&self) -> &axum::Router {
        &self.router
    }
}

/// `pages`' web UI, with `serve` showing benchmark run 1 as `shown`.
#[track_caller]
fn showing(pages: &Pages, shown: linewright::benchmarks::Shown) -> Showing {
    let (publish, links) = linewright::api::Publish::new();
    publish
        .benchmark_runs
        .shown(linewright::benchmark_runs::runner::BenchmarkRunsShown {
            benchmark_runs: std::collections::BTreeMap::from([("1".parse().unwrap(), shown)]),
            in_flight: 0,
            max_trials: 2,
        });
    let sources = Sources {
        benchmark_runs: links.benchmark_runs,
        ..pages.sources.clone()
    };
    Showing {
        router: web::router(sources, UiHosts::default(), pages.stopping.clone()),
    }
}

/// The benchmark run page's heading as a reader reads it: its name, its state's
/// badge, and the labels of the screen's buttons.
#[track_caller]
fn benchmark_run_heading(html: &Html) -> (String, Vec<String>, Vec<String>) {
    let heading = select(html, "#heading")[0];
    (
        text(select_in(heading, ".card-title")[0]),
        select_in(heading, ".badge").into_iter().map(text).collect(),
        select(html, "#buttons button")
            .into_iter()
            .map(text)
            .collect(),
    )
}

/// The benchmark run page's facts (`docs/specs/web/benchmarks.md`,
/// Benchmark run), each its `stat-title` and its `stat-value`.
#[track_caller]
fn benchmark_run_facts(html: &Html) -> Vec<(String, String)> {
    select(html, "#benchmark-run .stats > .stat")
        .into_iter()
        .map(|stat| {
            (
                text(select_in(stat, ".stat-title")[0]),
                text(select_in(stat, ".stat-value")[0]),
            )
        })
        .collect()
}

/// `docs/specs/benchmarks.md`, Benchmark run, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn a_waiting_benchmark_runs_heading_and_facts_say_what_it_waits_for() {
    let pages = with_benchmark_run().await;

    let html = pages.page("/benchmark-runs/1").await;

    assert_eq!(
        benchmark_run_heading(&html),
        (
            "Benchmark run 1".into(),
            vec!["waiting".into()],
            vec!["Cancel".into()]
        )
    );
    assert_eq!(
        benchmark_run_facts(&html),
        [
            ("Launched".into(), "2026-11-17 12:00:00 UTC".into()),
            ("Duration".into(), "0s".into()),
            ("Trials".into(), "6 of 8".into()),
            ("Cost".into(), "$1.61, 1 without a cost".into()),
            ("Scorers".into(), "$0.11".into()),
            ("Waiting for".into(), "serve".into()),
        ]
    );
    assert_eq!(
        select(&html, "#benchmark-run .stat time[data-since]")[0].attr("data-since"),
        Some("2026-11-17T12:00:00Z"),
        "an open benchmark run's duration is kept up"
    );
    assert_eq!(
        select(&html, "#benchmark-run .stats .badge"),
        [] as [scraper::ElementRef<'_>; 0]
    );
    let state = select(&html, "#heading .badge")[0];
    assert!(has_classes(state, &["badge-outline"]), "{}", state.html());
    assert_eq!(events_path(&html), Some("/events/benchmark-runs/1?cancels"));
    assert_no_status_by_colour_alone(&html);

    let limited = showing(
        &pages,
        linewright::benchmarks::Shown::Waiting(linewright::benchmarks::WaitingFor::Hold(
            linewright::holds::BenchmarkRunHold::UsageLimit {
                resets_at: "2026-11-17T14:30:00Z".parse().unwrap(),
            },
        )),
    )
    .page("/benchmark-runs/1")
    .await;
    assert_eq!(
        benchmark_run_facts(&limited).last().unwrap().1,
        "the usage limit, until 2026-11-17 14:30:00 UTC"
    );
}

/// `docs/specs/benchmarks.md`, Benchmark run, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn a_running_benchmark_runs_heading_offers_to_cancel_it_and_its_facts_wait_for_nothing() {
    let pages = with_benchmark_run().await;

    let html = showing(&pages, linewright::benchmarks::Shown::Running)
        .page("/benchmark-runs/1")
        .await;

    assert_eq!(
        benchmark_run_heading(&html),
        (
            "Benchmark run 1".into(),
            vec!["running".into()],
            vec!["Cancel".into()]
        )
    );
    let labels: Vec<String> = benchmark_run_facts(&html)
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    assert_eq!(
        labels,
        ["Launched", "Duration", "Trials", "Cost", "Scorers"]
    );
}

/// `docs/specs/benchmarks.md`, Benchmark run, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn an_ended_benchmark_run_says_when_it_ended_and_how_long_it_took() {
    let pages = with_benchmark_run().await;
    for key in [(2, 1, 0), (2, 1, 1)] {
        trial(
            &pages.path(""),
            1,
            key,
            "state = 'unscored', outcome = 'failed', seconds = 5, unscored = 'outcome'",
        )
        .await;
    }
    linewright::benchmarks::BenchmarkRuns::new(pages.sources.runs.database().clone())
        .end(
            "1".parse().unwrap(),
            "2026-11-17T13:30:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    let html = pages.page("/benchmark-runs/1").await;

    assert_eq!(
        benchmark_run_heading(&html),
        ("Benchmark run 1".into(), vec!["ended".into()], vec![])
    );
    assert_eq!(
        benchmark_run_facts(&html),
        [
            ("Launched".into(), "2026-11-17 12:00:00 UTC".into()),
            ("Ended".into(), "2026-11-17 13:30:00 UTC".into()),
            ("Duration".into(), "1h 30m".into()),
            ("Trials".into(), "8 of 8".into()),
            ("Cost".into(), "$1.61, 3 without a cost".into()),
            ("Scorers".into(), "$0.11".into()),
        ]
    );
    assert_eq!(
        select(&html, "#benchmark-run .stat time[data-since]"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

/// `docs/specs/design_system.md`, Screen states, Absent: a benchmark run that
/// ended before its end was kept.
#[test_log::test(tokio::test)]
async fn a_benchmark_run_whose_end_was_not_kept_shows_its_end_and_duration_absent() {
    let pages = with_benchmark_run().await;
    let pool = back_door(&pages.path("")).await;
    sqlx::query("UPDATE benchmark_runs SET state = 'ended', ended_at = NULL WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();

    let html = pages.page("/benchmark-runs/1").await;

    let facts = benchmark_run_facts(&html);
    assert_eq!(facts[1], ("Ended".into(), "–".into()));
    assert_eq!(facts[2], ("Duration".into(), "–".into()));
}

/// `docs/specs/benchmarks.md`, Benchmark run, Heading, and Cancel benchmark
/// run; `docs/specs/design_system.md`, Confirming.
#[test_log::test(tokio::test)]
async fn the_benchmark_run_page_offers_to_cancel_an_open_benchmark_run_and_cancelling_ends_it() {
    let pages = with_benchmark_run().await;
    let html = pages.page("/benchmark-runs/1").await;
    assert_eq!(
        confirmations(&html),
        [Confirmation {
            asking: "Cancel".into(),
            question: "Cancel benchmark run 1? This stops its running trials and scorers.".into(),
            turn_down: "Keep running".into(),
            action: "/benchmark-runs/1/cancel".into(),
            fields: vec![],
        }]
    );

    let (status, answer) = post(&pages, "/benchmark-runs/1/cancel", &[SAME_ORIGIN], &[]).await;
    let (again, refused) = post(&pages, "/benchmark-runs/1/cancel", &[SAME_ORIGIN], &[]).await;
    let (other_site, _) = post(&pages, "/benchmark-runs/1/cancel", &[CROSS_SITE], &[]).await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(said(&answer), "Cancelled benchmark run 1");
    assert_eq!(again, 422);
    assert_eq!(
        said(&refused),
        "Not cancelled: benchmark run 1 is cancelled"
    );
    assert_eq!(other_site, 403);
    let cancelled = pages.page("/benchmark-runs/1").await;
    assert_eq!(
        benchmark_run_heading(&cancelled),
        ("Benchmark run 1".into(), vec!["cancelled".into()], vec![])
    );
    assert_eq!(confirmations(&cancelled), []);
}

/// `docs/specs/benchmarks.md`, Benchmark run, Heading, State and Facts.
#[test_log::test(tokio::test)]
async fn a_cancelled_benchmark_run_says_when_it_was_cancelled_and_how_long_it_took() {
    let pages = with_benchmark_run().await;
    linewright::benchmarks::BenchmarkRuns::new(pages.sources.runs.database().clone())
        .cancel(
            "1".parse().unwrap(),
            "2026-11-17T14:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    let html = pages.page("/benchmark-runs/1").await;

    assert_eq!(
        benchmark_run_heading(&html),
        ("Benchmark run 1".into(), vec!["cancelled".into()], vec![])
    );
    assert_eq!(
        benchmark_run_facts(&html),
        [
            ("Launched".into(), "2026-11-17 12:00:00 UTC".into()),
            ("Ended".into(), "2026-11-17 14:00:00 UTC".into()),
            ("Duration".into(), "2h".into()),
            ("Trials".into(), "6 of 8".into()),
            ("Cost".into(), "$1.61, 1 without a cost".into()),
            ("Scorers".into(), "$0.11".into()),
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

/// `docs/specs/benchmarks.md`, Benchmark run, The chart and A point;
/// `docs/specs/web/benchmarks.md`, Benchmark run.
#[test_log::test(tokio::test)]
async fn the_chart_puts_each_configuration_at_its_pass_rate_against_the_mean_the_reader_chooses() {
    let pages = with_benchmark_run().await;

    let html = pages.page("/benchmark-runs/1").await;

    let chooser = select(&html, "#benchmark-run-chart select")[0];
    let options: Vec<(String, bool)> = select_in(chooser, "option")
        .into_iter()
        .map(|option| (text(option), option.attr("selected").is_some()))
        .collect();
    assert_eq!(
        options,
        [("Mean cost".into(), true), ("Mean time".into(), false)]
    );
    let label = select(&html, "#benchmark-run-chart label")[0];
    assert!(text(label).starts_with("Pass rate vs"), "{}", label.html());
    assert!(
        select(
            &html,
            "#chart select, #benchmark-run select, #configurations select"
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

/// `docs/specs/benchmarks.md`, Benchmark run, A point.
#[test_log::test(tokio::test)]
async fn a_chart_with_no_points_says_no_results_with_no_ticks() {
    let pages = with_benchmark_run().await;
    launched(&pages, "board", &["short"], 1, "2026-11-17T13:00:00Z").await;

    let html = pages.page("/benchmark-runs/2").await;

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

/// `docs/specs/benchmarks.md`, Benchmark run, The table;
/// `docs/specs/design_system.md`, Tables.
#[test_log::test(tokio::test)]
async fn the_configurations_table_shows_each_configurations_figures() {
    let pages = with_benchmark_run().await;

    let html = pages.page("/benchmark-runs/1").await;

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
            "Unscored"
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
        Some("configuration-as-is_task-columns configuration-as-is_task-lanes")
    );
    assert_eq!(select(&html, "#configurations details").len(), 0);
}

/// `docs/specs/benchmarks.md`, Benchmark run, The table: a configuration opens
/// to its tasks, and a task to its trials, in the order the benchmark run
/// starts them.
#[test_log::test(tokio::test)]
async fn a_configuration_opens_to_its_tasks_and_a_task_to_its_trials() {
    let pages = with_benchmark_run().await;

    let html = pages.page("/benchmark-runs/1").await;

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
                "configuration-as-is_task-columns",
                &["columns", "2 of 2", "$0.40", "$0.80", "1m", "2m", "0", "0"]
            ),
            row(
                "configuration-as-is_task-lanes",
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
                "configuration-short_task-columns",
                &["columns", "–", "$0.25", "$0.50", "25s", "50s", "1", "1"]
            ),
            row(
                "configuration-short_task-lanes",
                &["lanes", "0 of 1", "$0.20", "$0.20", "20s", "20s", "0", "0"]
            ),
        ]
    );
    let trial = |id: &str, name: &str, said: &str, cost: &str, time: &str| {
        row(id, &[name, said, cost, "", time, "", "", ""])
    };
    let trials = |of: &str| fold_rows(&html, Some(of));
    assert_eq!(
        trials("configuration-as-is_task-columns"),
        [
            trial(
                "configuration-as-is_task-columns_trial-1",
                "Trial 1",
                "pass",
                "$0.30",
                "1m 30s"
            ),
            trial(
                "configuration-as-is_task-columns_trial-2",
                "Trial 2",
                "pass",
                "$0.50",
                "30s"
            ),
        ]
    );
    assert_eq!(
        trials("configuration-as-is_task-lanes"),
        [
            trial(
                "configuration-as-is_task-lanes_trial-1",
                "Trial 1",
                "no score: its run is not done or handed back",
                "–",
                "30s"
            ),
            trial(
                "configuration-as-is_task-lanes_trial-2",
                "Trial 2",
                "running",
                "–",
                "–"
            ),
        ]
    );
    assert_eq!(
        trials("configuration-short_task-columns"),
        [
            trial(
                "configuration-short_task-columns_trial-1",
                "Trial 1",
                "fail, with a leak",
                "$0.10",
                "10s"
            ),
            trial(
                "configuration-short_task-columns_trial-2",
                "Trial 2",
                "no score: the scorer made none: it gave up",
                "$0.40",
                "40s"
            ),
        ]
    );
    assert_eq!(
        trials("configuration-short_task-lanes"),
        [
            trial(
                "configuration-short_task-lanes_trial-1",
                "Trial 1",
                "fail",
                "$0.20",
                "20s"
            ),
            trial(
                "configuration-short_task-lanes_trial-2",
                "Trial 2",
                "waiting",
                "–",
                "–"
            ),
        ]
    );
    assert_eq!(
        text(select(&html, "#configuration-as-is_task-columns_trial-1 > th")[0]),
        "Trial 1"
    );
    let waiting = select(&html, "#configuration-short_task-lanes_trial-2 .badge")[0];
    assert!(
        has_classes(waiting, &["badge-outline"]),
        "{}",
        waiting.html()
    );
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/benchmarks.md`, Benchmark run, A trial.
#[test_log::test(tokio::test)]
async fn a_trial_opens_to_its_scorers_reasoning_and_the_transcripts_that_have_started() {
    let pages = with_benchmark_run().await;
    pages.write(&[("benchmark-runs/1/short/columns/2/task", "1/implement-1\n")]);

    let html = pages.page("/benchmark-runs/1").await;

    let opens = select(
        &html,
        "#configuration-as-is_task-columns_trial-1 > th > button",
    );
    assert_eq!(
        opens[0].attr("aria-controls"),
        Some("configuration-as-is_task-columns_trial-1_details")
    );
    let details = select(&html, "#configuration-as-is_task-columns_trial-1_details")[0];
    assert_eq!(
        details.attr("data-fold-of"),
        Some("configuration-as-is_task-columns_trial-1")
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
            "/benchmark-runs/1/as-is/columns/1/run",
            "/benchmark-runs/1/as-is/columns/1/scorer"
        ]
    );
    let scorer_made_none = select(&html, "#configuration-short_task-columns_trial-2_details")[0];
    assert_eq!(
        hrefs(select_in(scorer_made_none, "a")),
        [
            "/benchmark-runs/1/short/columns/2/run",
            "/benchmark-runs/1/short/columns/2/scorer"
        ],
        "a scorer that made no score had started"
    );
    assert!(
        select(&html, "#configuration-short_task-lanes_trial-2 button").is_empty(),
        "a trial with nothing to open to has no button"
    );
    assert_eq!(
        select(&html, "#configuration-short_task-lanes_trial-2_details"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

/// `docs/specs/benchmarks.md`, Benchmark run, The table and The grid: highest
/// pass rate first, ties in the benchmark run's order, and those with none
/// last.
#[test_log::test(tokio::test)]
async fn configurations_are_in_the_order_of_their_pass_rates_in_the_table_and_the_grid() {
    let pages = with_benchmarks().await;
    launched(&pages, "board", &["a", "b", "c"], 1, "2026-11-17T12:00:00Z").await;
    let work_dir = pages.path("");
    let pass = "state = 'scored', outcome = 'done', seconds = 10, correctness = 'pass', leak = 0";
    let fail = "state = 'scored', outcome = 'done', seconds = 10, correctness = 'fail', leak = 0";
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

    let html = pages.page("/benchmark-runs/1").await;

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

/// `docs/specs/benchmarks.md`, Benchmark run, The grid.
#[test_log::test(tokio::test)]
async fn the_grid_shows_the_passes_leaks_and_unscored_of_each_task_and_configuration() {
    let pages = with_benchmark_run().await;

    let html = pages.page("/benchmark-runs/1").await;

    assert!(
        table_bars(&html).contains(&("Passes by task".into(), vec![])),
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
            ["columns", "2 of 2", "–, 1 leak, 1 unscored"],
            ["lanes", "–, 1 unscored", "0 of 1"],
        ]
    );
}

#[test_log::test(tokio::test)]
async fn a_trial_opens_to_the_transcripts_of_its_run_and_of_its_scorer() {
    let pages = with_benchmark_run().await;

    let run = pages.page("/benchmark-runs/1/as-is/columns/1/run").await;
    let scorer = pages.page("/benchmark-runs/1/as-is/columns/1/scorer").await;

    assert_eq!(
        text(select(&run, "#heading .card-title")[0]),
        "Trial as-is/columns/1's run"
    );
    assert_eq!(
        text(select(&run, "#trial")[0]),
        "Trial as-is/columns/1 of benchmark run 1: its run's transcript, or its scorer's"
    );
    assert_eq!(
        hrefs(select(&run, "#trial a")),
        [
            "/benchmark-runs/1",
            "/benchmark-runs/1/as-is/columns/1/scorer"
        ]
    );
    assert!(!select(&run, "#s1 .entry").is_empty(), "{}", run.html());
    assert_eq!(
        text(select(&scorer, "#heading .card-title")[0]),
        "Trial as-is/columns/1's scorer"
    );
    assert_eq!(
        text(select(&scorer, "#heading .line")[0]),
        "Trial as-is/columns/1 of benchmark run 1: its scorer's transcript, or its run's"
    );
    assert!(
        !select(&scorer, "#s1 .entry").is_empty(),
        "{}",
        scorer.html()
    );
    assert_eq!(
        events_path(&scorer),
        Some("/events/benchmark-runs/1/as-is/columns/1/scorer")
    );
}

#[test_log::test(tokio::test)]
async fn a_trial_with_no_output_or_scorer_answers_404() {
    let pages = with_benchmark_run().await;

    let (pending, _) = pages.get("/benchmark-runs/1/short/lanes/2/run").await;
    let (no_scorer, _) = pages.get("/benchmark-runs/1/as-is/columns/2/scorer").await;
    let (no_task, _) = pages.get("/benchmark-runs/1/as-is/rows/1/run").await;
    let (no_benchmark_run, _) = pages.get("/benchmark-runs/9").await;

    assert_eq!([pending, no_scorer, no_task, no_benchmark_run], [404; 4]);
}

/// `docs/specs/design_system.md`, Screen states, Nothing there.
#[test_log::test(tokio::test)]
async fn the_page_of_no_benchmark_run_says_there_is_none() {
    let pages = with_benchmark_run().await;

    let (no_benchmark_run, no_benchmark_run_body) = pages.get("/benchmark-runs/9").await;
    let (no_id, no_id_body) = pages.get("/benchmark-runs/first").await;
    let (trial, trial_body) = pages.get("/benchmark-runs/9/as-is/columns/1/run").await;

    for (status, body, region, shown) in [
        (
            no_benchmark_run,
            no_benchmark_run_body,
            "benchmark-run",
            "No such benchmark run 9",
        ),
        (
            no_id,
            no_id_body,
            "benchmark-run",
            "No such benchmark run first",
        ),
        (trial, trial_body, "transcript", "No such benchmark run 9"),
    ] {
        assert_eq!(status, 404);
        let html = Html::parse_document(&body);
        assert_eq!(text(select(&html, &format!("main #{region}"))[0]), shown);
    }
}

#[test_log::test(tokio::test)]
async fn the_benchmark_run_pages_stream_sends_the_benchmark_run_as_it_changes() {
    let pages = with_benchmark_run().await;
    let mut events = stream(&pages, "/events/benchmark-runs/1").await;
    events.recv().await.unwrap();

    trial(
        &pages.path(""),
        1,
        (2, 1, 0),
        "state = 'unscored', outcome = 'failed', seconds = 5, unscored = 'outcome'",
    )
    .await;

    loop {
        let event = events.recv().await.unwrap();
        let sent = regions(&event);
        let trials =
            sent.iter()
                .filter(|(id, _)| id == "benchmark-run")
                .any(|(_, benchmark_run)| {
                    benchmark_run_facts(benchmark_run).contains(&("Trials".into(), "7 of 8".into()))
                });
        let row = sent
            .iter()
            .filter(|(id, _)| id == "configurations")
            .any(|(_, table)| {
                text(select(table, "#configuration-as-is_task-lanes_trial-2 td")[0])
                    .starts_with("no score")
            });
        if trials && row {
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

/// A configuration of the New benchmark run dialog but `as-is`, as a reader
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

/// The New benchmark run dialog's form as a reader meets it: its
/// configurations but `as-is`, its repetitions, how many trials they make,
/// and the refusal above its buttons, if any.
#[derive(Debug, PartialEq)]
struct LaunchShown {
    configurations: Vec<ConfigurationShown>,
    repetitions: String,
    count: String,
    refusal: Option<String>,
}

/// What the form `form` of the New benchmark run dialog shows.
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
        count: text(select(form, ".trial-count")[0]),
        refusal: refusals.first().map(|refusal| text(*refusal)),
    }
}

/// What the reader reads in the New benchmark run dialog's form, in order,
/// leaving out what each configuration but `as-is` holds: its title, each
/// label, `as-is`, each button and how many trials it makes.
fn read_in_order(form: ElementRef) -> Vec<String> {
    select_in(
        form,
        "legend, label, .label, button, .trial-count, fieldset.configuration:not(.as-is)",
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

/// `docs/specs/benchmarks.md`, New benchmark run;
/// `docs/specs/design_system.md`, Dialogs and Making.
#[test_log::test(tokio::test)]
async fn new_opens_the_new_benchmark_run_dialog_with_as_is_first_the_repetitions_and_the_trials_they_make()
 {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/board").await;

    let dialog = select(&html, "dialog#new-benchmark-run");
    assert_eq!(dialog.len(), 1, "{}", html.html());
    let dialog = dialog[0];
    assert!(has_classes(dialog, &["modal"]));
    assert_eq!(select_in(dialog, ".modal-backdrop").len(), 0);
    assert_eq!(select(&html, "#benchmark-runs dialog").len(), 0);
    let legend = select_in(dialog, "form > fieldset > legend.fieldset-legend");
    assert_eq!(text(legend[0]), "New benchmark run on board");
    assert_eq!(dialog.attr("aria-labelledby"), legend[0].attr("id"));
    let form = select_in(dialog, "form[hx-post]")[0];
    assert_eq!(form.attr("hx-post"), Some("/benchmarks/board/launch"));
    assert_eq!(form.attr("hx-target"), Some("this"));
    assert!(form.attr("data-new").is_some());
    assert_eq!(
        read_in_order(form),
        [
            "New benchmark run on board",
            "Configurations",
            "as-is",
            "Add a configuration",
            "Repetitions",
            "1 configuration × 2 tasks × 1 repetition: 2 trials",
            "Cancel",
            "Run benchmark",
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
            count: "1 configuration × 2 tasks × 1 repetition: 2 trials".into(),
            refusal: None,
        }
    );
    let buttons = select_in(form, ".modal-action button");
    assert_eq!(buttons[0].attr("command"), Some("close"));
    assert_eq!(buttons[0].attr("commandfor"), Some("new-benchmark-run"));
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

/// `docs/specs/benchmarks.md`, New benchmark run, Files of the brief: the
/// page's stream sends the texts a run of the stage uses, from which the
/// dialog starts a file.
#[test_log::test(tokio::test)]
async fn the_benchmark_pages_stream_sends_the_texts_of_the_files_of_the_brief() {
    let pages = with_benchmarks().await;
    let mut events = stream(&pages, "/events/benchmarks/board").await;

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
async fn a_launch_on_a_benchmark_with_no_tasks_is_refused() {
    let pages = with_benchmarks().await;

    let (status, answer) = post(
        &pages,
        "/benchmarks/triage-basics/launch",
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
            count: "1 configuration × 0 tasks × 1 repetition: 0 trials".into(),
            refusal: Some("The benchmark triage-basics has no tasks.".into()),
        }
    );
    assert_eq!(
        text(select(&answer, "legend")[0]),
        "New benchmark run on triage-basics"
    );
    assert!(!pages.path("benchmark-runs/1").exists());
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

/// `docs/specs/benchmarks.md`, New benchmark run, Refusals;
/// `docs/specs/design_system.md`, Making, A refusal.
#[test_log::test(tokio::test)]
async fn a_refused_launch_answers_the_form_as_sent_naming_the_configuration_refused() {
    let pages = with_benchmarks().await;
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
            post_answer(&pages, "/benchmarks/board/launch", &[SAME_ORIGIN], &sent).await;
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
        "/benchmarks/board/launch",
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
            count: "2 configurations × 2 tasks × 1 repetition: 4 trials".into(),
            refusal: Some("The repetitions are a whole number, 1 or more.".into()),
        }
    );
    assert!(!pages.path("benchmark-runs/1").exists());
}

/// `docs/specs/design_system.md`, Making, A refusal: the fields keep every
/// configuration, its settings and its files' texts.
#[test_log::test(tokio::test)]
async fn a_refused_launch_keeps_every_configuration_with_its_settings_and_its_files() {
    let pages = with_benchmarks().await;
    let design = format!("{DESIGN}\nCut the work finely.\n");

    let (status, answer) = post(
        &pages,
        "/benchmarks/board/launch",
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
            count: "3 configurations × 2 tasks × 3 repetitions: 18 trials".into(),
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
    assert!(!pages.path("benchmark-runs/1").exists());
}

#[test_log::test(tokio::test)]
async fn a_launch_from_another_site_is_refused_keeping_the_form_as_sent() {
    let pages = with_benchmarks().await;

    let (cross_site, cross_site_answer) = post(
        &pages,
        "/benchmarks/board/launch",
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
            count: "2 configurations × 2 tasks × 2 repetitions: 8 trials".into(),
            refusal: Some("a launch is taken only from the web UI's own pages".into()),
        }
    );
    assert!(!pages.path("benchmark-runs/1").exists());
}

/// `docs/specs/benchmarks.md`, New benchmark run;
/// `docs/specs/design_system.md`, Making.
#[test_log::test(tokio::test)]
async fn launching_answers_the_form_as_it_starts_closing_the_dialog_and_the_benchmark_run_shows() {
    let pages = with_benchmarks().await;
    let design = format!("{DESIGN}\nCut the work finely.\n");
    let implement = IMPLEMENT.replace('\n', "\r\n");
    let fix_ci = format!("{FIX_CI}\nAs the page started it.\n");
    let mut events = stream(&pages, "/events/benchmarks/board").await;
    events.recv().await.unwrap();

    let (status, headers, answer) = post_answer(
        &pages,
        "/benchmarks/board/launch",
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
            count: "1 configuration × 2 tasks × 1 repetition: 2 trials".into(),
            refusal: None,
        }
    );
    let benchmark_runs = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, region)) = regions(&event)
            .into_iter()
            .find(|(id, _)| id == "benchmark-runs")
        {
            break region;
        }
    };
    assert_eq!(
        hrefs(select(&benchmark_runs, "tbody tr > th a")),
        ["/benchmark-runs/1"]
    );
    let kept = pages.path("benchmark-runs/1/.kept/configurations");
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
    let html = pages.page("/benchmark-runs/1").await;
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
