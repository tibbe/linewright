use linewright::domain::Timestamp;
use linewright::runs::{RunId, Started, StartedBy};
use linewright::transcript::{
    self, After, End, EntryAt, Follower, Lasted, MaintenanceRecord, MaintenanceStatus, Place,
    SessionRecord, Transcript, Verbosity, Wait, Written,
};
use serde_json::json;

use crate::common::{json_objects, output_of};

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
        maintenance_output: None,
        maintenance: None,
    }
}

fn item_run(id: RunId) -> Started {
    Started::new(id, None).unwrap()
}

/// The transcript of an ended run whose sessions are `sessions`.
fn of(sessions: &[SessionRecord]) -> Transcript {
    Transcript {
        run: item_run("105/implement-1".parse().unwrap()),
        in_flight: None,
        sessions: sessions
            .iter()
            .map(|record| transcript::parse(record, Written::Whole))
            .collect(),
    }
}

/// The transcript of a run in flight whose only session is `record`, read to
/// its end.
fn in_flight(record: &SessionRecord) -> Transcript {
    Transcript {
        run: item_run("105/implement-1".parse().unwrap()),
        in_flight: Some(transcript::InFlight::ReadTo(transcript::ReadTo {
            session: 1,
            line: record.output.lines().count(),
        })),
        sessions: vec![transcript::parse(record, Written::SoFar)],
    }
}

#[test_log::test]
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

