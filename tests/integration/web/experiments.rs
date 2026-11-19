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
    assert_eq!(replays(configurations[1], 1)[1], "repetition 2 pending");
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

#[test_log::test(tokio::test)]
async fn the_suite_page_offers_to_launch_an_experiment_as_is_first_and_counts_its_replays() {
    let pages = with_suites().await;

    let html = pages.page("/suites/board").await;

    let form = select(&html, "form.launch");
    assert_eq!(form.len(), 1);
    assert_eq!(form[0].attr("hx-post"), Some("/suites/board/launch"));
    assert_eq!(
        select(&html, "#new-experiment-offer")[0].attr("data-cases"),
        Some("2")
    );
    assert_eq!(
        select_in(form[0], "input[name=repetitions]")[0].attr("value"),
        Some("1")
    );
    let configurations: Vec<String> = select_in(form[0], "fieldset.configuration legend")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(configurations, ["as-is"]);
    assert_eq!(
        text(select_in(form[0], ".replay-count")[0]),
        "2 replays: 2 cases, 1 configuration, 1 repetition"
    );
    let template = select(&html, "template#configuration-template");
    let fields: Vec<&str> = select_in(template[0], "input, select")
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
    let name = select_in(template[0], "input[name='c__N__.name']");
    assert_eq!(name[0].attr("pattern"), Some(NAME_PATTERN));
    let texts: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&select(&html, "script#brief-texts")[0].inner_html()).unwrap();
    assert_eq!(texts["prompts/implement.md"], IMPLEMENT);
    assert_eq!(
        texts["plugin/skills/tdd/SKILL.md"],
        include_str!("../../../briefs/plugin/skills/tdd/SKILL.md")
    );
}

#[test_log::test(tokio::test)]
async fn a_suite_with_no_cases_offers_no_launch_and_a_launch_on_it_is_refused() {
    let pages = with_suites().await;

    let html = pages.page("/suites/triage-basics").await;
    let (status, answer) = post(
        &pages,
        "/suites/triage-basics/launch",
        &[SAME_ORIGIN],
        &[("repetitions", "1")],
    )
    .await;

    assert_eq!(disclosures(&html, &["launch"]), []);
    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not launched: the suite triage-basics has no cases"
    );
    assert!(!pages.path("experiments/1").exists());
}

#[test_log::test(tokio::test)]
async fn a_launch_with_a_configuration_with_an_error_or_a_name_taken_is_refused() {
    let pages = with_suites().await;
    let refusals: [(&[(&str, &str)], &str); 5] = [
        (
            &[
                ("repetitions", "1"),
                ("c1.name", "short"),
                ("c1.effort", "huge"),
            ],
            "Not launched: the configuration short cannot be used: config.toml: ",
        ),
        (
            &[
                ("repetitions", "1"),
                ("c1.name", "short"),
                ("c1.briefs/prompts/implement.md", "Implement {{nothing}}.\n"),
            ],
            "Not launched: the configuration short cannot be used: briefs/prompts/implement.md: ",
        ),
        (
            &[("repetitions", "1"), ("c1.name", "as-is")],
            "Not launched: two configurations are named as-is",
        ),
        (
            &[
                ("repetitions", "1"),
                ("c1.name", "short"),
                ("c2.name", "short"),
            ],
            "Not launched: two configurations are named short",
        ),
        (
            &[("repetitions", "1"), ("c1.name", "Short")],
            "Not launched: \"Short\" is not a configuration's name",
        ),
    ];

    for (fields, refused) in refusals {
        let (status, answer) = post(&pages, "/suites/board/launch", &[SAME_ORIGIN], fields).await;
        assert_eq!(status, 422, "{fields:?}");
        let answer = said(&answer);
        assert!(answer.starts_with(refused), "{answer}");
    }
    let (no_repetitions, answer) = post(
        &pages,
        "/suites/board/launch",
        &[SAME_ORIGIN],
        &[("repetitions", "0")],
    )
    .await;
    assert_eq!(no_repetitions, 422);
    assert_eq!(
        said(&answer),
        "Not launched: the repetitions are a whole number, 1 or more"
    );
    let (other_site, _) = post(
        &pages,
        "/suites/board/launch",
        &[CROSS_SITE],
        &[("repetitions", "1")],
    )
    .await;
    assert_eq!(other_site, 403);
    assert!(!pages.path("experiments/1").exists());
}

#[test_log::test(tokio::test)]
async fn launching_keeps_each_configuration_with_its_settings_and_only_the_files_changed() {
    let pages = with_suites().await;
    let design = format!("{DESIGN}\nCut the work finely.\n");
    let implement = IMPLEMENT.replace('\n', "\r\n");
    let fix_ci = format!("{FIX_CI}\nAs the page started it.\n");

    let (status, answer) = post(
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

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(
        said(&answer),
        "Launched experiment 1 on the suite board: 12 replays"
    );
    assert_eq!(hrefs(select(&answer, "a")), ["/experiments/1"]);
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
