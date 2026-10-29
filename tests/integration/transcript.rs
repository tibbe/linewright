use linewright::domain::Timestamp;
use linewright::runs::{RunId, Started};
use linewright::transcript::{
    self, After, Follower, SessionRecord, SetupRecord, SetupStatus, Transcript, Verbosity,
};
use serde_json::json;

fn recorded() -> SessionRecord {
    SessionRecord {
        output: [
            include_str!("fixtures/transcript/turn-1.jsonl"),
            include_str!("fixtures/transcript/turn-2.jsonl"),
        ]
        .concat(),
        brief: Some("/linewright:implement\n\n# Your task\n\nShip gh.\n".into()),
        followups: vec!["Your answer did not parse as the result.\n\nSend it again.\n".into()],
        ran: None,
        setup_output: None,
        setup: None,
    }
}

fn item_run(id: RunId) -> Started {
    Started::new(id, None).unwrap()
}

fn of(sessions: &[SessionRecord]) -> Transcript {
    Transcript {
        run: item_run("105/implement-1".parse().unwrap()),
        in_flight: None,
        sessions: sessions.iter().map(transcript::parse).collect(),
    }
}

#[test]
fn compact_shows_messages_in_full_and_each_tool_call_on_one_line() {
    let shown = transcript::render(&of(&[recorded()]), Verbosity::Compact, false);

    assert_eq!(
        shown,
        r###"run: 105/implement-1
session 1: started 2026-10-13 10:09:08 UTC
 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i "im…) → exit 0, 19 lines
 0:14  Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → exit 2
 0:15  Skill(linewright:tdd) → loaded
 0:18  Can't build images here (no Docker), and the repo has no test seam for the Dockerfile, so I'll make the Dockerfile and README changes and check them by hand.
 0:27  Read(Dockerfile) → 16 lines
 0:35  Edit(Dockerfile) → edited
 2:00  Agent(Standards review) → done, 2 tool uses, 16s
 2:02    Bash(git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…) → exit 0, 7 lines
 2:45  Both reviews came back clean. One optional fix: the README and the image label use the serial comma differently. Making them match.
 2:59  {"outcome":"done","pr_title":"Ship gh and python3 in the base image","pr_body":"## Summary\n\nThe base image's `runtime` stage now installs `gh` from GitHub's apt repository and `python3` from Debian. `gh`'s keyring is checked against a pin
 3:22  follow-up: Your answer did not parse as the result. …
 3:25  Bash(git commit -qam "Ship gh and python3 in the base image…) → exit 0, 1 line
 3:28  {"outcome":"done","pr_title":"Ship gh and python3 in the base image","pr_body":"## Summary\n\nThe base image's `runtime` stage now installs `gh` from GitHub's apt repository and `python3` from Debian. `gh`'s keyring is checked against a pin
"###
    );
}

#[test]
fn verbose_adds_the_brief_the_thinking_each_tools_input_and_result_and_the_follow_ups() {
    let shown = transcript::render(&of(&[recorded()]), Verbosity::Full, false);

    for part in [
        "session 1: started 2026-10-13 10:09:08 UTC\n \
         0:00  brief: /linewright:implement\n\
         \n       \
         # Your task\n\
         \n       \
         Ship gh.\n \
         0:00  event: {",
        " 0:04  thinking: The listing has a Dockerfile.\n       \
         I'll check what the image ships.\n",
        " 0:14  Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → exit 2\n         \
         input: {\n           \
         \"command\": \"which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1\"\n         \
         }\n         \
         result: Exit code 2\n         \
         ls: cannot access '/var/run/docker.sock': No such file or directory\n         \
         details: \"Error: Exit code 2\\nls: cannot access '/var/run/docker.sock': No such file or directory\"\n \
         0:15  Skill(",
        " 0:15  context: Base directory for this skill: /srv/linewright/clones/issue-105-WTrFF0/plugin/skills/tdd\n",
        " 2:00    context: Review a diff in /srv/linewright/clones/issue-105-WTrFF0/repo against its coding standards.",
        " 3:22  follow-up: Your answer did not parse as the result.\n\
         \n       \
         Send it again.\n \
         3:22  event: {",
    ] {
        assert!(shown.contains(part), "{part}\nnot in\n{shown}");
    }
    let events = |kind: &str| {
        shown
            .lines()
            .filter(|line| line.contains("  event: {") && line.contains(kind))
            .count()
    };
    // Each turn's start and end, and the sub-agent's.
    assert_eq!(events(r#""subtype":"init""#), 2);
    assert_eq!(events(r#""type":"result""#), 2);
    assert_eq!(events(r#""subtype":"task_notification""#), 1);
    assert!(
        shown.contains(" 0:27  Read(Dockerfile) → 16 lines\n         input: {\n"),
        "{shown}"
    );
    assert!(shown.contains("         details: {\n"), "{shown}");
}

#[test]
fn json_prints_one_object_per_entry_and_line() {
    let transcript = of(&[recorded()]);

    let compact: Vec<serde_json::Value> = transcript::json_lines(&transcript, Verbosity::Compact)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let full: Vec<serde_json::Value> = transcript::json_lines(&transcript, Verbosity::Full)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert_eq!(
        compact[0],
        serde_json::json!({"type": "run", "run": "105/implement-1", "sessions": 1})
    );
    assert_eq!(
        compact[1],
        serde_json::json!({"type": "session", "session": 1, "started_at": "2026-10-13T10:09:08Z"})
    );
    assert_eq!(compact.len(), 15);
    assert_eq!(
        compact[9],
        serde_json::json!({
            "type": "tool",
            "session": 1,
            "at": "2026-10-13T10:11:10.102Z",
            "elapsed_seconds": 122,
            "depth": 1,
            "parent_tool_use_id": "toolu_01T5JmkDxVfWvgdZeKR3KCjP",
            "tool_use_id": "toolu_01LBDfAEHMPeecSbXMbfKv6Z",
            "tool": "Bash",
            "argument": "git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…",
            "account": "exit 0, 7 lines",
            "error": false,
        })
    );
    assert_eq!(compact[8]["tool_use_id"], "toolu_01T5JmkDxVfWvgdZeKR3KCjP");
    assert_eq!(compact[8].get("parent_tool_use_id"), None);
    assert_eq!(compact[11]["type"], "message");
    assert_eq!(compact[11].get("tool_use_id"), None);
    assert_eq!(compact[11].get("parent_tool_use_id"), None);
    let sub_agents_prompt = full
        .iter()
        .find(|line| line["type"] == "context" && line["depth"] == 1)
        .unwrap();
    assert_eq!(
        sub_agents_prompt["parent_tool_use_id"],
        "toolu_01T5JmkDxVfWvgdZeKR3KCjP"
    );
    assert_eq!(
        compact[12]["summary"],
        "Your answer did not parse as the result. …"
    );
    assert_eq!(compact[12].get("text"), None);
    let follow_up = full
        .iter()
        .find(|line| line["type"] == "follow-up")
        .unwrap();
    assert_eq!(
        follow_up["text"],
        "Your answer did not parse as the result.\n\nSend it again.\n"
    );
    let thinking = full.iter().find(|line| line["type"] == "thinking").unwrap();
    assert_eq!(
        thinking["text"],
        "The listing has a Dockerfile.\nI'll check what the image ships."
    );
    let failed = full
        .iter()
        .find(|line| line["type"] == "tool" && line["error"] == true)
        .unwrap();
    assert_eq!(
        failed["input"],
        serde_json::json!({"command": "which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1"})
    );
    assert_eq!(
        failed["result"],
        "Exit code 2\nls: cannot access '/var/run/docker.sock': No such file or directory"
    );
}

#[test]
fn verbose_shows_content_of_kinds_it_has_no_line_of_its_own_for() {
    let events = [
        json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
        json!({
            "type": "assistant",
            "timestamp": "2026-10-13T10:00:01Z",
            "message": {"content": [{"type": "redacted_thinking", "data": "x"}]},
        }),
        json!({
            "type": "assistant",
            "timestamp": "2026-10-13T10:00:02Z",
            "message": {"content": [
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a.png"}},
                {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "b.txt"}},
            ]},
        }),
        json!({
            "type": "user",
            "timestamp": "2026-10-13T10:00:03Z",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "image", "source": "png"}]},
                {"type": "tool_result", "tool_use_id": "t2", "content": "one\ntwo"},
            ]},
            "tool_use_result": {"note": "shared"},
        }),
    ];
    let record = SessionRecord {
        output: events.iter().map(|event| format!("{event}\n")).collect(),
        ..SessionRecord::default()
    };

    let full = transcript::render(&of(std::slice::from_ref(&record)), Verbosity::Full, false);
    let compact = transcript::render(&of(&[record]), Verbosity::Compact, false);

    assert_eq!(
        full,
        r#"run: 105/implement-1
session 1: started 2026-10-13 10:00:00 UTC
 0:01  other: {"data":"x","type":"redacted_thinking"}
 0:02  Read(a.png) → 0 lines
         input: {
           "file_path": "a.png"
         }
         result: 
         content: {"source":"png","type":"image"}
 0:02  Read(b.txt) → 2 lines
         input: {
           "file_path": "b.txt"
         }
         result: one
         two
 0:03  other: {"tool_use_result":{"note":"shared"}}
"#
    );
    assert_eq!(
        compact,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:02  Read(a.png) → 0 lines\n \
         0:02  Read(b.txt) → 2 lines\n"
    );
}

fn written(record: &SessionRecord, bytes: usize) -> SessionRecord {
    SessionRecord {
        output: record.output[..bytes].to_string(),
        ..record.clone()
    }
}

fn text(lines: &[transcript::FollowedLine]) -> String {
    lines
        .iter()
        .map(|followed| transcript::render_line(&followed.line, Verbosity::Compact, false))
        .collect()
}

fn json(lines: &[transcript::FollowedLine]) -> Vec<serde_json::Value> {
    lines
        .iter()
        .map(|followed| serde_json::from_str(&transcript::json_line(&followed.line)).unwrap())
        .collect()
}

#[test]
fn following_shows_each_entry_once_it_is_written_and_each_tool_call_when_made_and_when_it_returns()
{
    let record = recorded();
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );
    let mut shown = vec![];
    let mut at_each_line = vec![];

    for (end, _) in record.output.match_indices('\n') {
        let lines = follower.update(&[written(&record, end + 1)]);
        at_each_line.push(text(&lines));
        shown.extend(lines);
    }
    shown.extend(follower.end(&[record]));

    // The first Bash call, written on the third line, shows as running,
    // then again once its result is written, on the fourth.
    assert_eq!(
        at_each_line[..4],
        [
            "run: 105/implement-1\nsession 1: started 2026-10-13 10:09:08 UTC\n",
            "",
            " 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → running\n",
            " 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit 0, 19 lines\n",
        ]
    );
    assert_eq!(
        text(&shown),
        r###"run: 105/implement-1
session 1: started 2026-10-13 10:09:08 UTC
 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i "im…) → running
 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i "im…) → exit 0, 19 lines
 0:14  Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → running
 0:14  Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → exit 2
 0:15  Skill(linewright:tdd) → running
 0:15  Skill(linewright:tdd) → loaded
 0:18  Can't build images here (no Docker), and the repo has no test seam for the Dockerfile, so I'll make the Dockerfile and README changes and check them by hand.
 0:27  Read(Dockerfile) → running
 0:27  Read(Dockerfile) → 16 lines
 0:35  Edit(Dockerfile) → running
 0:36  Edit(Dockerfile) → edited
 2:00  Agent(Standards review) → running
 2:02    Bash(git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…) → running
 2:02    Bash(git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…) → exit 0, 7 lines
 2:16  Agent(Standards review) → done, 2 tool uses, 16s
 2:45  Both reviews came back clean. One optional fix: the README and the image label use the serial comma differently. Making them match.
 2:59  {"outcome":"done","pr_title":"Ship gh and python3 in the base image","pr_body":"## Summary\n\nThe base image's `runtime` stage now installs `gh` from GitHub's apt repository and `python3` from Debian. `gh`'s keyring is checked against a pin
 3:22  follow-up: Your answer did not parse as the result. …
 3:25  Bash(git commit -qam "Ship gh and python3 in the base image…) → running
 3:25  Bash(git commit -qam "Ship gh and python3 in the base image…) → exit 0, 1 line
 3:28  {"outcome":"done","pr_title":"Ship gh and python3 in the base image","pr_body":"## Summary\n\nThe base image's `runtime` stage now installs `gh` from GitHub's apt repository and `python3` from Debian. `gh`'s keyring is checked against a pin
"###
    );
}

#[test]
fn following_shows_the_same_lines_whatever_pieces_the_output_is_written_in() {
    let record = recorded();
    let follow = |ends: Vec<usize>| {
        let mut follower = Follower::new(
            item_run("105/implement-1".parse().unwrap()),
            Verbosity::Full,
            None,
        );
        let mut shown = vec![];
        for end in ends {
            shown.extend(follower.update(&[written(&record, end)]));
        }
        shown.extend(follower.end(std::slice::from_ref(&record)));
        shown
    };
    let by_line = record.output.match_indices('\n').map(|(end, _)| end + 1);
    let by_chars = record.output.char_indices().map(|(i, _)| i).step_by(7);

    let at_once = follow(vec![]);
    let read_by_line = follow(by_line.collect());
    let read_by_chars = follow(by_chars.collect());

    assert_eq!(read_by_line, at_once);
    assert_eq!(read_by_chars, at_once);
    assert!(text(&at_once).contains(" 0:04  thinking: The listing has a Dockerfile.\n"));
    for transcript::FollowedLine { line, .. } in &at_once {
        let json = transcript::json_line(line);
        assert_eq!(
            &serde_json::from_str::<transcript::Line>(&json).unwrap(),
            line,
            "{json}"
        );
    }
}

#[test]
fn following_after_a_line_gives_the_lines_after_it() {
    let record = recorded();
    let follow = |after| {
        let mut follower = Follower::new(
            item_run("105/implement-1".parse().unwrap()),
            Verbosity::Compact,
            after,
        );
        follower.end(std::slice::from_ref(&record))
    };
    let all = follow(None);
    let at = |session, line| transcript::ReadTo { session, line };

    let after_the_third = follow(Some(After::Line(all[2].made)));
    let after_line_3 = follow(Some(After::Read(at(1, 3))));

    assert_eq!(after_the_third, all[3..]);
    assert_eq!(
        text(&after_line_3[..1]),
        " 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit 0, 19 lines\n"
    );
}

#[test]
fn following_a_run_that_ends_shows_each_of_its_sessions_as_logs_does() {
    let record = recorded();
    let wrote_nothing = SessionRecord::default();
    let sessions = [wrote_nothing.clone(), record, wrote_nothing];
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );

    let mut shown = follower.update(&sessions[..1]);
    shown.extend(follower.update(&sessions[..2]));
    shown.extend(follower.end(&sessions));

    let headers = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with("run:") || line.starts_with("session "))
            .map(str::to_string)
            .collect()
    };
    let logs = transcript::render(&of(&sessions), Verbosity::Compact, false);

    assert_eq!(
        headers(&text(&shown)),
        [
            "run: 105/implement-1",
            "session 1",
            "session 2: started 2026-10-13 10:09:08 UTC",
            "session 3",
        ]
    );
    assert_eq!(headers(&text(&shown)), headers(&logs));
}

#[test]
fn following_shows_a_tool_call_as_running_as_soon_as_it_is_made() {
    let events = |events: &[serde_json::Value]| SessionRecord {
        output: events.iter().map(|event| format!("{event}\n")).collect(),
        ..SessionRecord::default()
    };
    let first = events(&[
        json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
        json!({
            "type": "assistant",
            "timestamp": "2026-10-13T10:00:02Z",
            "message": {"content": [
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}},
            ]},
        }),
    ]);
    let second = events(&[
        json!({"type": "linewright-turn", "started_at": "2026-10-13T11:00:00Z", "started_by": "brief"}),
        json!({
            "type": "assistant",
            "timestamp": "2026-10-13T11:00:05Z",
            "message": {"content": [{"type": "text", "text": "Starting over."}]},
        }),
    ]);
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );

    let one = text(&follower.update(std::slice::from_ref(&first)));
    let two = text(&follower.update(&[first, second]));
    let mut nothing = Follower::new(
        item_run("105/update-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );

    assert_eq!(
        one,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:02  Bash(cargo test) → running\n"
    );
    assert_eq!(
        two,
        "session 2: started 2026-10-13 11:00:00 UTC\n \
         0:05  Starting over.\n"
    );
    assert_eq!(text(&nothing.update(&[])), "");
    assert_eq!(
        text(&nothing.end(&[])),
        "run: 105/update-1 (it started no session)\n"
    );
}

#[test]
fn following_shows_a_tool_call_again_once_it_ends_in_the_background() {
    let record = SessionRecord {
        output: [
            json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
            json!({
                "type": "assistant",
                "timestamp": "2026-10-13T10:00:02Z",
                "message": {"content": [
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}},
                ]},
            }),
            json!({
                "type": "user",
                "timestamp": "2026-10-13T10:00:04Z",
                "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "Moved to the background."},
                ]},
                "tool_use_result": {"backgroundTaskId": "b1"},
            }),
            json!({"type": "system", "subtype": "task_notification", "tool_use_id": "t1", "status": "completed"}),
        ]
        .iter()
        .map(|event| format!("{event}\n"))
        .collect(),
        ..SessionRecord::default()
    };
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );

    let lines = follower.end(&[record]);
    let shown = text(&lines);
    let ids: Vec<serde_json::Value> = json(&lines[2..])
        .into_iter()
        .map(|line| line["tool_use_id"].clone())
        .collect();

    assert_eq!(
        shown,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:02  Bash(cargo test) → running\n \
         0:04  Bash(cargo test) → moved to the background\n \
         0:04  Bash(cargo test) → completed in the background\n"
    );
    assert_eq!(ids, [json!("t1"), json!("t1"), json!("t1")]);
}