#[test_log::test]
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
    assert_eq!(events(r#""subtype":"init""#), 2);
    assert_eq!(events(r#""type":"result""#), 2);
    assert_eq!(events(r#""subtype":"task_notification""#), 1);
    assert!(
        shown.contains(" 0:27  Read(Dockerfile) → 16 lines\n         input: {\n"),
        "{shown}"
    );
    assert!(shown.contains("         details: {\n"), "{shown}");
}

#[test_log::test]
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

#[test_log::test]
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

/// `docs/specs/monitoring.md`, logs, Running in JSON.
#[test_log::test]
fn json_gives_a_tool_call_null_for_what_it_holds_once_it_returns() {
    let record = SessionRecord {
        output: [
            json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
            json!({
                "type": "assistant",
                "timestamp": "2026-10-13T10:00:02Z",
                "message": {"content": [
                    {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a.txt"}},
                    {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "cargo test"}},
                ]},
            }),
            json!({
                "type": "user",
                "timestamp": "2026-10-13T10:00:03Z",
                "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "one\ntwo"},
                ]},
            }),
        ]
        .iter()
        .map(|event| format!("{event}\n"))
        .collect(),
        ..SessionRecord::default()
    };
    let json = |verbosity| json_objects(&transcript::json_lines(&in_flight(&record), verbosity));
    let with_stamp = |line| {
        with_keys(
            line,
            json!({"session": 1, "at": "2026-10-13T10:00:02Z", "elapsed_seconds": 2, "depth": 0}),
        )
    };

    assert_eq!(
        json(Verbosity::Compact)[2..],
        [
            with_stamp(json!({
                "type": "tool",
                "tool_use_id": "t1",
                "tool": "Read",
                "argument": "a.txt",
                "account": "2 lines",
                "error": false,
            })),
            with_stamp(json!({
                "type": "tool",
                "tool_use_id": "t2",
                "tool": "Bash",
                "argument": "cargo test",
                "account": "running",
                "error": null,
            })),
        ]
    );
    assert_eq!(
        json(Verbosity::Full)[2..],
        [
            with_stamp(json!({
                "type": "tool",
                "tool_use_id": "t1",
                "tool": "Read",
                "argument": "a.txt",
                "account": "2 lines",
                "error": false,
                "input": {"file_path": "a.txt"},
                "result": "one\ntwo",
                "content": null,
                "details": null,
            })),
            with_stamp(json!({
                "type": "tool",
                "tool_use_id": "t2",
                "tool": "Bash",
                "argument": "cargo test",
                "account": "running",
                "error": null,
                "input": {"command": "cargo test"},
                "result": null,
                "content": null,
                "details": null,
            })),
        ]
    );
}

/// `object` with the keys of `more` too.
fn with_keys(object: serde_json::Value, more: serde_json::Value) -> serde_json::Value {
    let (serde_json::Value::Object(mut object), serde_json::Value::Object(more)) = (object, more)
    else {
        panic!("only objects have keys");
    };
    object.extend(more);
    serde_json::Value::Object(object)
}

/// `record` as written up to the agent's first tool call, its result still
/// to come.
fn first_call_made(record: &SessionRecord) -> SessionRecord {
    let called = record.output.find(r#""type": "tool_use""#).unwrap();
    written(
        record,
        called + record.output[called..].find('\n').unwrap() + 1,
    )
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

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
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
        " 0:02  Bash(cargo test) → interrupted\n\
         session 2: started 2026-10-13 11:00:00 UTC\n \
         0:05  Starting over.\n"
    );
    assert_eq!(text(&nothing.update(&[])), "");
    assert_eq!(
        text(&nothing.end(&[])),
        "run: 105/update-1 (it started no session)\n"
    );
}

#[test_log::test]
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

#[test_log::test]
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
    let parallel_calls_with_a_nested_one_returning_out_of_order = [
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
        output: parallel_calls_with_a_nested_one_returning_out_of_order
            .iter()
            .map(|event| format!("{event}\n"))
            .collect(),
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

#[test_log::test]
fn an_entry_that_says_no_time_shows_the_time_of_the_one_the_agent_wrote_before_it() {
    let shown = transcript::render(&of(&[recorded()]), Verbosity::Full, false);
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Full,
        None,
    );
    let followed = text(&follower.end(&[recorded()]));

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

/// The record of a session whose maintenance command `command` started at
/// `start`, then, if it has, ended as `ended` gives, `(status, seconds)`,
/// having written `output`.
fn setting_up(
    start: &str,
    command: &str,
    ended: Option<(MaintenanceStatus, f64)>,
    output: &str,
) -> SessionRecord {
    let start = time(start);
    let mut events = transcript::maintenance_started(start, command) + "\n";
    let mut maintenance_output = None;
    if let Some((status, seconds)) = ended {
        let at = start
            + chrono::TimeDelta::from_std(std::time::Duration::from_secs_f64(seconds)).unwrap();
        events += &(transcript::maintenance_ended(at, status, seconds) + "\n");
        maintenance_output = Some(output.to_string());
    }
    SessionRecord {
        output: events,
        maintenance_output,
        ..SessionRecord::default()
    }
}

/// The recorded session after a maintenance command that succeeded, then one
/// whose maintenance command failed, recorded before its agent's first turn.
fn after_setting_up() -> [SessionRecord; 2] {
    let set_up = setting_up(
        "2026-10-13T10:08:56Z",
        "npm ci",
        Some((MaintenanceStatus::Exited(0), 12.4)),
        "added 3 packages\nin 12s\n",
    );
    [
        SessionRecord {
            output: set_up.output.clone() + &recorded().output,
            ..SessionRecord {
                maintenance_output: set_up.maintenance_output,
                ..recorded()
            }
        },
        setting_up(
            "2026-10-13T10:20:00Z",
            "npm ci",
            Some((MaintenanceStatus::Exited(1), 3.0)),
            "npm ERR! missing lockfile\n",
        ),
    ]
}

/// `docs/specs/design_system.md`, Transcripts, Times and Maintenance.
#[test_log::test]
fn a_maintenance_command_is_its_sessions_first_entry_at_its_start_with_its_output_only_in_full_or_when_it_failed()
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
            " 0:00  maintenance: npm ci → exit 0, 12s",
        ]
    );
    assert!(compact[3].starts_with(" 0:15  Bash(ls;"), "{compact:#?}");
    assert_eq!(
        compact[compact.len() - 3..],
        [
            "session 2: started 2026-10-13 10:20:00 UTC",
            " 0:00  maintenance: npm ci → exit 1, 3s",
            "         npm ERR! missing lockfile",
        ]
    );
    let full: Vec<&str> = full.lines().collect();
    assert_eq!(
        full[2..5],
        [
            " 0:00  maintenance: npm ci → exit 0, 12s",
            "         added 3 packages",
            "         in 12s",
        ]
    );
    assert!(full[5].starts_with(" 0:12  brief: "), "{full:#?}");
}

#[test_log::test]
fn a_maintenance_command_shows_how_it_ended_and_its_output_unless_it_succeeded_or_was_cancelled() {
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
        shown(MaintenanceStatus::Signalled(9)),
        " 0:00  maintenance: make deps → signal 9, 1m 1s\n         fetching"
    );
    assert_eq!(
        shown(MaintenanceStatus::TimedOut),
        " 0:00  maintenance: make deps → timed out, 1m 1s\n         fetching"
    );
    assert_eq!(
        shown(MaintenanceStatus::Cancelled),
        " 0:00  maintenance: make deps → cancelled, 1m 1s"
    );
}

/// `docs/specs/design_system.md`, Transcripts, Running.
#[test_log::test]
fn a_maintenance_command_shows_running_until_it_ends() {
    let running = setting_up("2026-10-13T10:00:00Z", "npm ci", None, "");
    let transcript = in_flight(&running);

    let shown = transcript::render(&transcript, Verbosity::Full, false);

    assert_eq!(
        shown,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:00  maintenance: npm ci → running\n"
    );
}

