use super::*;

use linewright::transcript::{
    MaintenanceStatus, clone_ended, clone_started, maintenance_ended, maintenance_started,
    session_ended,
};
use serde_json::json;

/// Where #1's implementing session keeps its record.
const SESSION: &str = "sessions/issue-1/20261013T100908Z-implement";

/// How a session that ran with the harness's model and effort, and the
/// operator's implementing prompt, is kept in its run's record, its own
/// record in `dir`: a record kept before `maintenance` was named, which
/// names it `setup`.
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

/// The output of a session whose maintenance command `command` started at
/// `start` and ended `seconds` later as `status`.
fn setting_up(start: &str, command: &str, status: MaintenanceStatus, seconds: f64) -> String {
    let start = at(start);
    let end =
        start + chrono::TimeDelta::from_std(std::time::Duration::from_secs_f64(seconds)).unwrap();
    format!(
        "{}\n{}\n",
        maintenance_started(start, command),
        maintenance_ended(end, status, seconds)
    )
}

/// #1, `Ship gh`, implemented in run 1/implement-1 of one session, whose
/// transcript is the one recorded, with a sub-agent's work and a call that
/// failed.
pub(super) async fn implemented_as_recorded() -> Pages {
    implemented_after("", None).await
}

/// [`implemented_as_recorded`]'s, its session's output after `maintenance`, and
/// its maintenance command's output `maintenance_log`.
async fn implemented_after(maintenance: &str, maintenance_log: Option<&str>) -> Pages {
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
        maintenance,
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
    if let Some(log) = maintenance_log {
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

/// The run page's headers and entries in order, each entry as its line
/// with its text cut to `chars` characters, and with `io` its fold's input
/// and result too, as a reader reads them.
fn transcript_shown(html: &Html, chars: usize, io: bool) -> Vec<String> {
    select(html, "#transcript h1, #transcript h2, #transcript .entry")
        .into_iter()
        .map(|shown| {
            if !shown.value().classes().any(|class| class == "entry") {
                return text(shown);
            }
            let line = line_of(shown, chars);
            if !io {
                return line;
            }
            let parts: Vec<String> = select_in(shown, ".io")
                .into_iter()
                .filter(|part| entry_of(*part) == Some(shown))
                .map(text)
                .collect();
            [vec![line], parts].concat().join(" ")
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

/// The own line of `entry`, as a reader reads it: its text cut to `chars`
/// characters, then, where it shows one, ` | ` and its duration, which
/// reads `since` its start while it ticks.
#[track_caller]
fn line_of(entry: ElementRef, chars: usize) -> String {
    let line = select_in(entry, ".line")
        .into_iter()
        .find(|line| entry_of(*line) == Some(entry))
        .expect("an entry has a line");
    read_line(line, chars)
}

/// `line` as [`line_of`] reads an entry's.
#[track_caller]
fn read_line(line: ElementRef, chars: usize) -> String {
    let said: String = text(select_in(line, ".text")[0])
        .chars()
        .take(chars)
        .collect();
    let durations = select_in(line, "time.duration");
    match durations.first() {
        None => said,
        Some(duration) => match duration.attr("data-since") {
            Some(since) => format!("{said} | since {since}"),
            None => format!("{said} | {}", text(*duration)),
        },
    }
}

/// The session's settings, as their line and the lines they open to, and
/// whether they start open.
#[track_caller]
fn settings_shown(html: &Html) -> (String, Vec<String>, bool) {
    let settings = select(html, "#s1 .settings > details");
    assert_eq!(settings.len(), 1, "one settings fold");
    let line = text(select_in(settings[0], "summary .ran")[0]);
    let layers = select_in(settings[0], ".layers li")
        .into_iter()
        .map(text)
        .collect();
    (line, layers, settings[0].attr("open").is_some())
}

/// `docs/specs/design_system.md`, The run page, A session's settings and
/// Verbose.
#[test_log::test(tokio::test)]
async fn the_run_pages_session_settings_open_on_click_to_where_they_came_from_and_verbose_opens_them()
 {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;
    let verbose = pages.page("/runs/1/implement-1?verbose=").await;

    let layers: Vec<String> = [
        "model: the harness's",
        "effort: the harness's",
        "session-timeout: the operator's",
        "session-env: Linewright's own",
        "session-pass-env: Linewright's own",
        "maintenance: Linewright's own",
        "follow-ups/result-error.md: Linewright's own",
        "implement.md: the operator's",
    ]
    .map(String::from)
    .to_vec();
    let line = "model the harness's choice, effort the harness's choice".to_string();
    assert_eq!(settings_shown(&html), (line.clone(), layers.clone(), false));
    assert_eq!(settings_shown(&verbose), (line, layers, true));
    let chevron = select(&html, "#s1 .settings > details > summary > .chevron");
    assert_eq!(chevron.len(), 1);
}

/// `docs/specs/web/design_system.md`, Colour scheme: secondary text is in
/// Muted text.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_a_sessions_settings_and_each_entrys_duration_in_muted_text() {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;

    for selector in ["#s1 .ran", "#s1 .layers", "#transcript time.duration"] {
        assert_in_muted_text(&html, selector);
    }
}

/// `docs/specs/design_system.md`, The run page, Views, Prompts, Durations
/// and Folding.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_the_agents_default_view_with_each_tool_call_folded_to_its_line() {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;

    assert_eq!(
        transcript_shown(&html, 95, false),
        [
            "1/implement-1",
            "session 1: started 2026-10-13 10:09:08 UTC",
            "brief: # Your task",
            "Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit  | 0s",
            "Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → exit 2 | 0s",
            "Skill(linewright:tdd) → loaded | 0s",
            "Can't build images here (no Docker), and the repo has no test seam for the Dockerfile, so I'll ",
            "Read(Dockerfile) → 16 lines | 0s",
            "Edit(Dockerfile) → edited | 0s",
            "Agent(Standards review) → done, 2 tool uses 1 entry | 16s",
            "Bash(git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…) → exit 0 | 0s",
            "Both reviews came back clean. One optional fix: the README and the image label use the serial c",
            "{\"outcome\":\"done\",\"pr_title\":\"Ship gh and python3 in the base image\",\"pr_body\":\"## Summary\\n\\nT",
            "follow-up: Your final message could not be used: The final message holds no JSON object. End wi",
            "Bash(git commit -qam \"Ship gh and python3 in the base image…) → exit 0, 1 line | 0s",
            "{\"outcome\":\"done\",\"pr_title\":\"Ship gh and python3 in the base image\",\"pr_body\":\"## Summary\\n\\nT",
        ]
    );
    let calls = select(&html, "#transcript .entry.tool > details");
    assert_eq!(calls.len(), 8);
    for call in &calls {
        let line = text(select_in(*call, "summary.line .text")[0]);
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
    let counted = select(&html, "#transcript summary.line .count");
    assert_eq!(counted.len(), 1);
    let agent_call = counted[0]
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|ancestor| ancestor.value().name() == "details")
        .unwrap();
    assert_eq!(agent_call.attr("open"), None);
    let work: Vec<String> = select_in(agent_call, ".entry")
        .into_iter()
        .map(|entry| line_of(entry, 30))
        .collect();
    assert_eq!(work, ["Bash(git diff origin/main...HE | 0s"]);
    let brief = select(&html, "#transcript .entry.brief > details");
    assert_eq!(brief.len(), 1);
    assert_eq!(brief[0].attr("open"), None);
    assert_eq!(
        text(select_in(brief[0], ".prompt")[0]),
        "# Your task Implement the ticket below."
    );
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/web/design_system.md`, Transcripts, Lines.
#[test_log::test(tokio::test)]
async fn every_entrys_line_starts_with_a_chevron_column_that_only_an_entry_that_opens_fills() {
    let pages = implemented_as_recorded().await;

    for path in ["/runs/1/implement-1", "/runs/1/implement-1?verbose="] {
        let html = pages.page(path).await;

        let entries = select(&html, "#transcript .entry");
        assert_ne!(entries.len(), 0);
        for entry in entries {
            let line = select_in(entry, ".line")
                .into_iter()
                .find(|line| entry_of(*line) == Some(entry))
                .unwrap();
            let first = line.children().find_map(ElementRef::wrap).unwrap();
            assert!(
                first.value().classes().any(|class| class == "chevron"),
                "{}",
                line.html()
            );
            let opens = line.value().name() == "summary";
            let folds = select_in(entry, "details")
                .into_iter()
                .any(|fold| entry_of(fold) == Some(entry));
            assert_eq!(opens, folds, "{}", entry.html());
        }
    }
}

/// `docs/specs/design_system.md`, Out of scope: a link of its own to each
/// transcript entry.
#[test_log::test(tokio::test)]
async fn no_entry_on_the_run_page_has_a_link_of_its_own() {
    let pages = implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1?verbose=").await;

    assert_eq!(select(&html, "#transcript .entry a").len(), 0);
    assert_eq!(select(&html, "a[href^='#']").len(), 0);
}

/// `docs/specs/design_system.md`, The run page, Views and Verbose.
#[test_log::test(tokio::test)]
async fn the_run_pages_verbose_switch_shows_the_agents_verbose_view_with_everything_open() {
    let pages = implemented_as_recorded().await;
    let html = pages.page("/runs/1/implement-1").await;
    let switch = select(&html, "[role=switch]");
    assert_eq!(switch[0].attr("aria-checked"), Some("false"));
    let verbose_path = switch[0].attr("href").unwrap().to_string();
    assert_eq!(verbose_path, "/runs/1/implement-1?verbose=");

    let verbose = pages.page(&verbose_path).await;

    assert_eq!(
        cut(&transcript_shown(&verbose, usize::MAX, true), 40),
        [
            "1/implement-1",
            "session 1: started 2026-10-13 10:09:08 U",
            "brief: # Your task",
            "Bash(ls; cat Dockerfile; ls deploy; cat ",
            "thinking: The listing has a Dockerfile. ",
            "Bash(which podman buildah nerdctl hadoli",
            "Skill(linewright:tdd) → loaded | 0s inpu",
            "Can't build images here (no Docker), and",
            "Read(Dockerfile) → 16 lines | 0s input: ",
            "Edit(Dockerfile) → edited | 0s input: { ",
            "Agent(Standards review) → done, 2 tool u",
            "Bash(git diff origin/main...HEAD --stat ",
            "Both reviews came back clean. One option",
            "{\"outcome\":\"done\",\"pr_title\":\"Ship gh an",
            "follow-up: Your final message could not ",
            "Bash(git commit -qam \"Ship gh and python",
            "{\"outcome\":\"done\",\"pr_title\":\"Ship gh an",
        ]
    );
    let shown = transcript_shown(&verbose, 200, true);
    assert!(
        shown[3].contains(
            r#"→ exit 0, 19 lines | 0s input: { "command": "ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"image\\|git\\b\" README.md | head -50; cat CODING_STANDARDS.md | head -80" } result: AGENTS.md CODING_STANDARDS.md"#
        ),
        "{}",
        shown[3]
    );
    assert_eq!(
        shown[4],
        "thinking: The listing has a Dockerfile. I'll check what the image ships."
    );
    let folds = select(&verbose, "#transcript details");
    assert_eq!(folds.len(), 10);
    assert!(folds.iter().all(|fold| fold.attr("open").is_some()));
    let count = select(&verbose, "#transcript summary.line .count");
    assert_eq!(text(count[0]), "1 entry");
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

/// Each entry's line, as [`line_of`] reads it, by its ID.
fn entry_lines(html: &Html) -> Vec<(String, String)> {
    select(html, ".entry")
        .into_iter()
        .map(|entry| {
            (
                entry.attr("id").unwrap().to_string(),
                line_of(entry, usize::MAX),
            )
        })
        .collect()
}

#[test_log::test(tokio::test)]
async fn the_run_page_follows_a_run_in_flight_from_where_it_was_read_to_and_resumes_by_event_id() {
    let world = with_issue_for_triage().await;
    before_the_recorded_transcript(&world);
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    let turn = crate::local_api::turn_1();
    agent.writes(&turn.through_the_first_bash_result);
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

    agent.writes(&turn.the_second_bash_call);
    let made = transcript_events(&mut events, |sent| !sent.is_empty()).await;
    agent.writes(&turn.its_result);
    let returned = transcript_events(&mut events, |sent| !sent.is_empty()).await;
    agent.writes(&turn.the_rest);
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
        entry_lines(call)[0]
            .1
            .contains("2>&1) → running | since 2026-"),
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
        (format!("#{call_id}-fold").as_str(), "outerHTML")
    );
    let replaced = select(account, "details");
    assert!(replaced[0].attr("open").is_some());
    let returned_line = read_line(select_in(replaced[0], "summary.line")[0], usize::MAX);
    assert!(
        returned_line.ends_with("2>&1) → exit 2 | 0s"),
        "{returned_line}"
    );
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
    assert!(accounts.contains(&replaced_line("s1-e7")), "{accounts:?}");

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
                id.strip_suffix("-fold")
                    .or_else(|| id.strip_suffix("-line"))
            }) {
                let line = read_line(select(&html, ".line")[0], usize::MAX);
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

#[test_log::test(tokio::test)]
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
    assert_eq!((row[2].as_str(), &cell_links), ("#1: Ship gh", &links));
    assert_eq!(item_line(&html), ("#1: Ship gh".to_string(), links));
}

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
async fn the_run_pages_stream_passes_through_a_proxy_unbuffered_unchanged_and_uncached() {
    let pages = implemented_as_recorded().await;

    let response = pages.response("/events/runs/1/implement-1").await;

    assert_eq!(response.status(), 200);
    assert_passes_through_a_proxy(response.headers());
}

/// Each session's maintenance command, as its line, the output it opens to, and
/// whether it starts open.
fn maintenances_shown(html: &Html) -> Vec<(String, String, bool)> {
    select(html, ".entry.maintenance")
        .into_iter()
        .map(|maintenance| {
            let line = read_line(select_in(maintenance, ".line")[0], usize::MAX);
            let output = select_in(maintenance, ".output")
                .into_iter()
                .map(text)
                .collect::<String>();
            let open = select_in(maintenance, "details")
                .first()
                .is_some_and(|details| details.attr("open").is_some());
            (line, output, open)
        })
        .collect()
}

/// `docs/specs/design_system.md`, Transcripts, Maintenance; The run page,
/// Durations, Folding and Verbose.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_a_maintenance_command_as_its_sessions_first_entry_folded_to_its_line() {
    let pages = implemented_after(
        &setting_up(
            "2026-10-13T10:09:08Z",
            "echo 'added 3 packages'",
            MaintenanceStatus::Exited(0),
            0.2,
        ),
        Some("added 3 packages\n"),
    )
    .await;

    let html = pages.page("/runs/1/implement-1").await;
    let verbose = pages.page("/runs/1/implement-1?verbose=").await;

    let line = "maintenance: echo 'added 3 packages' → exit 0 | 0s".to_string();
    assert_eq!(
        maintenances_shown(&html),
        [(line.clone(), "added 3 packages".to_string(), false)]
    );
    assert_eq!(
        maintenances_shown(&verbose),
        [(line, "added 3 packages".to_string(), true)]
    );
    let entries = select(&html, "#s1-entries > .entry");
    assert_eq!(entries[0].attr("id"), Some("s1-e1"));
    assert!(
        entries[0]
            .value()
            .classes()
            .any(|class| class == "maintenance")
    );
}

#[test_log::test(tokio::test)]
async fn the_page_of_a_run_whose_maintenance_command_failed_shows_it_open_under_each_session() {
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
            MaintenanceStatus::Exited(3),
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
        "maintenance: echo 'npm ERR! missing lockfile'; exit 3 → exit 3 | 0s".to_string(),
        "npm ERR! missing lockfile".to_string(),
        true,
    );
    assert_eq!(maintenances_shown(&html), [failed.clone(), failed]);
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/design_system.md`, Transcripts, Running; The run page,
/// Following.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_a_maintenance_command_running_and_patches_its_line_once_it_ends() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let dir = "sessions/issue-1/20261014T120000Z-triage";
    let start = at("2026-10-14T12:00:00Z");
    pages.write(&[(
        &format!("{dir}/output.jsonl"),
        &format!("{}\n", maintenance_started(start, "npm ci")),
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
    let line = maintenance_ended(
        start + chrono::TimeDelta::seconds(12),
        MaintenanceStatus::Exited(0),
        12.4,
    );
    pages.append(&format!("{dir}/output.jsonl"), &format!("{line}\n"));
    let ended = transcript_events(&mut events, |sent| {
        sent.iter().any(|(_, data)| data.contains("exit 0"))
    })
    .await;

    assert_eq!(
        maintenances_shown(&read),
        [(
            "maintenance: npm ci → running | since 2026-10-14T12:00:00Z".to_string(),
            String::new(),
            false
        )]
    );
    let [(target, swap, html)] = &patches(&ended.last().unwrap().1)[..] else {
        panic!("{ended:?}")
    };
    assert_eq!((target.as_str(), swap.as_str()), ("#s1-e1", "outerHTML"));
    assert_eq!(
        maintenances_shown(html),
        [(
            "maintenance: npm ci → exit 0 | 12s".to_string(),
            "installing".to_string(),
            false
        )]
    );
}

/// The output of a session whose clone started being made at `start` and
/// ended `seconds` later with `error`.
fn cloning(start: &str, seconds: f64, error: Option<&str>) -> String {
    let start = at(start);
    let end =
        start + chrono::TimeDelta::from_std(std::time::Duration::from_secs_f64(seconds)).unwrap();
    format!(
        "{}\n{}\n",
        clone_started(start),
        clone_ended(end, seconds, error)
    )
}

/// Each session's clone, as its line, the error it opens to, and whether it
/// starts open.
fn clones_shown(html: &Html) -> Vec<(String, String, bool)> {
    select(html, ".entry.clone")
        .into_iter()
        .map(|clone| {
            let line = read_line(select_in(clone, ".line")[0], usize::MAX);
            let error = select_in(clone, ".output")
                .into_iter()
                .map(text)
                .collect::<String>();
            let open = select_in(clone, "details")
                .first()
                .is_some_and(|details| details.attr("open").is_some());
            (line, error, open)
        })
        .collect()
}

/// `docs/specs/design_system.md`, Transcripts, The clone; The run page,
/// Folding.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_the_clone_as_its_sessions_first_entry_before_the_maintenance_command() {
    let pages = implemented_after(
        &[
            cloning("2026-10-13T10:09:02Z", 6.2, None),
            setting_up(
                "2026-10-13T10:09:08Z",
                "echo 'added 3 packages'",
                MaintenanceStatus::Exited(0),
                0.2,
            ),
        ]
        .concat(),
        Some("added 3 packages\n"),
    )
    .await;

    let html = pages.page("/runs/1/implement-1").await;

    assert_eq!(
        clones_shown(&html),
        [("clone → done | 6s".to_string(), String::new(), false)]
    );
    let entries = select(&html, "#s1-entries > .entry");
    assert_eq!(entries[0].attr("id"), Some("s1-e1"));
    assert!(entries[0].value().classes().any(|class| class == "clone"));
    assert!(
        entries[1]
            .value()
            .classes()
            .any(|class| class == "maintenance")
    );
    assert_eq!(
        heading(&html)[1],
        "session 1: started 2026-10-13 10:09:02 UTC"
    );
}

/// `docs/specs/design_system.md`, The run page, Folding.
#[test_log::test(tokio::test)]
async fn the_page_of_a_run_whose_clone_could_not_be_made_shows_its_error_open() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let dir = "sessions/issue-1/20261014T120000Z-triage";
    pages.write(&[(
        &format!("{dir}/output.jsonl"),
        &cloning(
            "2026-10-14T12:00:00Z",
            3.0,
            Some("fetching origin: fatal: could not read from remote"),
        ),
    )]);
    pages.record(&json!({
        "item": 1,
        "stage": "triage",
        "n": 1,
        "started_at": "2026-10-14T12:00:00Z",
        "end": {"at": "2026-10-14T12:00:20Z", "ending": "handed-back"},
        "sessions": [ran(dir)],
    }));

    let html = pages.page("/runs/1/triage-1").await;

    assert_eq!(
        clones_shown(&html),
        [(
            "clone → failed | 3s".to_string(),
            "fetching origin: fatal: could not read from remote".to_string(),
            true
        )]
    );
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/design_system.md`, Transcripts, Running; The run page,
/// Following.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_the_clone_running_and_patches_its_line_once_it_is_made() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let dir = "sessions/issue-1/20261014T120000Z-triage";
    let start = at("2026-10-14T12:00:00Z");
    pages.write(&[(
        &format!("{dir}/output.jsonl"),
        &format!("{}\n", clone_started(start)),
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
    let line = clone_ended(start + chrono::TimeDelta::seconds(12), 12.4, None);
    pages.append(&format!("{dir}/output.jsonl"), &format!("{line}\n"));
    let ended = transcript_events(&mut events, |sent| {
        sent.iter().any(|(_, data)| data.contains("12s"))
    })
    .await;

    assert_eq!(
        clones_shown(&read),
        [(
            "clone → running | since 2026-10-14T12:00:00Z".to_string(),
            String::new(),
            false
        )]
    );
    let [(target, swap, html)] = &patches(&ended.last().unwrap().1)[..] else {
        panic!("{ended:?}")
    };
    assert_eq!((target.as_str(), swap.as_str()), ("#s1-e1", "outerHTML"));
    assert_eq!(
        clones_shown(html),
        [("clone → done | 12s".to_string(), String::new(), false)]
    );
}

/// `docs/specs/design_system.md`, Transcripts, Running; The run page,
/// Following.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_an_entry_its_session_ends_first_as_interrupted() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let dir = "sessions/issue-1/20261014T120000Z-triage";
    let start = at("2026-10-14T12:00:00Z");
    let call = json!({
        "type": "assistant",
        "timestamp": "2026-10-14T12:00:20Z",
        "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}},
        ]},
    });
    pages.write(&[(
        &format!("{dir}/output.jsonl"),
        &format!(
            "{}\n{}\n{call}\n",
            clone_started(start),
            clone_ended(start + chrono::TimeDelta::seconds(2), 2.0, None),
        ),
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
    let end = session_ended(start + chrono::TimeDelta::seconds(30));
    pages.append(&format!("{dir}/output.jsonl"), &format!("{end}\n"));
    let ended = transcript_events(&mut events, |sent| {
        sent.iter().any(|(_, data)| data.contains("interrupted"))
    })
    .await;

    assert!(
        entry_lines(&read)[1].1 == "Bash(cargo test) → running | since 2026-10-14T12:00:20Z",
        "{:?}",
        entry_lines(&read)
    );
    let lines: Vec<String> = patches(&ended.last().unwrap().1)
        .iter()
        .filter_map(|(_, _, html)| {
            select(html, ".line")
                .first()
                .map(|line| read_line(*line, usize::MAX))
        })
        .collect();
    assert_eq!(lines, ["Bash(cargo test) → interrupted | 10s"]);
}

/// `docs/specs/design_system.md`, Transcripts, Running; The run page,
/// Durations.
#[test_log::test(tokio::test)]
async fn the_run_page_shows_no_duration_for_an_entry_interrupted_where_its_sessions_end_is_not_known()
 {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let dir = "sessions/issue-1/20261014T120000Z-triage";
    let call = json!({
        "type": "assistant",
        "timestamp": "2026-10-14T12:00:20Z",
        "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}},
        ]},
    });
    pages.write(&[(&format!("{dir}/output.jsonl"), &format!("{call}\n"))]);
    pages.record(&json!({
        "item": 1,
        "stage": "triage",
        "n": 1,
        "started_at": "2026-10-14T12:00:00Z",
        "end": {"at": "2026-10-14T12:05:00Z", "ending": "handed-back"},
        "sessions": [ran(dir)],
    }));

    let html = pages.page("/runs/1/triage-1").await;

    let lines: Vec<String> = entry_lines(&html)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    assert_eq!(lines, ["Bash(cargo test) → interrupted"]);
}
