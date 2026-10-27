use super::*;

/// #1 implemented in a run whose transcript is the one recorded, with a
/// sub-agent's work and a call that failed.
async fn implemented_as_recorded() -> World {
    implemented_by(crate::logs::implement_as_recorded).await
}

/// #1 implemented in a run whose sessions `implement` scripts.
async fn implemented_by(implement: fn(&World)) -> World {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    implement(&world);
    world.store.open_issue(
        "maintainer",
        "Ship gh",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world
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

/// #1's run page, once its run, with the recorded transcript, is done.
pub(super) async fn run_page_of_implemented() -> (Web, Html) {
    run_page_of(implemented_as_recorded().await).await
}

/// #1's run page, once its run is done.
async fn run_page_of(world: World) -> (Web, Html) {
    let web = Web::start(world).await;
    let html = web
        .eventually_page("/runs?item=1", |html| {
            has_run(&runs_shown(html), "done", "1/implement-1")
        })
        .await;
    let link = select(&html, "#runs tbody a")[0]
        .attr("href")
        .unwrap()
        .to_string();
    assert_eq!(link, "/runs/1/implement-1");
    let page = web.page(&link).await;
    (web, page)
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
    let (_web, html) = run_page_of_implemented().await;

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
            "session-env: Linewright's own",
        ]
    );
    let prompts = texts("#s1 .layers .prompt");
    assert!(
        prompts.contains(&"implement.md: Linewright's own".to_string()),
        "{prompts:?}"
    );
}

/// `docs/specs/web/design_system.md`, Colour scheme: secondary text is in
/// Muted text.
#[tokio::test]
async fn the_run_page_shows_a_sessions_ran_with_line_its_layers_and_each_entrys_time_in_muted_text()
{
    let (web, html) = run_page_of_implemented().await;

    for selector in ["#s1 .ran", "#s1 .layers", "#transcript a.at"] {
        assert_in_muted_text(&html, selector);
    }
    web.stop().await;
}

/// `docs/specs/monitoring.md`, logs, Lines; `docs/specs/design_system.md`,
/// Time, Durations.
#[tokio::test]
async fn the_run_page_shows_how_long_a_sub_agent_ran_as_a_duration() {
    let (_web, html) =
        run_page_of(implemented_by(crate::logs::implement_with_a_sub_agent_of_3h_2m_9s).await)
            .await;

    assert_eq!(
        transcript_shown(&html, false)[2..],
        [
            "0:02 Agent(Review) → done, 3 tool uses, 3h 2m",
            "3:02:15 The review came back clean.",
        ]
    );
}

#[tokio::test]
async fn the_run_page_shows_what_logs_shows_with_each_tool_call_folded_to_its_line() {
    let (web, html) = run_page_of_implemented().await;

    // As `lw logs 1` prints them.
    assert_eq!(
        cut(&transcript_shown(&html, false), 100),
        [
            "run: 1/implement-1",
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
    web.stop().await;
}

#[tokio::test]
async fn the_run_pages_entry_stamp_shows_when_the_entry_happened_in_the_pages_tooltip() {
    let (web, html) = run_page_of_implemented().await;

    let stamp = select(&html, "#transcript a.at")
        .into_iter()
        .find(|stamp| text(*stamp) == "0:03")
        .unwrap();
    // The session started at 10:09:08, and the entry 3 seconds after.
    assert_eq!(stamp.attr("data-tip"), Some("2026-10-13 10:09:11 UTC"));
    assert_eq!(select(&html, "[title]").len(), 0);
    web.stop().await;
}

#[tokio::test]
async fn the_run_pages_verbose_switch_shows_what_logs_verbose_shows_with_everything_open() {
    let (web, html) = run_page_of_implemented().await;
    let switch = select(&html, "[role=switch]");
    assert_eq!(switch[0].attr("aria-checked"), Some("false"));
    let verbose_path = switch[0].attr("href").unwrap().to_string();
    assert_eq!(verbose_path, "/runs/1/implement-1?verbose=");

    let verbose = web.page(&verbose_path).await;

    // As `lw logs 1 --verbose` prints them.
    assert_eq!(
        cut(&transcript_shown(&verbose, true), 40),
        [
            "run: 1/implement-1",
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
    web.stop().await;
}

/// Each event the stream at `path` sends, as its ID, if it has one, and
/// its data, resuming after the event `last_id` if one is given.
async fn stream_with_ids(
    web: &Web,
    path: &str,
    last_id: Option<&str>,
) -> tokio::sync::mpsc::UnboundedReceiver<(Option<String>, String)> {
    let mut request = axum::http::Request::get(path).header("Host", "localhost");
    if let Some(last_id) = last_id {
        request = request.header("Last-Event-ID", last_id);
    }
    let response = web
        .router
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

#[tokio::test]
async fn an_automations_run_whose_id_holds_a_slash_has_its_page() {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/automations/weekly.md",
                "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    world.harness.on_automation(|_| {
        Ok(serde_json::json!({"summary": "Nothing needs doing.", "issues": []}).to_string())
    });
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();
    let web = Web::start(world).await;

    let runs = web.page("/runs").await;
    let html = web.page("/runs/weekly/1").await;

    let links: Vec<&str> = select(&runs, "#runs tbody td:nth-child(2) a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/runs/weekly/1"]);
    let shown = heading(&html);
    assert_eq!(shown[0], "run: weekly/1, by hand");
    assert!(shown[1].starts_with("session 1"), "{shown:?}");
    web.stop().await;
}

#[tokio::test]
async fn the_page_of_an_automations_run_by_hand_says_so_in_its_heading_and_of_a_scheduled_one_not()
{
    let mut world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    world.hourly_by_schedule_then_by_hand().await;
    let web = Web::start(world).await;

    let by_hand = web.page("/runs/hourly/2").await;
    let scheduled = web.page("/runs/hourly/1").await;

    assert_eq!(heading(&by_hand)[0], "run: hourly/2, by hand");
    assert_eq!(heading(&scheduled)[0], "run: hourly/1");
    web.stop().await;
}

#[tokio::test]
async fn the_page_of_a_run_that_started_no_session_says_so() {
    let mut world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .merge_change(
            "maintainer",
            "Other work",
            "",
            &[("src/other.txt", "other\n")],
        )
        .await
        .unwrap();
    // Brought up to date without a session: nothing conflicts.
    world.poll().await;
    let web = Web::start(world).await;

    let html = web.page(&format!("/runs/{}/update-1", pr.0)).await;

    assert_eq!(
        heading(&html),
        [format!("run: {}/update-1 (it started no session)", pr.0)]
    );
    assert_eq!(select(&html, "#transcript .entry").len(), 0);
    assert_eq!(events_path(&html), Some("/events/runs/2/update-1"));
    web.stop().await;
}

#[tokio::test]
async fn the_page_of_no_run_says_there_is_none() {
    let web = Web::start(with_ticket().await).await;

    let (no_run, no_run_body) = web.get("/runs/1/rework-9").await;
    let (no_id, no_id_body) = web.get("/runs/nothing").await;

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
    web.stop().await;
}

#[tokio::test]
async fn the_run_pages_stream_passes_through_a_proxy_unbuffered_unchanged_and_uncached() {
    let web = Web::start(implemented_as_recorded().await).await;
    web.eventually_page("/runs?item=1", |html| {
        has_run(&runs_shown(html), "done", "1/implement-1")
    })
    .await;

    let response = web.response("/events/runs/1/implement-1").await;

    assert_eq!(response.status(), 200);
    assert_passes_through_a_proxy(response.headers());
    web.stop().await;
}