/// `docs/specs/monitoring.md`, logs, Maintenance.
#[test_log::test]
fn json_gives_a_maintenance_command_with_its_account_and_output_null_until_it_has_ended() {
    let running = setting_up("2026-10-13T10:00:00Z", "npm ci", None, "");
    let [succeeded, failed] = after_setting_up();

    let json = |records: &[SessionRecord], verbosity| -> Vec<serde_json::Value> {
        transcript::json_lines(&of(records), verbosity)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };

    assert_eq!(
        json_objects(&transcript::json_lines(
            &in_flight(&running),
            Verbosity::Compact
        ))[2],
        json!({
            "type": "maintenance",
            "session": 1,
            "at": "2026-10-13T10:00:00Z",
            "elapsed_seconds": 0,
            "depth": 0,
            "command": "npm ci",
            "status": null,
            "duration_seconds": null,
            "output": null,
        })
    );
    for verbosity in [Verbosity::Compact, Verbosity::Full] {
        let lines = json(&[succeeded.clone(), failed.clone()], verbosity);
        assert_eq!(lines[1]["type"], "session");
        assert_eq!(
            lines[2],
            json!({
                "type": "maintenance",
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
                    "type": "maintenance",
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
        Some((MaintenanceStatus::TimedOut, 1.0)),
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
        " 0:00  maintenance: sleep 9 → timed out, 1s\n"
    );
}

/// `docs/specs/monitoring.md`, logs, Following in JSON.
#[test_log::test]
fn following_gives_a_tool_call_as_the_object_it_has_running_then_the_one_it_has_ended() {
    let recorded = recorded();
    let running = first_call_made(&recorded);
    let snapshot = |record: &SessionRecord| -> Vec<serde_json::Value> {
        json_objects(&transcript::json_lines(&in_flight(record), Verbosity::Full))
            .into_iter()
            .filter(|line| line["type"] == "tool")
            .take(1)
            .collect()
    };
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Full,
        None,
    );

    let mut followed = json(&follower.update(std::slice::from_ref(&running)));
    followed.extend(json(&follower.update(std::slice::from_ref(&recorded))));
    let first_call =
        followed.iter().find(|line| line["type"] == "tool").unwrap()["tool_use_id"].clone();
    let objects: Vec<serde_json::Value> = followed
        .into_iter()
        .filter(|line| line["tool_use_id"] == first_call)
        .collect();

    assert_eq!(objects.len(), 2, "{objects:#?}");
    assert_eq!(objects[0]["account"], "running");
    assert_eq!(objects[..1], snapshot(&running));
    let unstamped = |line: &serde_json::Value| {
        let mut line = line.clone();
        let fields = line.as_object_mut().unwrap();
        fields.remove("at");
        fields.remove("elapsed_seconds");
        line
    };
    assert_eq!(unstamped(&objects[1]), unstamped(&snapshot(&recorded)[0]));
}

/// `docs/specs/monitoring.md`, logs, Running in JSON.
#[test_log::test]
fn a_maintenance_line_ended_in_part_does_not_read() {
    let line = |ended| {
        serde_json::from_value::<transcript::Line>(with_keys(
            json!({
                "type": "maintenance",
                "session": 1,
                "at": "2026-10-13T10:00:00Z",
                "elapsed_seconds": 0,
                "depth": 0,
                "command": "npm ci",
            }),
            ended,
        ))
    };

    assert!(line(json!({"status": null, "duration_seconds": null, "output": null})).is_ok());
    assert!(line(json!({"status": {"exited": 0}, "duration_seconds": 1.0, "output": ""})).is_ok());
    for part in [
        json!({"status": {"exited": 0}, "duration_seconds": null, "output": null}),
        json!({"status": {"exited": 0}, "duration_seconds": 1.0, "output": null}),
        json!({"status": null, "duration_seconds": 1.0, "output": "added 3 packages\n"}),
    ] {
        assert!(line(part.clone()).is_err(), "{part}");
    }
}

/// `docs/specs/design_system.md`, Transcripts, Followed.
#[test_log::test]
fn following_shows_a_maintenance_command_as_it_starts_and_again_at_its_end_with_its_account() {
    let [succeeded, failed] = after_setting_up();
    let started = succeeded.output.find('\n').unwrap() + 1;
    let ended = started + succeeded.output[started..].find('\n').unwrap() + 1;
    let running = SessionRecord {
        maintenance_output: None,
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
        "session 1: started 2026-10-13 10:08:56 UTC\n 0:00  maintenance: npm ci → running\n"
    );
    assert_eq!(
        text(&all[3..4]),
        " 0:12  maintenance: npm ci → exit 0, 12s\n"
    );
    assert!(
        text(&all[4..5]).starts_with(" 0:15  Bash(ls;"),
        "{}",
        text(&all)
    );
    assert_eq!(
        text(&rest[rest.len() - 3..]),
        "session 2: started 2026-10-13 10:20:00 UTC\n \
         0:00  maintenance: npm ci → running\n \
         0:03  maintenance: npm ci → exit 1, 3s\n         \
         npm ERR! missing lockfile\n"
    );
    let at_once = follow(None).end(&[succeeded.clone(), failed.clone()]);
    assert_eq!(all[1..], at_once[1..]);
    let after_running =
        follow(Some(After::Line(all[2].made))).end(&[succeeded.clone(), failed.clone()]);
    assert_eq!(after_running, all[3..]);
    let after_read = follow(Some(After::Read(all[2].made.read))).end(&[succeeded, failed]);
    assert_eq!(after_read, all[3..]);
}

/// A session recorded before a maintenance command's start and end were: its
/// record holds `setup.json`, written once the command ended.
fn recorded_before_maintenance_events(status: MaintenanceStatus, output: &str) -> SessionRecord {
    SessionRecord {
        maintenance: Some(MaintenanceRecord {
            command: "npm ci".into(),
            status,
            duration_seconds: 12.4,
        }),
        maintenance_output: Some(output.into()),
        ..SessionRecord::default()
    }
}

#[test_log::test]
fn a_maintenance_command_recorded_before_its_start_and_end_were_shows_with_no_time() {
    let succeeded = SessionRecord {
        output: recorded().output,
        ..recorded_before_maintenance_events(MaintenanceStatus::Exited(0), "added 3 packages\n")
    };
    let failed = recorded_before_maintenance_events(
        MaintenanceStatus::Exited(1),
        "npm ERR! missing lockfile\n",
    );

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
            "       maintenance: npm ci → exit 0, 12s",
            " 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit 0, 19 lines",
        ]
    );
    assert_eq!(
        shown[shown.len() - 3..],
        [
            "session 2",
            "       maintenance: npm ci → exit 1, 12s",
            "         npm ERR! missing lockfile",
        ]
    );
    let followed: Vec<&str> = followed.lines().collect();
    assert_eq!(followed[1..3], shown[1..3]);
    assert_eq!(followed[followed.len() - 3..], shown[shown.len() - 3..]);
}

