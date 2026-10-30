use super::*;

use linewright::transcript::{SetupStatus, setup_ended, setup_started};
use serde_json::json;

/// Where #1's implementing session keeps its record.
const SESSION: &str = "sessions/issue-1/20261013T100908Z-implement";

/// How a session that ran with the harness's model and effort, and the
/// operator's implementing prompt, is kept in its run's record, its own
/// record in `dir`.
fn ran(dir: &str) -> serde_json::Value {
    json!({
        "dir": dir,
        "model": null,
        "effort": null,
        "layers": {
            "settings": {
                "model": "harness",
                "effort": "harness",
                "session-timeout": "operator",
                "setup": "linewright",
                "session-env": "linewright",
                "session-pass-env": "linewright",
            },
            "prompts": {
                "follow-ups/result-error.md": "linewright",
                "implement.md": "operator",
            },
        },
    })
}

/// The output of a session whose setup command `command` started at
/// `start` and ended `seconds` later as `status`.
fn setting_up(start: &str, command: &str, status: SetupStatus, seconds: f64) -> String {
    let start = at(start);
    let end =
        start + chrono::TimeDelta::from_std(std::time::Duration::from_secs_f64(seconds)).unwrap();
    format!(
        "{}\n{}\n",
        setup_started(start, command),
        setup_ended(end, status, seconds)
    )
}

/// #1, `Ship gh`, implemented in run 1/implement-1 of one session, whose
/// transcript is the one recorded, with a sub-agent's work and a call that
/// failed.
pub(super) async fn implemented_as_recorded() -> Pages {
    implemented_after("", None).await
}

/// [`implemented_as_recorded`]'s, its session's output after `setup`, and
/// its setup command's output `setup_log`.
async fn implemented_after(setup: &str, setup_log: Option<&str>) -> Pages {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let record = json!({
        "item": 1,
        "stage": "implement",
        "n": 1,
        "started_at": "2026-10-13T10:09:00Z",
        "end": {"at": "2026-10-13T10:13:00Z", "ending": "done"},
        "sessions": [ran(SESSION)],
    });
    let output = [
        setup,
        include_str!("../fixtures/transcript/turn-1.jsonl"),
        include_str!("../fixtures/transcript/turn-2.jsonl"),
    ]
    .concat();
    let mut files = vec![
        (format!("{SESSION}/output.jsonl"), output),
        (
            format!("{SESSION}/brief.md"),
            "# Your task\n\nImplement the ticket below.\n".to_string(),
        ),
        (
            format!("{SESSION}/followup-1.md"),
            "Your final message could not be used: The final message holds no JSON object. \
             End with one.\n"
                .to_string(),
        ),
    ];
    if let Some(log) = setup_log {
        files.push((format!("{SESSION}/setup.log"), log.to_string()));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    pages.write(&files);
    pages.record(&record);
    pages
}

/// The run page's headers and entries in order, each entry as its line,
/// and with `io` its fold's input and result too, as a reader reads them.
fn transcript_shown(html: &Html, io: bool) -> Vec<String> {
    let parts = Selector::parse(if io { ".line, .io" } else { ".line" }).unwrap();
    select(html, "#transcript h1, #transcript h2, #transcript .entry")
        .into_iter()
        .map(|shown| {
            if !shown.value().classes().any(|class| class == "entry") {
                return text(shown);
            }
            let own: Vec<String> = shown
                .select(&parts)
                .filter(|part| entry_of(*part) == Some(shown))
                .map(text)
                .collect();
            own.join(" ")
        })
        .collect()
}

/// The entry `element` is part of.
fn entry_of(element: ElementRef) -> Option<ElementRef> {
    element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|ancestor| ancestor.value().classes().any(|class| class == "entry"))
}

/// Each of `shown`, cut to its first `chars` characters.
fn cut(shown: &[String], chars: usize) -> Vec<String> {
    shown
        .iter()
        .map(|line| line.chars().take(chars).collect())
        .collect()
}