#[test]
fn json_ties_each_tool_call_and_sub_agent_object_to_the_agents_call_ids() {
    let tool_use = |id: &str, name: &str, input: serde_json::Value| json!({"type": "tool_use", "id": id, "name": name, "input": input});
    let result =
        |id: &str, text: &str| json!({"type": "tool_result", "tool_use_id": id, "content": text});
    let message = |kind: &str, parent: Option<&str>, content: Vec<serde_json::Value>| {
        json!({
            "type": kind,
            "timestamp": "2026-10-13T10:00:01Z",
            "parent_tool_use_id": parent,
            "message": {"content": content},
        })
    };
    let assistant = |parent, content| message("assistant", parent, content);
    let user = |parent, content| message("user", parent, content);
    // A sub-agent and a command started in parallel; the sub-agent starts
    // one of its own, and the results come back out of order.
    let events = [
        json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
        assistant(
            None,
            vec![
                tool_use("a1", "Agent", json!({"description": "Review"})),
                tool_use("b1", "Bash", json!({"command": "cargo test"})),
            ],
        ),
        user(
            Some("a1"),
            vec![json!({"type": "text", "text": "Review it."})],
        ),
        assistant(
            Some("a1"),
            vec![tool_use(
                "a2",
                "Agent",
                json!({"description": "Look deeper"}),
            )],
        ),
        assistant(
            Some("a2"),
            vec![
                json!({"type": "text", "text": "Reading."}),
                tool_use("r1", "Read", json!({"file_path": "a.rs"})),
            ],
        ),
        user(None, vec![result("b1", "ok")]),
        user(Some("a2"), vec![result("r1", "one")]),
        user(Some("a1"), vec![result("a2", "Fine.")]),
        user(None, vec![result("a1", "Clean.")]),
    ];
    let record = SessionRecord {
        output: events.iter().map(|event| format!("{event}\n")).collect(),
        ..SessionRecord::default()
    };
    let summaries = |lines: Vec<serde_json::Value>| -> Vec<String> {
        lines
            .iter()
            .filter(|line| line.get("depth").is_some())
            .map(|line| {
                let field = |name| line.get(name).and_then(|id| id.as_str()).unwrap_or("-");
                format!(
                    "{} {} {} {}",
                    line["type"].as_str().unwrap(),
                    field("tool_use_id"),
                    field("parent_tool_use_id"),
                    field("account"),
                )
            })
            .collect()
    };
    let whole = |verbosity| {
        summaries(
            transcript::json_lines(&of(std::slice::from_ref(&record)), verbosity)
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
        )
    };
    let followed = |verbosity| {
        let mut follower = Follower::new(
            item_run("105/implement-1".parse().unwrap()),
            verbosity,
            None,
        );
        let mut shown = vec![];
        for (end, _) in record.output.match_indices('\n') {
            shown.extend(follower.update(&[written(&record, end + 1)]));
        }
        shown.extend(follower.end(std::slice::from_ref(&record)));
        summaries(json(&shown))
    };

    assert_eq!(
        whole(Verbosity::Compact),
        [
            "tool a1 - Clean.",
            "tool a2 a1 Fine.",
            "message - a2 -",
            "tool r1 a2 1 line",
            "tool b1 - exit 0, 1 line",
        ]
    );
    assert_eq!(
        whole(Verbosity::Full),
        [
            "tool a1 - Clean.",
            "context - a1 -",
            "tool a2 a1 Fine.",
            "message - a2 -",
            "tool r1 a2 1 line",
            "tool b1 - exit 0, 1 line",
        ]
    );
    assert_eq!(
        followed(Verbosity::Compact),
        [
            "tool a1 - running",
            "tool b1 - running",
            "tool a2 a1 running",
            "message - a2 -",
            "tool r1 a2 running",
            "tool b1 - exit 0, 1 line",
            "tool r1 a2 1 line",
            "tool a2 a1 Fine.",
            "tool a1 - Clean.",
        ]
    );
    assert_eq!(
        followed(Verbosity::Full),
        [
            "tool a1 - running",
            "tool b1 - running",
            "context - a1 -",
            "tool a2 a1 running",
            "message - a2 -",
            "tool r1 a2 running",
            "tool b1 - exit 0, 1 line",
            "tool r1 a2 1 line",
            "tool a2 a1 Fine.",
            "tool a1 - Clean.",
        ]
    );
}