/// `docs/specs/design_system.md`, Transcripts, Followed: a maintenance command's
/// output shows once it has ended, however its end is read.
#[test_log::test]
fn a_maintenance_commands_output_is_read_once_its_end_is_even_when_a_read_stops_inside_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join(transcript::OUTPUT);
    let ended = transcript::maintenance_ended(
        time("2026-10-13T10:00:03Z"),
        MaintenanceStatus::Exited(1),
        3.0,
    ) + "\n";
    std::fs::write(
        dir.path().join(transcript::MAINTENANCE_OUTPUT),
        "npm ERR!\n",
    )
    .unwrap();
    std::fs::write(
        &output,
        transcript::maintenance_started(time("2026-10-13T10:00:00Z"), "npm ci")
            + "\n"
            + &ended[..5],
    )
    .unwrap();
    let mut record = SessionRecord::read(dir.path()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&output)
        .unwrap();
    std::io::Write::write_all(&mut file, &ended.as_bytes()[5..]).unwrap();

    record.read_more(dir.path()).unwrap();

    assert_eq!(record.maintenance_output.as_deref(), Some("npm ERR!\n"));
    assert!(
        transcript::render(&of(&[record]), Verbosity::Compact, false)
            .ends_with(" 0:00  maintenance: npm ci → exit 1, 3s\n         npm ERR!\n")
    );
}

/// The output of a session that starts a sub-agent which runs for 3h 2m 9s
/// and makes 3 tool calls, then answers.
fn with_a_sub_agent_of_3h_2m_9s() -> String {
    let message = |kind: &str, at: &str, content: serde_json::Value| json!({"type": kind, "timestamp": at, "message": {"content": content}});
    let mut returned = message(
        "user",
        "2026-10-13T13:02:11Z",
        json!([{"type": "tool_result", "tool_use_id": "a1", "content": "Clean."}]),
    );
    returned["tool_use_result"] = json!({
        "status": "completed",
        "totalDurationMs": 10_929_400,
        "totalToolUseCount": 3,
    });
    output_of(&[
        json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
        message(
            "assistant",
            "2026-10-13T10:00:02Z",
            json!([{"type": "tool_use", "id": "a1", "name": "Agent", "input": {"description": "Review"}}]),
        ),
        returned,
        message(
            "assistant",
            "2026-10-13T13:02:15Z",
            json!([{"type": "text", "text": "The review came back clean."}]),
        ),
    ])
}

/// `docs/specs/design_system.md`, Transcripts; Time, Durations.
#[test_log::test]
fn a_sub_agent_that_ran_for_hours_shows_how_long_as_a_duration_and_each_line_its_time_past_the_hour()
 {
    let record = SessionRecord {
        output: with_a_sub_agent_of_3h_2m_9s(),
        ..SessionRecord::default()
    };

    let shown = transcript::render(&of(&[record]), Verbosity::Compact, false);

    assert_eq!(
        shown,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:02  Agent(Review) → done, 3 tool uses, 3h 2m\n\
         3:02:15  The review came back clean.\n"
    );
}

fn automations_run(n: u32, by: StartedBy) -> Transcript {
    Transcript {
        run: Started::new(format!("hourly/{n}").parse().unwrap(), Some(by)).unwrap(),
        ..of(&[recorded()])
    }
}