#[tokio::test]
async fn the_run_page_shows_above_each_session_its_model_effort_and_where_its_settings_and_prompts_came_from()
 {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;

    let texts =
        |selector: &str| -> Vec<String> { select(&html, selector).into_iter().map(text).collect() };
    assert_eq!(
        texts("#s1 .ran"),
        ["model the harness's choice, effort the harness's choice"]
    );
    assert_eq!(
        texts("#s1 .layers .setting"),
        [
            "model: the harness's",
            "effort: the harness's",
            "session-timeout: the operator's",
            "setup: Linewright's own",
            "session-env: Linewright's own",
            "session-pass-env: Linewright's own",
        ]
    );
    assert_eq!(
        texts("#s1 .layers .prompt"),
        [
            "follow-ups/result-error.md: Linewright's own",
            "implement.md: the operator's",
        ]
    );
}

/// `docs/specs/web/design_system.md`, Colour scheme: secondary text is in
/// Muted text.
#[tokio::test]
async fn the_run_page_shows_a_sessions_ran_with_line_its_layers_and_each_entrys_time_in_muted_text()
{
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;

    for selector in ["#s1 .ran", "#s1 .layers", "#transcript a.at"] {
        assert_in_muted_text(&html, selector);
    }
}

#[tokio::test]
async fn the_run_page_shows_what_logs_shows_with_each_tool_call_folded_to_its_line() {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;

    // As `lw logs 1` prints them.
    assert_eq!(
        cut(&transcript_shown(&html, false), 100),
        [
            "1/implement-1",
            "session 1: started 2026-10-13 10:09:08 UTC",
            "0:03 Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit ",
            "0:14 Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → exit 2",
            "0:15 Skill(linewright:tdd) → loaded",
            "0:18 Can't build images here (no Docker), and the repo has no test seam for the Dockerfile, so I'll ",
            "0:27 Read(Dockerfile) → 16 lines",
            "0:35 Edit(Dockerfile) → edited",
            "2:00 Agent(Standards review) → done, 2 tool uses, 16s",
            "2:02 Bash(git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…) → exit 0",
            "2:45 Both reviews came back clean. One optional fix: the README and the image label use the serial c",
            "2:59 {\"outcome\":\"done\",\"pr_title\":\"Ship gh and python3 in the base image\",\"pr_body\":\"## Summary\\n\\nT",
            "3:22 follow-up: Your final message could not be used: The final message holds no JSON object. End wi",
            "3:25 Bash(git commit -qam \"Ship gh and python3 in the base image…) → exit 0, 1 line",
            "3:28 {\"outcome\":\"done\",\"pr_title\":\"Ship gh and python3 in the base image\",\"pr_body\":\"## Summary\\n\\nT",
        ]
    );
    let calls = select(&html, "#transcript .entry > details.call");
    assert_eq!(calls.len(), 8);
    for call in &calls {
        let line = text(select_in(*call, "summary .line")[0]);
        let failed = line.ends_with("→ exit 2");
        assert_eq!(call.attr("open").is_some(), failed, "{line}");
        let io: Vec<String> = select_in(*call, ".io > *")
            .into_iter()
            .map(|part| text(part))
            .collect();
        assert!(
            io.first()
                .is_some_and(|input| input.starts_with("input: {")),
            "{line}: {io:?}"
        );
        assert!(
            io.get(1)
                .is_some_and(|result| result.starts_with("result: ")),
            "{line}: {io:?}"
        );
    }
    let counted = select(&html, "#transcript details.call > summary > .count");
    assert_eq!(counted.len(), 1);
    let agent_call = counted[0]
        .ancestors()
        .find_map(ElementRef::wrap)
        .and_then(|summary| ElementRef::wrap(summary.parent().unwrap()))
        .unwrap();
    assert_eq!(agent_call.attr("open"), None);
    assert_eq!(
        text(select_in(agent_call, "summary")[0]),
        "2:00 Agent(Standards review) → done, 2 tool uses, 16s 1 entry"
    );
    // Its prompt is context, which `logs` hides, and one tool call.
    let work: Vec<String> = entry_lines_in(agent_call)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    assert_eq!(cut(&work, 30), ["2:02 Bash(git diff origin/main"]);
    assert_no_status_by_colour_alone(&html);
}

