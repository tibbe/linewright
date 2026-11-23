//! The tools that show the work (`docs/specs/access.md`, The MCP server,
//! What a tool shows): each returns what its screen shows.

use super::*;

/// `docs/specs/access.md`, The MCP server, What a tool shows;
/// `docs/specs/board.md`, Board.
#[test_log::test(tokio::test)]
async fn get_board_returns_the_board_its_screen_shows() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let board = mcp.shown("get_board", json!({})).await;

    assert_eq!(
        board,
        json!({
            "needing_a_person": 0,
            "people": ["maintainer"],
            "lanes": [{
                "number": 1,
                "title": "Do the thing",
                "cards": [{
                    "number": 1,
                    "title": "Do the thing",
                    "pull_request": false,
                    "column": "ready",
                    "line": "2026-09-30 09:00:00 UTC",
                }],
            }],
        })
    );
}

/// `docs/specs/board.md`, Filtering by person: the board narrowed to the
/// cards a person matches, and the people to choose from, whoever is
/// chosen among them.
#[test_log::test(tokio::test)]
async fn get_board_narrows_the_board_to_a_person() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let narrowed = mcp.shown("get_board", json!({ "person": "bob" })).await;

    assert_eq!(narrowed["lanes"], json!([]));
    assert_eq!(narrowed["person"], "bob");
    assert_eq!(narrowed["people"], json!(["bob", "maintainer"]));
}

/// `docs/specs/board.md`, Item: its heading, its facts and its runs;
/// `docs/specs/ui_patterns.md`, Screen states, Nothing there.
#[test_log::test(tokio::test)]
async fn get_item_returns_the_item_its_screen_shows() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let item = mcp.shown("get_item", json!({ "number": 1 })).await;

    assert_eq!(
        item,
        json!({
            "number": 1,
            "title": "Do the thing",
            "pull_request": false,
            "last_read": "2026-10-01T12:00:00Z",
            "state": "ready-for-agent",
            "standing": "card",
            "column": "ready",
            "line": "2026-09-30 09:00:00 UTC",
            "waiting_for": [],
            "runs": [],
            "left_out": 0,
        })
    );
    assert_eq!(
        refusal(&mcp.call("get_item", json!({ "number": 9 })).await),
        "No such item #9"
    );
}