#[test_log::test]
fn an_automations_run_by_hand_says_so_in_its_header_and_a_scheduled_one_does_not() {
    let by_hand = automations_run(2, StartedBy::Hand);
    let scheduled = automations_run(1, StartedBy::Schedule);
    let header = |transcript: &Transcript| {
        let shown = transcript::render(transcript, Verbosity::Compact, false);
        let json = transcript::json_lines(transcript, Verbosity::Compact);
        (
            shown.lines().next().unwrap().to_string(),
            serde_json::from_str::<serde_json::Value>(json.lines().next().unwrap()).unwrap(),
        )
    };

    assert_eq!(
        [header(&by_hand), header(&scheduled)],
        [
            (
                "run: hourly/2, by hand".to_string(),
                json!({"type": "run", "run": "hourly/2", "started_by": "hand", "sessions": 1}),
            ),
            (
                "run: hourly/1".to_string(),
                json!({"type": "run", "run": "hourly/1", "started_by": "schedule", "sessions": 1}),
            ),
        ]
    );
}

#[test_log::test]
fn a_run_that_started_no_session_says_so() {
    let transcript = Transcript {
        run: item_run("2/update-1".parse().unwrap()),
        ..of(&[])
    };

    assert_eq!(
        transcript::render(&transcript, Verbosity::Compact, false),
        "run: 2/update-1 (it started no session)\n"
    );
    assert_eq!(
        transcript::json_lines(&transcript, Verbosity::Compact),
        "{\"type\":\"run\",\"run\":\"2/update-1\",\"sessions\":0}\n"
    );
}

#[test_log::test]
fn following_a_session_yet_to_write_anything_shows_the_runs_header_once() {
    let record = recorded();
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );
    let third_line = record.output.match_indices('\n').nth(2).unwrap().0 + 1;

    let before = text(&follower.update(&[SessionRecord::default()]));
    let still = text(&follower.update(&[SessionRecord::default()]));
    let written = text(&follower.update(&[written(&record, third_line)]));

    assert_eq!(
        [before, still, written],
        [
            "run: 105/implement-1\n".to_string(),
            String::new(),
            "session 1: started 2026-10-13 10:09:08 UTC\n \
             0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → running\n"
                .to_string(),
        ]
    );
}

/// The lines of a session's clone that started being made at `start`, then,
/// if it has, ended `seconds` later with `error`, as `ended` gives them.
#[track_caller]
fn cloning(start: &str, ended: Option<(f64, Option<&str>)>) -> String {
    let start = time(start);
    let mut events = transcript::clone_started(start) + "\n";
    if let Some((seconds, error)) = ended {
        let at = start
            + chrono::TimeDelta::from_std(std::time::Duration::from_secs_f64(seconds)).unwrap();
        events += &(transcript::clone_ended(at, seconds, error) + "\n");
    }
    events
}

/// The recorded session after its clone was made in 6.2 seconds and its
/// maintenance command succeeded, then a session whose clone could not be
/// made.
fn after_cloning() -> [SessionRecord; 2] {
    let [set_up, _] = after_setting_up();
    [
        SessionRecord {
            output: cloning("2026-10-13T10:08:50Z", Some((6.2, None))) + &set_up.output,
            ..set_up
        },
        SessionRecord {
            output: cloning(
                "2026-10-13T10:20:00Z",
                Some((
                    3.0,
                    Some("fetching origin: fatal: could not read from remote"),
                )),
            ),
            ..SessionRecord::default()
        },
    ]
}

/// `docs/specs/design_system.md`, Transcripts, The clone.
#[test_log::test]
fn the_clone_is_its_sessions_first_entry_at_its_start_with_how_long_it_took_and_its_error_if_it_failed()
 {
    let transcript = of(&after_cloning());

    let shown = transcript::render(&transcript, Verbosity::Compact, false);

    let shown: Vec<&str> = shown.lines().collect();
    assert_eq!(
        shown[..4],
        [
            "run: 105/implement-1",
            "session 1: started 2026-10-13 10:08:50 UTC",
            " 0:00  clone → 6s",
            " 0:06  maintenance: npm ci → exit 0, 12s",
        ]
    );
    assert_eq!(
        shown[shown.len() - 3..],
        [
            "session 2: started 2026-10-13 10:20:00 UTC",
            " 0:00  clone → 3s",
            "         fetching origin: fatal: could not read from remote",
        ]
    );
    let coloured = transcript::render(&of(&after_cloning()[1..]), Verbosity::Compact, true);
    assert!(
        coloured.contains("\u{1b}[31mfetching origin: fatal: could not read from remote"),
        "{coloured:?}"
    );
}

/// `docs/specs/design_system.md`, Transcripts, Running.
#[test_log::test]
fn the_clone_shows_running_until_it_is_made() {
    let running = SessionRecord {
        output: cloning("2026-10-13T10:00:00Z", None),
        ..SessionRecord::default()
    };
    let transcript = in_flight(&running);

    let shown = transcript::render(&transcript, Verbosity::Full, false);

    assert_eq!(
        shown,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:00  clone → running\n"
    );
}