#[tokio::test]
async fn the_run_pages_entry_stamp_shows_when_the_entry_happened_in_the_pages_tooltip() {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;

    let stamp = select(&html, "#transcript a.at")
        .into_iter()
        .find(|stamp| text(*stamp) == "0:03")
        .unwrap();
    // The session started at 10:09:08, and the entry 3 seconds after.
    assert_eq!(stamp.attr("data-tip"), Some("2026-10-13 10:09:11 UTC"));
    assert_eq!(select(&html, "[title]").len(), 0);
}

#[tokio::test]
async fn the_run_pages_verbose_switch_shows_what_logs_verbose_shows_with_everything_open() {
    let pages = implemented_as_recorded().await;
    let html = pages.page("/runs/1/implement-1").await;
    let switch = select(&html, "[role=switch]");
    assert_eq!(switch[0].attr("aria-checked"), Some("false"));
    let verbose_path = switch[0].attr("href").unwrap().to_string();
    assert_eq!(verbose_path, "/runs/1/implement-1?verbose=");

    let verbose = pages.page(&verbose_path).await;

    // As `lw logs 1 --verbose` prints them.
    assert_eq!(
        cut(&transcript_shown(&verbose, true), 40),
        [
            "1/implement-1",
            "session 1: started 2026-10-13 10:09:08 U",
            "0:00 brief: # Your task Implement the ti",
            "0:00 event: {\"analytics_disabled\":false,",
            "0:03 Bash(ls; cat Dockerfile; ls deploy;",
            "0:04 thinking: The listing has a Dockerf",
            "0:14 Bash(which podman buildah nerdctl h",
            "0:15 Skill(linewright:tdd) → loaded inpu",
            "0:15 context: Base directory for this sk",
            "0:18 Can't build images here (no Docker)",
            "0:27 Read(Dockerfile) → 16 lines input: ",
            "0:35 Edit(Dockerfile) → edited input: { ",
            "2:00 Agent(Standards review) → done, 2 t",
            "2:00 context: Review a diff in /srv/line",
            "2:02 Bash(git diff origin/main...HEAD --",
            "2:00 event: {\"description\":\"Standards re",
            "2:02 event: {\"output_file\":\"/srv/linewri",
            "2:45 Both reviews came back clean. One o",
            "2:59 {\"outcome\":\"done\",\"pr_title\":\"Ship ",
            "2:59 event: {\"api_error_status\":null,\"du",
            "3:22 follow-up: Your final message could",
            "3:22 event: {\"analytics_disabled\":false,",
            "3:25 Bash(git commit -qam \"Ship gh and p",
            "3:28 {\"outcome\":\"done\",\"pr_title\":\"Ship ",
            "3:28 event: {\"api_error_status\":null,\"du",
        ]
    );
    let shown = transcript_shown(&verbose, true);
    assert!(
        shown[4].contains(
            r#"→ exit 0, 19 lines input: { "command": "ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"image\\|git\\b\" README.md | head -50; cat CODING_STANDARDS.md | head -80" } result: AGENTS.md CODING_STANDARDS.md"#
        ),
        "{}",
        shown[4]
    );
    assert_eq!(
        shown[5],
        "0:04 thinking: The listing has a Dockerfile. I'll check what the image ships."
    );
    let folds = select(&verbose, "#transcript details");
    assert_eq!(folds.len(), 8);
    assert!(folds.iter().all(|fold| fold.attr("open").is_some()));
    let count = select(&verbose, "#transcript details.call > summary > .count");
    assert_eq!(text(count[0]), "2 entries");
    let back = select(&verbose, "[role=switch]");
    assert_eq!(back[0].attr("aria-checked"), Some("true"));
    assert_eq!(back[0].attr("href"), Some("/runs/1/implement-1"));
}