#[test]
fn an_entry_that_says_no_time_shows_the_time_of_the_one_the_agent_wrote_before_it() {
    let shown = transcript::render(&of(&[recorded()]), Verbosity::Full, false);
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Full,
        None,
    );
    let followed = text(&follower.end(&[recorded()]));

    // The sub-agent's start, which says no time, written just after the
    // call that started the sub-agent at 2:00, and before the sub-agent's
    // work at 2:02.
    let started = |text: &str| {
        text.lines()
            .find(|line| line.contains("event: {\"description\":\"Standards review\""))
            .map(|line| line[..5].to_string())
    };
    assert_eq!(started(&shown).as_deref(), Some(" 2:00"));
    assert_eq!(started(&followed).as_deref(), Some(" 2:00"));
}

fn time(at: &str) -> Timestamp {
    at.parse().unwrap()
}

/// The record of a session whose setup command `command` started at
/// `start`, then, if it has, ended as `ended` gives, `(status, seconds)`,
/// having written `output`.
fn setting_up(
    start: &str,
    command: &str,
    ended: Option<(SetupStatus, f64)>,
    output: &str,
) -> SessionRecord {
    let start = time(start);
    let mut events = transcript::setup_started(start, command) + "\n";
    let mut setup_output = None;
    if let Some((status, seconds)) = ended {
        let at = start
            + chrono::TimeDelta::from_std(std::time::Duration::from_secs_f64(seconds)).unwrap();
        events += &(transcript::setup_ended(at, status, seconds) + "\n");
        setup_output = Some(output.to_string());
    }
    SessionRecord {
        output: events,
        setup_output,
        ..SessionRecord::default()
    }
}