/// `docs/specs/monitoring.md`, logs, The clone and Running in JSON.
#[test_log::test]
fn json_gives_the_clone_with_how_long_it_took_and_its_error_null_until_it_is_made() {
    let running = SessionRecord {
        output: cloning("2026-10-13T10:00:00Z", None),
        ..SessionRecord::default()
    };
    let clone = |seconds: serde_json::Value, error: serde_json::Value, session, at| {
        json!({
            "type": "clone",
            "session": session,
            "at": at,
            "elapsed_seconds": 0,
            "depth": 0,
            "duration_seconds": seconds,
            "error": error,
        })
    };

    let running = json_objects(&transcript::json_lines(
        &in_flight(&running),
        Verbosity::Compact,
    ));
    let made = json_objects(&transcript::json_lines(
        &of(&after_cloning()),
        Verbosity::Full,
    ));

    assert_eq!(
        running[2],
        clone(json!(null), json!(null), 1, "2026-10-13T10:00:00Z")
    );
    assert_eq!(
        made[2],
        clone(json!(6.2), json!(null), 1, "2026-10-13T10:08:50Z")
    );
    assert_eq!(made[3]["type"], "maintenance");
    assert_eq!(
        made[made.len() - 1],
        clone(
            json!(3.0),
            json!("fetching origin: fatal: could not read from remote"),
            2,
            "2026-10-13T10:20:00Z"
        )
    );
    let line = serde_json::from_value::<transcript::Line>(made[made.len() - 1].clone()).unwrap();
    assert_eq!(
        transcript::render_line(&line, Verbosity::Compact, false),
        " 0:00  clone → 3s\n         fetching origin: fatal: could not read from remote\n"
    );
    let error_alone = with_keys(
        clone(json!(null), json!("cancelled"), 1, "2026-10-13T10:00:00Z"),
        json!({}),
    );
    assert!(serde_json::from_value::<transcript::Line>(error_alone).is_err());
}

/// `docs/specs/monitoring.md`, logs, Following in JSON.
#[test_log::test]
fn following_gives_the_clone_as_it_starts_and_again_once_it_ends() {
    let [made, failed] = after_cloning();
    let started = made.output.find('\n').unwrap() + 1;
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );

    let once_started = follower.update(&[written(&made, started)]);
    let mut rest = follower.update(std::slice::from_ref(&made));
    rest.extend(follower.end(&[made, failed]));

    assert_eq!(
        text(&once_started),
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:08:50 UTC\n \
         0:00  clone → running\n"
    );
    let objects = json(&[once_started, rest.clone()].concat());
    let clones: Vec<&serde_json::Value> = objects
        .iter()
        .filter(|line| line["type"] == "clone" && line["session"] == 1)
        .collect();
    assert_eq!(clones.len(), 2, "{objects:#?}");
    assert_eq!(clones[0]["duration_seconds"], json!(null));
    assert_eq!(clones[1]["duration_seconds"], json!(6.2));
    assert_eq!(clones[1]["elapsed_seconds"], json!(6));
    assert_eq!(
        text(&rest[..2]),
        " 0:06  clone → 6s\n 0:06  maintenance: npm ci → running\n"
    );
    assert_eq!(
        text(&rest[rest.len() - 3..]),
        "session 2: started 2026-10-13 10:20:00 UTC\n \
         0:00  clone → running\n \
         0:03  clone → 3s\n         \
         fetching origin: fatal: could not read from remote\n"
    );
}

/// `docs/specs/design_system.md`, Transcripts: a session recorded before its
/// clone was an entry starts with its maintenance command, as it did.
#[test_log::test]
fn a_session_recorded_before_its_clone_was_an_entry_reads_as_before() {
    let [set_up, _] = after_setting_up();

    let shown = transcript::render(&of(&[set_up]), Verbosity::Compact, false);

    let shown: Vec<&str> = shown.lines().collect();
    assert_eq!(
        shown[1..3],
        [
            "session 1: started 2026-10-13 10:08:56 UTC",
            " 0:00  maintenance: npm ci → exit 0, 12s",
        ]
    );
}

/// A session that started at 10:00:00 with a turn whose `Bash` call `t1`,
/// made at 10:00:02, is followed by `events`.
fn calling(events: &[serde_json::Value]) -> SessionRecord {
    let mut all = vec![
        json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
        json!({
            "type": "assistant",
            "timestamp": "2026-10-13T10:00:02Z",
            "message": {"content": [
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}},
            ]},
        }),
    ];
    all.extend_from_slice(events);
    SessionRecord {
        output: output_of(&all),
        ..SessionRecord::default()
    }
}

#[track_caller]
fn session_ended(at: &str) -> serde_json::Value {
    serde_json::from_str(&transcript::session_ended(time(at))).unwrap()
}