/// Each event the stream at `path` sends, as its ID, if it has one, and
/// its data, resuming after the event `last_id` if one is given.
async fn stream_with_ids(
    web: &impl Routed,
    path: &str,
    last_id: Option<&str>,
) -> tokio::sync::mpsc::UnboundedReceiver<(Option<String>, String)> {
    let mut request = axum::http::Request::get(path).header("Host", "localhost");
    if let Some(last_id) = last_id {
        request = request.header("Last-Event-ID", last_id);
    }
    let response = web
        .router()
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
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
                let id = block
                    .lines()
                    .find_map(|line| line.strip_prefix("id: "))
                    .map(str::to_string);
                let data: Vec<&str> = block
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .collect();
                if !data.is_empty() && sender.send((id, data.join("\n"))).is_err() {
                    return;
                }
            }
        }
    });
    receiver
}

/// The transcript's events, those with an ID, until none comes for a
/// while.
async fn transcript_events(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<(Option<String>, String)>,
    until: impl Fn(&[(String, String)]) -> bool,
) -> Vec<(String, String)> {
    let mut read = vec![];
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        while !until(&read) {
            let (id, data) = events.recv().await.unwrap();
            if let Some(id) = id {
                read.push((id, data));
            }
        }
    })
    .await;
    assert!(waited.is_ok(), "{read:?}");
    read
}

/// Each change an event makes to the page: where, how, and the markup.
fn patches(event: &str) -> Vec<(String, String, Html)> {
    let html = Html::parse_fragment(event);
    select(&html, "hx-partial")
        .into_iter()
        .map(|partial| {
            (
                partial.attr("hx-target").unwrap().to_string(),
                partial.attr("hx-swap").unwrap().to_string(),
                Html::parse_fragment(&partial.inner_html()),
            )
        })
        .collect()
}

/// Each entry's line, by its anchor.
fn entry_lines(html: &Html) -> Vec<(String, String)> {
    entry_lines_in(html.root_element())
}

fn entry_lines_in(element: ElementRef) -> Vec<(String, String)> {
    select_in(element, ".entry")
        .into_iter()
        .map(|entry| {
            let line = entry
                .select(&Selector::parse(".line").unwrap())
                .find(|line| entry_of(*line) == Some(entry))
                .map(text)
                .unwrap_or_default();
            (entry.attr("id").unwrap().to_string(), line)
        })
        .collect()
}