/// The recorded session after a setup command that succeeded, then one
/// whose setup command failed, so that it never started.
fn after_setting_up() -> [SessionRecord; 2] {
    let set_up = setting_up(
        "2026-10-13T10:08:56Z",
        "npm ci",
        Some((SetupStatus::Exited(0), 12.4)),
        "added 3 packages\nin 12s\n",
    );
    [
        SessionRecord {
            output: set_up.output.clone() + &recorded().output,
            ..SessionRecord {
                setup_output: set_up.setup_output,
                ..recorded()
            }
        },
        setting_up(
            "2026-10-13T10:20:00Z",
            "npm ci",
            Some((SetupStatus::Exited(1), 3.0)),
            "npm ERR! missing lockfile\n",
        ),
    ]
}

/// `docs/specs/design_system.md`, Transcripts, Times and Setup.
#[test]
fn a_setup_command_is_its_sessions_first_entry_at_its_start_with_its_output_only_in_full_or_when_it_failed()
 {
    let transcript = of(&after_setting_up());

    let compact = transcript::render(&transcript, Verbosity::Compact, false);
    let full = transcript::render(&transcript, Verbosity::Full, false);

    let compact: Vec<&str> = compact.lines().collect();
    assert_eq!(
        compact[..3],
        [
            "run: 105/implement-1",
            "session 1: started 2026-10-13 10:08:56 UTC",
            " 0:00  setup: npm ci → exit 0, 12s",
        ]
    );
    // The agent's first call, at 10:09:11, counts from the setup's start.
    assert!(compact[3].starts_with(" 0:15  Bash(ls;"), "{compact:#?}");
    assert_eq!(
        compact[compact.len() - 3..],
        [
            "session 2: started 2026-10-13 10:20:00 UTC",
            " 0:00  setup: npm ci → exit 1, 3s",
            "         npm ERR! missing lockfile",
        ]
    );
    let full: Vec<&str> = full.lines().collect();
    assert_eq!(
        full[2..5],
        [
            " 0:00  setup: npm ci → exit 0, 12s",
            "         added 3 packages",
            "         in 12s",
        ]
    );
    assert!(full[5].starts_with(" 0:12  brief: "), "{full:#?}");
}