/// `docs/specs/design_system.md`, Transcripts, Running.
#[test_log::test]
fn a_tool_call_its_session_ends_first_shows_interrupted_where_it_showed_running() {
    let known = calling(&[session_ended("2026-10-13T10:00:09Z")]);
    let unknown = calling(&[]);

    for record in [known, unknown] {
        let shown = transcript::render(&of(&[record]), Verbosity::Compact, false);

        assert_eq!(
            shown,
            "run: 105/implement-1\n\
             session 1: started 2026-10-13 10:00:00 UTC\n \
             0:02  Bash(cargo test) → interrupted\n"
        );
    }
}

fn returned(at: &str, result: &serde_json::Value) -> serde_json::Value {
    json!({
        "type": "user",
        "timestamp": at,
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "Done."},
        ]},
        "tool_use_result": result.clone(),
    })
}

/// How long the call `t1` that `record` makes lasted, as much of it
/// `written`.
#[track_caller]
fn call_lasted(record: &SessionRecord, written: Written) -> Option<Lasted> {
    let first = Place::Entry(EntryAt {
        turn: 0,
        path: vec![0],
    });
    transcript::parse(record, written).lasted(&first)
}

/// `docs/specs/design_system.md`, Transcripts, Running and Followed.
#[test_log::test]
fn a_tool_call_runs_until_it_returns_or_ends_in_the_background_or_its_session_ends() {
    let returned_at_5 = calling(&[returned("2026-10-13T10:00:05Z", &json!({}))]);
    let moved = returned("2026-10-13T10:00:03Z", &json!({"backgroundTaskId": "b1"}));
    let in_the_background = calling(std::slice::from_ref(&moved));
    let ended_in_the_background = calling(&[
        moved,
        json!({"type": "system", "subtype": "task_notification", "timestamp": "2026-10-13T10:00:12Z", "tool_use_id": "t1", "status": "completed"}),
    ]);
    let ended_at_9 = calling(&[session_ended("2026-10-13T10:00:09Z")]);
    let made = calling(&[]);

    assert_eq!(
        call_lasted(&returned_at_5, Written::Whole),
        Some(Lasted::Took { seconds: Some(3.0) })
    );
    assert_eq!(
        call_lasted(&in_the_background, Written::SoFar),
        Some(Lasted::Running {
            since: Some(time("2026-10-13T10:00:02Z"))
        })
    );
    assert_eq!(
        call_lasted(&ended_in_the_background, Written::Whole),
        Some(Lasted::Took {
            seconds: Some(10.0)
        })
    );
    assert_eq!(
        call_lasted(&ended_at_9, Written::SoFar),
        Some(Lasted::Interrupted { seconds: Some(7.0) })
    );
    assert_eq!(
        call_lasted(&made, Written::SoFar),
        Some(Lasted::Running {
            since: Some(time("2026-10-13T10:00:02Z"))
        })
    );
    assert_eq!(
        call_lasted(&made, Written::Whole),
        Some(Lasted::Interrupted { seconds: None })
    );
    assert_eq!(
        call_lasted(&in_the_background, Written::Whole),
        Some(Lasted::Interrupted { seconds: None })
    );
}

/// `docs/specs/design_system.md`, Transcripts, Running.
#[test_log::test]
fn the_clone_and_the_maintenance_command_last_until_they_end_or_their_session_does() {
    let cloned = SessionRecord {
        output: cloning("2026-10-13T10:00:00Z", Some((6.2, None))),
        ..SessionRecord::default()
    };
    let cloning_cut = SessionRecord {
        output: cloning("2026-10-13T10:00:00Z", None)
            + &transcript::session_ended(time("2026-10-13T10:00:04Z"))
            + "\n",
        ..SessionRecord::default()
    };
    let maintaining = setting_up("2026-10-13T10:00:00Z", "npm ci", None, "");
    let maintained = setting_up(
        "2026-10-13T10:00:00Z",
        "npm ci",
        Some((MaintenanceStatus::Exited(0), 12.4)),
        "",
    );
    let lasted =
        |record: &SessionRecord, whole, place| transcript::parse(record, whole).lasted(&place);

    assert_eq!(
        lasted(&cloned, Written::Whole, Place::Clone),
        Some(Lasted::Took { seconds: Some(6.2) })
    );
    assert_eq!(
        lasted(&cloning_cut, Written::SoFar, Place::Clone),
        Some(Lasted::Interrupted { seconds: Some(4.0) })
    );
    assert_eq!(
        lasted(&maintaining, Written::SoFar, Place::Maintenance),
        Some(Lasted::Running {
            since: Some(time("2026-10-13T10:00:00Z"))
        })
    );
    assert_eq!(
        lasted(&maintaining, Written::Whole, Place::Maintenance),
        Some(Lasted::Interrupted { seconds: None })
    );
    assert_eq!(
        lasted(&maintained, Written::Whole, Place::Maintenance),
        Some(Lasted::Took {
            seconds: Some(12.4)
        })
    );
}