#[tokio::test]
async fn the_run_page_follows_a_run_in_flight_from_where_it_was_read_to_and_resumes_by_event_id() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    let turn = crate::local_api::turn_1();
    // The marker, the start event and the first Bash call, with its result.
    agent.writes(&turn[..4].concat());
    let read = web
        .eventually_page("/runs/1/triage-1", |html| {
            entry_lines(html)
                .iter()
                .any(|(_, line)| line.contains("Bash(ls;"))
        })
        .await;
    let events_path = events_path(&read).unwrap().to_string();
    assert!(
        events_path.starts_with("/events/runs/1/triage-1?follow=1%2Ftriage-1%3A1."),
        "{events_path}"
    );
    let mut events = stream_with_ids(&web, &events_path, None).await;

    // The second Bash call, its result still to come.
    agent.writes(&turn[4..6].concat());
    let made = transcript_events(&mut events, |sent| !sent.is_empty()).await;
    agent.writes(&turn[6]);
    let returned = transcript_events(&mut events, |sent| !sent.is_empty()).await;
    agent.writes(&turn[7..].concat());
    drop(agent);
    web.eventually_page("/runs?item=1", |html| {
        has_run(&runs_shown(html), "done", "1/triage-1")
    })
    .await;
    let rest = transcript_events(&mut events, |sent| {
        sent.iter().any(|(_, data)| {
            data.contains("{&quot;outcome&quot;") || data.contains(r#"{"outcome""#)
        })
    })
    .await;

    let [(_, made_data)] = &made[..] else {
        panic!("one event for the call made: {made:?}")
    };
    let [(added_to, added_by, call)] = &patches(made_data)[..] else {
        panic!("{made_data}")
    };
    assert_eq!(
        (added_to.as_str(), added_by.as_str()),
        ("#s1-entries", "beforeend")
    );
    let call_id = entry_lines(call)[0].0.clone();
    assert!(
        entry_lines(call)[0].1.ends_with("2>&1) → running"),
        "{:?}",
        entry_lines(call)
    );
    let [(_, returned_data)] = &returned[..] else {
        panic!("one event for the call returned: {returned:?}")
    };
    let [(replaced_at, replaced_by, account)] = &patches(returned_data)[..] else {
        panic!("{returned_data}")
    };
    assert_eq!(
        (replaced_at.as_str(), replaced_by.as_str()),
        (format!("#{call_id}-call").as_str(), "outerHTML")
    );
    let replaced = select(account, "details.call");
    assert!(replaced[0].attr("open").is_some());
    assert!(text(select_in(replaced[0], "summary")[0]).ends_with("2>&1) → exit 2"));
    // The account of a call that did not fail keeps its fold as the reader
    // left it.
    let accounts: Vec<Vec<(String, String)>> = rest
        .iter()
        .map(|(_, data)| {
            patches(data)
                .into_iter()
                .map(|(at, by, _)| (at, by))
                .collect()
        })
        .filter(|patches: &Vec<(String, String)>| patches.iter().any(|(_, by)| by == "outerHTML"))
        .collect();
    let replaced_line = |entry: &str| {
        vec![
            (format!("#{entry}-line"), "outerHTML".to_string()),
            (format!("#{entry}-io"), "outerHTML".to_string()),
        ]
    };
    assert!(accounts.contains(&replaced_line("s1-e6")), "{accounts:?}");

    // Followed in pieces, the page is as it reads whole.
    let followed: Vec<(String, String)> = [made, returned, rest.clone()].concat();
    let mut shown = entry_lines(&read);
    for (_, data) in &followed {
        for (target, swap, html) in patches(data) {
            if swap == "beforeend" {
                for (id, line) in entry_lines(&html) {
                    assert!(
                        shown.iter().all(|(shown, _)| *shown != id),
                        "{id} sent twice"
                    );
                    shown.push((id, line));
                }
            } else if let Some(entry) = target.strip_prefix('#').and_then(|id| {
                id.strip_suffix("-call")
                    .or_else(|| id.strip_suffix("-line"))
            }) {
                let line = text(select(&html, ".line")[0]);
                shown.iter_mut().find(|(id, _)| id == entry).unwrap().1 = line;
            }
        }
    }
    let whole = web.page("/runs/1/triage-1").await;
    let mut shown_sorted = shown.clone();
    shown_sorted.sort();
    let mut whole_sorted = entry_lines(&whole);
    whole_sorted.sort();
    assert_eq!(shown_sorted, whole_sorted);

    // Resumed after its first event, the stream sends the rest, once.
    let first = followed[0].0.clone();
    let mut resumed = stream_with_ids(&web, &events_path, Some(&first)).await;
    let again = transcript_events(&mut resumed, |sent| sent.len() >= followed.len() - 1).await;
    assert_eq!(again, followed[1..]);
    let more = tokio::time::timeout(Duration::from_millis(1500), async {
        loop {
            if let (Some(id), _) = resumed.recv().await.unwrap() {
                return id;
            }
        }
    })
    .await;
    assert!(more.is_err(), "sent more: {more:?}");
    web.stop().await;
}

fn heading(html: &Html) -> Vec<String> {
    select(html, "#transcript h1, #transcript h2")
        .into_iter()
        .map(text)
        .collect()
}

/// The line under the run page's heading that names the run's item, and
/// where its links go.
fn item_line(html: &Html) -> (String, Vec<String>) {
    let line = select(html, "#transcript hgroup#run > p");
    assert_eq!(line.len(), 1, "one item line");
    (text(line[0]), hrefs(select_in(line[0], "a")))
}