#[test]
fn a_setup_command_shows_how_it_ended_and_its_output_unless_it_succeeded_or_was_cancelled() {
    let shown = |status| {
        let record = setting_up(
            "2026-10-13T10:00:00Z",
            "make deps",
            Some((status, 61.0)),
            "fetching\n",
        );
        let shown = transcript::render(&of(&[record]), Verbosity::Compact, false);
        shown.lines().skip(2).collect::<Vec<_>>().join("\n")
    };

    assert_eq!(
        shown(SetupStatus::Signalled(9)),
        " 0:00  setup: make deps → signal 9, 1m 1s\n         fetching"
    );
    assert_eq!(
        shown(SetupStatus::TimedOut),
        " 0:00  setup: make deps → timed out, 1m 1s\n         fetching"
    );
    assert_eq!(
        shown(SetupStatus::Cancelled),
        " 0:00  setup: make deps → cancelled, 1m 1s"
    );
}

/// `docs/specs/design_system.md`, Transcripts, Running.
#[test]
fn a_setup_command_shows_running_until_it_ends() {
    let running = setting_up("2026-10-13T10:00:00Z", "npm ci", None, "");
    let transcript = Transcript {
        in_flight: Some(transcript::InFlight::ReadTo(transcript::ReadTo {
            session: 1,
            line: 1,
        })),
        ..of(&[running])
    };

    let shown = transcript::render(&transcript, Verbosity::Full, false);

    assert_eq!(
        shown,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:00  setup: npm ci → running\n"
    );
}