/// `docs/specs/design_system.md`, The run page, A session's heading and
/// Working.
#[test_log::test]
fn a_session_has_its_end_and_its_waits_for_the_usage_limit() {
    let line = |event: String| -> serde_json::Value { serde_json::from_str(&event).unwrap() };
    let waited = calling(&[
        line(transcript::usage_limit_waiting(
            time("2026-10-13T10:00:05Z"),
            time("2026-10-13T12:00:00Z"),
        )),
        line(transcript::usage_limit_waited(time("2026-10-13T12:00:01Z"))),
        line(transcript::usage_limit_waiting(
            time("2026-10-13T12:00:30Z"),
            time("2026-10-13T14:00:00Z"),
        )),
        session_ended("2026-10-13T12:01:00Z"),
    ]);

    let going = transcript::parse(&calling(&[]), Written::SoFar);
    let not_known = transcript::parse(&calling(&[]), Written::Whole);
    let ended = transcript::parse(&waited, Written::Whole);

    assert_eq!(going.end, End::Going);
    assert_eq!(going.waits, []);
    assert_eq!(not_known.end, End::NotKnown);
    assert_eq!(ended.end, End::At(time("2026-10-13T12:01:00Z")));
    assert_eq!(
        ended.waits,
        [
            Wait {
                at: time("2026-10-13T10:00:05Z"),
                until: time("2026-10-13T12:00:00Z"),
                end: End::At(time("2026-10-13T12:00:01Z")),
            },
            Wait {
                at: time("2026-10-13T12:00:30Z"),
                until: time("2026-10-13T14:00:00Z"),
                end: End::At(time("2026-10-13T12:01:00Z")),
            },
        ]
    );
    assert_eq!(
        transcript::render(&of(&[waited]), Verbosity::Full, false),
        transcript::render(
            &of(&[calling(&[session_ended("2026-10-13T12:01:00Z")])]),
            Verbosity::Full,
            false
        ),
        "a wait shows no line"
    );
}

/// `docs/specs/monitoring.md`, logs, Running in JSON.
#[test_log::test]
fn json_gives_an_interrupted_entry_the_account_interrupted_and_what_it_holds_as_if_running() {
    let call = calling(&[]);
    let clone = SessionRecord {
        output: cloning("2026-10-13T10:00:00Z", None),
        ..SessionRecord::default()
    };
    let maintenance = setting_up("2026-10-13T10:00:00Z", "npm ci", None, "");
    let entry =
        |record: &SessionRecord, transcript: fn(&SessionRecord) -> Transcript, verbosity| {
            let lines = transcript::json_lines(&transcript(record), verbosity);
            for line in lines.lines() {
                let read: transcript::Line = serde_json::from_str(line).unwrap();
                assert_eq!(transcript::json_line(&read), format!("{line}\n"));
            }
            json_objects(&lines).swap_remove(2)
        };
    let ended = |record: &SessionRecord| of(std::slice::from_ref(record));

    for verbosity in [Verbosity::Compact, Verbosity::Full] {
        assert_eq!(
            entry(&call, ended, verbosity),
            with_keys(
                entry(&call, in_flight, verbosity),
                json!({"account": "interrupted"})
            )
        );
    }
    assert_eq!(
        entry(&clone, ended, Verbosity::Compact),
        with_keys(
            entry(&clone, in_flight, Verbosity::Compact),
            json!({"account": "interrupted"})
        )
    );
    assert_eq!(
        entry(&maintenance, ended, Verbosity::Full),
        with_keys(
            entry(&maintenance, in_flight, Verbosity::Full),
            json!({"account": "interrupted"})
        )
    );
}

/// `docs/specs/design_system.md`, Transcripts, Running and Followed.
#[test_log::test]
fn following_shows_each_entry_still_running_again_as_interrupted_once_its_session_ends() {
    let maintaining = cloning("2026-10-13T10:00:00Z", Some((2.0, None)))
        + &transcript::maintenance_started(time("2026-10-13T10:00:02Z"), "npm ci")
        + "\n";
    let cut = SessionRecord {
        output: maintaining + &transcript::session_ended(time("2026-10-13T10:00:30Z")) + "\n",
        ..SessionRecord::default()
    };
    let cloning_only = SessionRecord {
        output: cloning("2026-10-13T11:00:00Z", None),
        ..SessionRecord::default()
    };
    let mut follower = Follower::new(
        item_run("105/implement-1".parse().unwrap()),
        Verbosity::Compact,
        None,
    );

    let ended_itself = text(&follower.update(std::slice::from_ref(&cut)));
    let so_far = text(&follower.update(&[cut.clone(), cloning_only.clone()]));
    let run_ended = text(&follower.end(&[cut, cloning_only]));

    assert_eq!(
        ended_itself,
        "run: 105/implement-1\n\
         session 1: started 2026-10-13 10:00:00 UTC\n \
         0:00  clone → running\n \
         0:02  clone → 2s\n \
         0:02  maintenance: npm ci → running\n \
         0:30  maintenance: npm ci → interrupted\n"
    );
    assert_eq!(
        so_far,
        "session 2: started 2026-10-13 11:00:00 UTC\n \
         0:00  clone → running\n"
    );
    assert_eq!(run_ended, " 0:00  clone → interrupted\n");
}