/// Where the links in the run page's heading itself go.
fn heading_links(html: &Html) -> Vec<String> {
    hrefs(select(html, "#transcript h1 a"))
}

#[tokio::test]
async fn the_run_page_heads_an_items_run_with_its_whole_id_and_names_its_item_under_it_as_the_runs_page_does()
 {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;
    let runs = pages.page("/runs").await;

    let row = runs_shown(&runs)
        .into_iter()
        .find(|row| row[1] == "1/implement-1")
        .unwrap();
    let cell_links = hrefs(select(&runs, "#runs tbody td:nth-child(3) a"));
    assert_eq!(heading(&html)[0], "1/implement-1");
    assert!(
        heading_links(&html).is_empty(),
        "{:?}",
        heading_links(&html)
    );
    let links = vec![
        "/items/1".to_string(),
        format!("https://github.com/{REPO}/issues/1"),
    ];
    // As the runs page's Item cell names the run's item.
    assert_eq!((row[2].as_str(), &cell_links), ("#1: Ship gh", &links));
    assert_eq!(item_line(&html), ("#1: Ship gh".to_string(), links));
}

#[tokio::test]
async fn the_run_pages_stream_swaps_in_the_whole_heading_when_the_runs_header_comes() {
    let pages = implemented_as_recorded().await;
    let mut events = stream_with_ids(&pages, "/events/runs/1/implement-1?follow=start", None).await;

    let sent = transcript_events(&mut events, |sent| !sent.is_empty()).await;

    let [(target, swap, heading)] = &patches(&sent[0].1)[..] else {
        panic!("one patch for the header: {sent:?}")
    };
    assert_eq!((target.as_str(), swap.as_str()), ("#run", "outerHTML"));
    let swapped: Vec<String> = select(heading, "hgroup#run > h1")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(swapped, ["1/implement-1"]);
    assert_eq!(text(select(heading, "hgroup#run > p")[0]), "#1: Ship gh");
}

#[tokio::test]
async fn an_automations_run_whose_id_holds_a_slash_has_its_page() {
    let pages = Pages::of(view(vec![], vec![])).await;
    let session = "sessions/automation-weekly/20261014T120005Z-automation";
    let record = json!({
        "automation": "weekly",
        "n": 1,
        "started_by": "hand",
        "started_at": "2026-10-14T12:00:00Z",
        "end": {"at": "2026-10-14T12:01:00Z", "ending": "done"},
        "sessions": [ran(session)],
    });
    let output = output_of(&[
        json!({"type": "linewright-turn", "started_at": "2026-10-14T12:00:05Z", "started_by": "brief"}),
        json!({
            "type": "assistant",
            "timestamp": "2026-10-14T12:00:09Z",
            "message": {"content": [{"type": "text", "text": "Nothing needs doing."}]},
        }),
    ]);
    pages.record(&record);
    pages.write(&[(&format!("{session}/output.jsonl"), &output)]);

    let runs = pages.page("/runs").await;
    let html = pages.page("/runs/weekly/1").await;

    let links: Vec<&str> = select(&runs, "#runs tbody td:nth-child(2) a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/runs/weekly/1"]);
    assert_eq!(
        heading(&html),
        [
            "weekly/1, by hand",
            "session 1: started 2026-10-14 12:00:05 UTC"
        ]
    );
    assert!(
        heading_links(&html).is_empty(),
        "{:?}",
        heading_links(&html)
    );
    assert_eq!(
        item_line(&html),
        (
            "automation weekly".to_string(),
            vec!["/automations".to_string()]
        )
    );
}