/// `docs/specs/runs.md`, Runs: the runs newest first, narrowed by the
/// screen's filters; `docs/specs/access.md`, The MCP server, Lists.
#[test_log::test(tokio::test)]
async fn list_runs_lists_the_runs_newest_first_narrowed_and_100_or_limit_at_a_time() {
    let pages = super::super::runs::two_issues_triaged().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let first = mcp.shown("list_runs", json!({ "limit": 1 })).await;
    let next = mcp
        .shown("list_runs", json!({ "limit": 1, "after": first["after"] }))
        .await;
    let narrowed = mcp
        .shown(
            "list_runs",
            json!({ "item": 1, "stage": "triage", "since": "2026-10-01" }),
        )
        .await;
    let past_one_left_out = mcp
        .shown("list_runs", json!({ "item": 1, "after": "2/triage-1" }))
        .await;

    assert_eq!(
        first,
        json!({
            "runs": [{
                "id": "2/triage-1",
                "item": 2,
                "stage": "triage",
                "started_at": "2026-10-01T12:05:00Z",
                "duration_seconds": 150,
                "outcome": "done",
            }],
            "left_out": 1,
            "after": "2/triage-1",
        })
    );
    assert_eq!(next["runs"][0]["id"], "1/triage-1");
    assert_eq!(
        (&next["left_out"], &next["after"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(narrowed["runs"].as_array().unwrap().len(), 1);
    assert_eq!(narrowed["runs"][0]["id"], "1/triage-1");
    assert_eq!(past_one_left_out, narrowed);
    assert_eq!(
        refusal(&mcp.call("list_runs", json!({ "since": "lately" })).await),
        r#"since: "lately" is not a time (2026-10-14T12:00:00Z), a date (2026-10-14) or a duration (2h)"#
    );
}

/// `docs/specs/automations.md`, Automations: each automation file that does
/// not parse, then each automation by name, with its trigger in words, its
/// last run or when it was first seen, and its next run.
#[test_log::test(tokio::test)]
async fn list_automations_lists_the_unparsed_files_then_each_automation() {
    use super::super::automations::{WEEKLY, with};
    use linewright_core::automation::LastScheduledRuns;
    let pages = Pages::of(linewright_core::status::View {
        last_scheduled_runs: ["hourly.md", "team/weekly.md"]
            .map(|name| (name.parse().unwrap(), at("2026-10-01T12:00:00Z")))
            .into_iter()
            .collect::<LastScheduledRuns>(),
        ..with(&[("team/weekly.md", WEEKLY), ("hourly.md", HOURLY)], true)
    })
    .await;
    pages
        .record(&json!({
            "automation": "team/weekly.md",
            "n": 1,
            "started_by": "hand",
            "started_at": "2026-10-01T11:00:00Z",
            "end": {"at": "2026-10-01T11:01:00Z", "ending": "failed"},
            "sessions": [],
        }))
        .await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let first = mcp.shown("list_automations", json!({ "limit": 1 })).await;
    let next = mcp
        .shown(
            "list_automations",
            json!({ "limit": 1, "after": first["after"] }),
        )
        .await;

    assert_eq!(
        first,
        json!({
            "unparsed_automations": [{
                "path": ".linewright/automations/broken.md",
                "reason": "`on.schedule` is \"every hour\", not a cron expression",
            }],
            "automations": [{
                "name": "hourly.md",
                "when": "At minute 0 past every hour.",
                "schedule": "scheduled",
                "next_run": "2026-10-01T13:00:00Z",
                "first_seen": "2026-10-01T12:00:00Z",
            }],
            "left_out": 1,
            "after": "hourly.md",
        })
    );
    assert_eq!(
        next["automations"],
        json!([{
            "name": "team/weekly.md",
            "when": "At 06:00, on Monday.",
            "schedule": "scheduled",
            "next_run": "2026-10-05T06:00:00Z",
            "last_run": "2026-10-01T11:00:00Z",
            "last_run_id": "team/weekly.md/1",
            "last_run_started_by": "hand",
            "last_run_outcome": "failed",
        }])
    );
}

/// `docs/specs/access.md`, The MCP server: a tool that shows something
/// before the first poll says why it shows nothing yet, as its screen does.
#[test_log::test(tokio::test)]
async fn list_automations_before_the_first_poll_is_refused_as_its_screen_is() {
    let pages = Pages::of(a_ticket()).await;
    pages.served.send_replace(None);
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;

    let refused = refusal(
        &Mcp::new(&pages, &token)
            .call("list_automations", json!({}))
            .await,
    );

    assert_eq!(
        refused,
        "`serve` has not polled yet: ask again after its first poll"
    );
}

/// `docs/specs/operation.md`, Server: the facts about Linewright that
/// `status` shows, when it shows them, and the environment's builds.
#[test_log::test(tokio::test)]
async fn get_server_returns_the_facts_about_linewright_its_screen_shows() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;

    let server = Mcp::new(&pages, &token)
        .shown("get_server", json!({}))
        .await;

    assert_eq!(server["builds"], json!([]));
    assert_eq!(server["linewright"]["runs_in_flight"], 0);
    assert_eq!(
        server["linewright"]["polls"],
        json!({ "last": "2026-10-01T12:00:00Z", "next": "2026-10-01T12:01:00Z" })
    );
    assert_eq!(server["linewright"]["drain"], Value::Null);
}

/// `docs/specs/runs.md`, Run: its heading and its sessions' settings;
/// `docs/specs/access.md`, The MCP server, Transcripts: the latest 100
/// entries, or the latest `limit`, with how many were left out and the
/// `before` that gives the ones before them, in full with `verbose`.
#[test_log::test(tokio::test)]
async fn get_run_returns_the_run_and_its_latest_entries_and_the_earlier_ones_before_them() {
    let pages = super::super::run::implemented_as_recorded().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let all = mcp
        .shown("get_run", json!({ "run_id": "1/implement-1" }))
        .await;
    let latest = mcp
        .shown("get_run", json!({ "run_id": "1/implement-1", "limit": 2 }))
        .await;
    let before = mcp
        .shown(
            "get_run",
            json!({ "run_id": "1/implement-1", "limit": 2, "before": latest["before"] }),
        )
        .await;
    let verbose = mcp
        .shown(
            "get_run",
            json!({ "run_id": "1/implement-1", "verbose": true }),
        )
        .await;

    assert_eq!(all["id"], "1/implement-1");
    assert_eq!(all["item"], 1);
    assert_eq!(all["stage"], "implement");
    assert_eq!(all["outcome"], "done");
    assert_eq!(all["duration_seconds"], 240);
    assert_eq!(all["sessions"][0]["session"], 1);
    assert_eq!(all["sessions"][0]["model"], "claude-opus-5-5");
    assert_eq!(all["sessions"][0]["effort"], "medium");
    let entries = all["entries"].as_array().unwrap();
    let n = entries.len();
    assert!(n > 4, "{all}");
    assert_eq!(
        (&all["left_out"], &all["before"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(latest["entries"].as_array().unwrap(), &entries[n - 2..]);
    assert_eq!(latest["left_out"], n - 2);
    assert_eq!(
        before["entries"].as_array().unwrap(),
        &entries[n - 4..n - 2]
    );
    assert_eq!(before["left_out"], n - 4);
    let has_input = |shown: &Value| {
        shown["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry.get("input").is_some())
    };
    assert!(!has_input(&all), "{all}");
    assert!(has_input(&verbose), "{verbose}");
    let brief = |shown: &Value| {
        let mut brief = shown["entries"][0].clone();
        let brief = brief.as_object_mut().unwrap();
        ["type", "summary", "text"].map(|key| brief.remove(key).unwrap_or_default())
    };
    assert_eq!(
        brief(&all),
        [json!("brief"), json!("# Your task …"), Value::Null]
    );
    assert_eq!(
        brief(&verbose),
        [
            json!("brief"),
            json!("# Your task …"),
            json!("# Your task\n\nImplement the ticket below.\n")
        ]
    );
    assert_eq!(
        refusal(
            &mcp.call("get_run", json!({ "run_id": "9/implement-1" }))
                .await
        ),
        "No such run 9/implement-1"
    );
}

/// What the local API answers at `path`, as a monitoring command's `--json`
/// prints it.
#[track_caller]
fn answered(pages: &Pages, path: &str) -> impl Future<Output = Value> + use<> {
    let caller = Location::caller();
    let router = linewright_core::api::router(pages.sources.clone(), CancellationToken::new());
    let path = format!("/api{path}");
    async move {
        let (status, body) =
            status_and_body(routed(&router, hyper::Method::GET, Some("localhost"), &path).await)
                .await;
        assert_eq!(status, 200, "{path} at {caller}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("{path} at {caller}: {e}: {body}"))
    }
}

/// `docs/specs/access.md`, The MCP server, What a tool shows: a fact that
/// `status`, `runs`, `explain` and `logs` also give in `--json` has the
/// name it has there, here with the same value on the same world.
#[test_log::test(tokio::test)]
async fn the_facts_a_monitoring_commands_json_also_gives_have_its_names() {
    let pages = super::super::run::implemented_as_recorded().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let server = mcp.shown("get_server", json!({})).await;
    let item = mcp.shown("get_item", json!({ "number": 1 })).await;
    let runs = mcp.shown("list_runs", json!({ "item": 1 })).await;
    let run = mcp
        .shown("get_run", json!({ "run_id": "1/implement-1" }))
        .await;
    let verbose = mcp
        .shown(
            "get_run",
            json!({ "run_id": "1/implement-1", "verbose": true }),
        )
        .await;
    let status = answered(&pages, "/status").await;
    let explained = answered(&pages, "/items/1").await;
    let listed = answered(&pages, "/runs?item=1").await;
    let transcript: linewright_core::transcript::Transcript =
        serde_json::from_value(answered(&pages, "/logs?run=1/implement-1").await).unwrap();

    assert_eq!(server["linewright"], status["linewright"]);
    assert_eq!(
        shared(&item, &explained),
        ["last_read", "number", "state", "title", "waiting_for"]
    );
    assert_eq!(runs["runs"], listed["runs"]);
    let listed_run = &listed["runs"][0];
    let mut heading = run.clone();
    heading.as_object_mut().unwrap().remove("sessions");
    assert_eq!(
        shared(&heading, listed_run),
        [
            "duration_seconds",
            "id",
            "item",
            "outcome",
            "stage",
            "started_at"
        ]
    );
    for (session, listed_session) in run["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .zip(listed_run["sessions"].as_array().unwrap())
    {
        assert_eq!(
            shared(session, listed_session),
            ["effort", "layers", "model"]
        );
    }
    for (shown, verbosity) in [
        (&run, linewright_core::transcript::lines::Verbosity::Compact),
        (
            &verbose,
            linewright_core::transcript::lines::Verbosity::Full,
        ),
    ] {
        let logged: Vec<Value> =
            linewright_core::transcript::lines::json_lines(&transcript, verbosity)
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        let headers = |kind: &str| -> Vec<Value> {
            logged
                .iter()
                .filter(|line| line["type"] == kind)
                .cloned()
                .collect()
        };
        assert_eq!(
            headers("session")
                .iter()
                .map(|session| (session["session"].clone(), session["started_at"].clone()))
                .collect::<Vec<_>>(),
            shown["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|session| (session["session"].clone(), session["started_at"].clone()))
                .collect::<Vec<_>>()
        );
        let of_type = |lines: &[Value], kind: &str| -> Vec<Value> {
            lines
                .iter()
                .filter(|line| line["type"] == kind)
                .map(|line| {
                    let mut line = line.clone();
                    line.as_object_mut().unwrap().remove("id");
                    line
                })
                .collect()
        };
        let shown = shown["entries"].as_array().unwrap();
        for kind in ["message", "tool", "clone", "maintenance", "follow-up"] {
            assert_eq!(
                of_type(shown, kind),
                of_type(&logged, kind),
                "{kind}, {verbosity:?}"
            );
        }
        let texts = |lines: Vec<Value>| -> Vec<Value> {
            lines
                .into_iter()
                .map(|line| line["text"].clone())
                .filter(|text| !text.is_null())
                .collect()
        };
        assert_eq!(
            texts(of_type(shown, "thought")),
            texts(of_type(&logged, "thinking")),
            "{verbosity:?}"
        );
        if verbosity == linewright_core::transcript::lines::Verbosity::Full {
            assert_eq!(
                texts(of_type(shown, "brief")),
                texts(of_type(&logged, "brief"))
            );
        }
        assert_ne!(of_type(shown, "tool"), [] as [Value; 0]);
    }
}

/// `docs/specs/automations.md`, Automations, and `lw status`: an
/// automation's facts by the names `status --json` gives them.
#[test_log::test(tokio::test)]
async fn an_automations_facts_have_the_names_status_gives_them() {
    use super::super::automations::{WEEKLY, with};
    let pages = Pages::of(with(&[("team/weekly.md", WEEKLY)], true)).await;
    pages
        .record(&json!({
            "automation": "team/weekly.md",
            "n": 1,
            "started_by": "hand",
            "started_at": "2026-10-01T11:00:00Z",
            "end": {"at": "2026-10-01T11:01:00Z", "ending": "done"},
            "sessions": [],
        }))
        .await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;

    let automations = Mcp::new(&pages, &token)
        .shown("list_automations", json!({}))
        .await;
    let status = answered(&pages, "/status").await;

    assert_eq!(
        automations["unparsed_automations"],
        status["unparsed_automations"]
    );
    assert_eq!(
        shared(&automations["automations"][0], &status["automations"][0]),
        [
            "last_run",
            "last_run_id",
            "last_run_started_by",
            "name",
            "schedule",
            "when"
        ]
    );
}

/// `docs/specs/board.md`, Item, and `lw explain`: an item Linewright
/// leaves, by the names `explain --json` gives its facts, and what keeps it
/// out as Item words it.
#[test_log::test(tokio::test)]
async fn get_item_says_what_keeps_an_item_linewright_leaves_out() {
    let pages = Pages::of(view(vec![], vec![])).await;
    let unlabeled = pages
        .store
        .open_issue("maintainer", "Old idea", "Maybe.", &[]);
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;

    let item = Mcp::new(&pages, &token)
        .shown("get_item", json!({ "number": unlabeled.0 }))
        .await;
    let mut explained = answered(&pages, &format!("/items/{}", unlabeled.0)).await;

    assert_eq!(item["standing"], "left-alone");
    assert_eq!(
        item["kept_out_by"],
        "labeled none of `needs-triage`, `needs-info`, `ready-for-agent` or `ready-for-human`"
    );
    let read_again = explained
        .as_object_mut()
        .unwrap()
        .remove("last_read")
        .unwrap();
    assert!(item["last_read"].is_string() && read_again.is_string());
    assert_eq!(shared(&item, &explained), ["number", "state", "title"]);
}