/// `docs/specs/monitoring.md`, logs, Setup.
#[test]
fn json_gives_a_setup_command_with_its_account_and_output_once_it_has_ended() {
    let running = setting_up("2026-10-13T10:00:00Z", "npm ci", None, "");
    let [succeeded, failed] = after_setting_up();

    let json = |records: &[SessionRecord], verbosity| -> Vec<serde_json::Value> {
        transcript::json_lines(&of(records), verbosity)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };

    assert_eq!(
        json(&[running], Verbosity::Compact)[2],
        json!({
            "type": "setup",
            "session": 1,
            "at": "2026-10-13T10:00:00Z",
            "elapsed_seconds": 0,
            "depth": 0,
            "command": "npm ci",
        })
    );
    for verbosity in [Verbosity::Compact, Verbosity::Full] {
        let lines = json(&[succeeded.clone(), failed.clone()], verbosity);
        assert_eq!(lines[1]["type"], "session");
        assert_eq!(
            lines[2],
            json!({
                "type": "setup",
                "session": 1,
                "at": "2026-10-13T10:08:56Z",
                "elapsed_seconds": 0,
                "depth": 0,
                "command": "npm ci",
                "status": {"exited": 0},
                "duration_seconds": 12.4,
                "output": "added 3 packages\nin 12s\n",
            })
        );
        assert_eq!(
            lines[lines.len() - 2..],
            [
                json!({"type": "session", "session": 2, "started_at": "2026-10-13T10:20:00Z"}),
                json!({
                    "type": "setup",
                    "session": 2,
                    "at": "2026-10-13T10:20:00Z",
                    "elapsed_seconds": 0,
                    "depth": 0,
                    "command": "npm ci",
                    "status": {"exited": 1},
                    "duration_seconds": 3.0,
                    "output": "npm ERR! missing lockfile\n",
                }),
            ]
        );
    }
    let timed_out = setting_up(
        "2026-10-13T10:00:00Z",
        "sleep 9",
        Some((SetupStatus::TimedOut, 1.0)),
        "",
    );
    let timed_out = transcript::json_lines(&of(&[timed_out]), Verbosity::Compact);
    let line = timed_out.lines().nth(2).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(line).unwrap()["status"],
        "timed-out"
    );
    assert_eq!(
        transcript::render_line(
            &serde_json::from_str::<transcript::Line>(line).unwrap(),
            Verbosity::Compact,
            false
        ),
        " 0:00  setup: sleep 9 → timed out, 1s\n"
    );
}