#[tokio::test]
async fn the_page_of_an_automations_run_by_hand_says_so_in_its_heading_and_of_a_scheduled_one_not()
{
    let pages = Pages::of(view(vec![], vec![])).await;
    for (n, by) in [(1, "schedule"), (2, "hand")] {
        let record = json!({
            "automation": "hourly",
            "n": n,
            "started_by": by,
            "started_at": "2026-10-14T12:00:00Z",
            "end": {"at": "2026-10-14T12:01:00Z", "ending": "done"},
            "sessions": [],
        });
        pages.record(&record);
    }

    let by_hand = pages.page("/runs/hourly/2").await;
    let scheduled = pages.page("/runs/hourly/1").await;

    assert_eq!(
        heading(&by_hand)[0],
        "hourly/2, by hand (it started no session)"
    );
    assert_eq!(heading(&scheduled)[0], "hourly/1 (it started no session)");
}

#[tokio::test]
async fn the_page_of_a_run_that_started_no_session_says_so() {
    let pages = Pages::of(view(
        vec![ticket(1, "Ship gh")],
        vec![open_pull(2, LINEWRIGHT, &[1], &[LINEWRIGHT])],
    ))
    .await;
    pages.record(&json!({
        "item": 2,
        "stage": "update",
        "n": 1,
        "started_at": "2026-10-14T12:00:00Z",
        "end": {"at": "2026-10-14T12:01:00Z", "ending": "done"},
        "sessions": [],
    }));

    let html = pages.page("/runs/2/update-1").await;

    assert_eq!(heading(&html), ["2/update-1 (it started no session)"]);
    let (item, links) = item_line(&html);
    assert_eq!(
        (item.as_str(), links[0].as_str()),
        ("#2: Pull 2", "/items/2")
    );
    assert_eq!(select(&html, "#transcript .entry").len(), 0);
    assert_eq!(events_path(&html), Some("/events/runs/2/update-1"));
}

#[tokio::test]
async fn the_page_of_no_run_says_there_is_none() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;

    let (no_run, no_run_body) = pages.get("/runs/1/rework-9").await;
    let (no_id, no_id_body) = pages.get("/runs/nothing").await;

    for (status, body, error) in [
        (no_run, no_run_body, "there is no run 1/rework-9"),
        (no_id, no_id_body, "\"nothing\" is not a run ID"),
    ] {
        assert_eq!(status, 404);
        let html = Html::parse_document(&body);
        let shown: Vec<String> = select(&html, "#transcript p")
            .into_iter()
            .map(text)
            .collect();
        assert_eq!(shown, [error]);
    }
}

#[tokio::test]
async fn the_run_pages_stream_passes_through_a_proxy_unbuffered_unchanged_and_uncached() {
    let pages = implemented_as_recorded().await;

    let response = pages.response("/events/runs/1/implement-1").await;

    assert_eq!(response.status(), 200);
    assert_passes_through_a_proxy(response.headers());
}

/// Each session's setup command, as its line, the output it opens to, and
/// whether it starts open.
fn setups_shown(html: &Html) -> Vec<(String, String, bool)> {
    select(html, ".entry.setup")
        .into_iter()
        .map(|setup| {
            let line = text(select_in(setup, ".line")[0]);
            let output = select_in(setup, ".output")
                .into_iter()
                .map(text)
                .collect::<String>();
            let open = select_in(setup, "details")
                .first()
                .is_some_and(|details| details.attr("open").is_some());
            (line, output, open)
        })
        .collect()
}

/// `docs/specs/design_system.md`, Transcripts, Times and Setup; The run
/// page, Folding and Verbose.
#[tokio::test]
async fn the_run_page_shows_a_setup_command_as_its_sessions_first_entry_folded_to_its_line() {
    let pages = implemented_after(
        &setting_up(
            "2026-10-13T10:09:08Z",
            "echo 'added 3 packages'",
            SetupStatus::Exited(0),
            0.2,
        ),
        Some("added 3 packages\n"),
    )
    .await;

    let html = pages.page("/runs/1/implement-1").await;
    let verbose = pages.page("/runs/1/implement-1?verbose=").await;

    let line = "0:00 setup: echo 'added 3 packages' → exit 0, 0s".to_string();
    assert_eq!(
        setups_shown(&html),
        [(line.clone(), "added 3 packages".to_string(), false)]
    );
    assert_eq!(
        setups_shown(&verbose),
        [(line, "added 3 packages".to_string(), true)]
    );
    let entries = select(&html, "#s1-entries > .entry");
    assert_eq!(entries[0].attr("id"), Some("s1-e1"));
    assert!(entries[0].value().classes().any(|class| class == "setup"));
}

#[tokio::test]
async fn the_page_of_a_run_whose_setup_command_failed_shows_it_open_under_each_session() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let sessions = [
        (
            "sessions/issue-1/20261014T120000Z-triage",
            "2026-10-14T12:00:00Z",
        ),
        (
            "sessions/issue-1/20261014T120010Z-triage",
            "2026-10-14T12:00:10Z",
        ),
    ];
    for (dir, start) in sessions {
        let output = setting_up(
            start,
            "echo 'npm ERR! missing lockfile'; exit 3",
            SetupStatus::Exited(3),
            0.1,
        );
        pages.write(&[
            (&format!("{dir}/output.jsonl"), &output),
            (&format!("{dir}/setup.log"), "npm ERR! missing lockfile\n"),
        ]);
    }
    pages.record(&json!({
        "item": 1,
        "stage": "triage",
        "n": 1,
        "started_at": "2026-10-14T12:00:00Z",
        "end": {"at": "2026-10-14T12:00:20Z", "ending": "handed-back"},
        "sessions": sessions.map(|(dir, _)| ran(dir)),
    }));

    let html = pages.page("/runs/1/triage-1").await;

    assert_eq!(
        heading(&html),
        [
            "1/triage-1",
            "session 1: started 2026-10-14 12:00:00 UTC",
            "session 2: started 2026-10-14 12:00:10 UTC",
        ]
    );
    let failed = (
        "0:00 setup: echo 'npm ERR! missing lockfile'; exit 3 → exit 3, 0s".to_string(),
        "npm ERR! missing lockfile".to_string(),
        true,
    );
    assert_eq!(setups_shown(&html), [failed.clone(), failed]);
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/design_system.md`, Transcripts, Running; The run page,
/// Following.
#[tokio::test]
async fn the_run_page_shows_a_setup_command_running_and_patches_its_line_once_it_ends() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let dir = "sessions/issue-1/20261014T120000Z-triage";
    let start = at("2026-10-14T12:00:00Z");
    pages.write(&[(
        &format!("{dir}/output.jsonl"),
        &format!("{}\n", setup_started(start, "npm ci")),
    )]);
    pages.record(&json!({
        "item": 1,
        "stage": "triage",
        "n": 1,
        "started_at": "2026-10-14T12:00:00Z",
        "end": null,
        "sessions": [ran(dir)],
    }));
    let _in_flight = lock_of(&pages.path("runs/items/1/triage-1.json"));

    let read = pages.page("/runs/1/triage-1").await;
    let events_path = events_path(&read).unwrap().to_string();
    let mut events = stream_with_ids(&pages, &events_path, None).await;
    pages.write(&[(&format!("{dir}/setup.log"), "installing\n")]);
    let line = setup_ended(
        start + chrono::TimeDelta::seconds(12),
        SetupStatus::Exited(0),
        12.4,
    );
    pages.append(&format!("{dir}/output.jsonl"), &format!("{line}\n"));
    let ended = transcript_events(&mut events, |sent| {
        sent.iter().any(|(_, data)| data.contains("exit 0"))
    })
    .await;

    assert_eq!(
        setups_shown(&read),
        [(
            "0:00 setup: npm ci → running".to_string(),
            String::new(),
            false
        )]
    );
    let [(target, swap, html)] = &patches(&ended.last().unwrap().1)[..] else {
        panic!("{ended:?}")
    };
    assert_eq!((target.as_str(), swap.as_str()), ("#s1-e1", "outerHTML"));
    assert_eq!(
        setups_shown(html),
        [(
            "0:00 setup: npm ci → exit 0, 12s".to_string(),
            "installing".to_string(),
            false
        )]
    );
}