/// `docs/specs/design_system.md`, Transcripts, Followed.
#[test]
fn following_shows_a_setup_command_as_it_starts_and_again_at_its_end_with_its_account() {
    let [succeeded, failed] = after_setting_up();
    let started = succeeded.output.find('\n').unwrap() + 1;
    let ended = started + succeeded.output[started..].find('\n').unwrap() + 1;
    let running = SessionRecord {
        setup_output: None,
        ..written(&succeeded, started)
    };
    let follow = |after| {
        Follower::new(
            item_run("105/implement-1".parse().unwrap()),
            Verbosity::Compact,
            after,
        )
    };
    let mut follower = follow(None);

    let before = follower.update(&[written(&running, 0)]);
    let once_started = follower.update(std::slice::from_ref(&running));
    let once_ended = follower.update(&[written(&succeeded, ended)]);
    let mut rest = follower.update(std::slice::from_ref(&succeeded));
    rest.extend(follower.end(&[succeeded.clone(), failed.clone()]));
    let all = [
        before.clone(),
        once_started.clone(),
        once_ended,
        rest.clone(),
    ]
    .concat();

    assert_eq!(text(&before), "run: 105/implement-1\n");
    assert_eq!(
        text(&once_started),
        "session 1: started 2026-10-13 10:08:56 UTC\n 0:00  setup: npm ci → running\n"
    );
    assert_eq!(text(&all[3..4]), " 0:12  setup: npm ci → exit 0, 12s\n");
    assert!(
        text(&all[4..5]).starts_with(" 0:15  Bash(ls;"),
        "{}",
        text(&all)
    );
    assert_eq!(
        text(&rest[rest.len() - 3..]),
        "session 2: started 2026-10-13 10:20:00 UTC\n \
         0:00  setup: npm ci → running\n \
         0:03  setup: npm ci → exit 1, 3s\n         \
         npm ERR! missing lockfile\n"
    );
    // Read at once, it shows the same lines, made at the same points; the
    // run's header counts the sessions it was first read with.
    let at_once = follow(None).end(&[succeeded.clone(), failed.clone()]);
    assert_eq!(all[1..], at_once[1..]);
    // From a line, or from where a transcript was read to, it shows the
    // lines after it.
    let after_running =
        follow(Some(After::Line(all[2].made))).end(&[succeeded.clone(), failed.clone()]);
    assert_eq!(after_running, all[3..]);
    let after_read = follow(Some(After::Read(all[2].made.read))).end(&[succeeded, failed]);
    assert_eq!(after_read, all[3..]);
}

/// A session recorded before a setup command's start and end were: its
/// record holds `setup.json`, written once the command ended.
fn recorded_before_setup_events(status: SetupStatus, output: &str) -> SessionRecord {
    SessionRecord {
        setup: Some(SetupRecord {
            command: "npm ci".into(),
            status,
            duration_seconds: 12.4,
        }),
        setup_output: Some(output.into()),
        ..SessionRecord::default()
    }
}

#[test]
fn a_setup_command_recorded_before_its_start_and_end_were_shows_with_no_time() {
    let succeeded = SessionRecord {
        output: recorded().output,
        ..recorded_before_setup_events(SetupStatus::Exited(0), "added 3 packages\n")
    };
    let failed =
        recorded_before_setup_events(SetupStatus::Exited(1), "npm ERR! missing lockfile\n");

    let shown = transcript::render(
        &of(&[succeeded.clone(), failed.clone()]),
        Verbosity::Compact,
        false,
    );
    let followed = text(
        &Follower::new(
            item_run("105/implement-1".parse().unwrap()),
            Verbosity::Compact,
            None,
        )
        .end(&[succeeded, failed]),
    );

    let shown: Vec<&str> = shown.lines().collect();
    assert_eq!(
        shown[1..4],
        [
            "session 1: started 2026-10-13 10:09:08 UTC",
            "       setup: npm ci → exit 0, 12s",
            " 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit 0, 19 lines",
        ]
    );
    assert_eq!(
        shown[shown.len() - 3..],
        [
            "session 2",
            "       setup: npm ci → exit 1, 12s",
            "         npm ERR! missing lockfile",
        ]
    );
    let followed: Vec<&str> = followed.lines().collect();
    assert_eq!(followed[1..3], shown[1..3]);
    assert_eq!(followed[followed.len() - 3..], shown[shown.len() - 3..]);
}

/// `docs/specs/design_system.md`, Transcripts, Followed: a setup command's
/// output shows once it has ended, however its end is read.
#[test]
fn a_setup_commands_output_is_read_once_its_end_is_even_when_a_read_stops_inside_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join(transcript::OUTPUT);
    let ended =
        transcript::setup_ended(time("2026-10-13T10:00:03Z"), SetupStatus::Exited(1), 3.0) + "\n";
    std::fs::write(dir.path().join(transcript::SETUP_OUTPUT), "npm ERR!\n").unwrap();
    std::fs::write(
        &output,
        transcript::setup_started(time("2026-10-13T10:00:00Z"), "npm ci") + "\n" + &ended[..5],
    )
    .unwrap();
    let mut record = SessionRecord::read(dir.path()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&output)
        .unwrap();
    std::io::Write::write_all(&mut file, &ended.as_bytes()[5..]).unwrap();

    record.read_more(dir.path()).unwrap();

    assert_eq!(record.setup_output.as_deref(), Some("npm ERR!\n"));
    assert!(
        transcript::render(&of(&[record]), Verbosity::Compact, false)
            .ends_with(" 0:00  setup: npm ci → exit 1, 3s\n         npm ERR!\n")
    );
}
