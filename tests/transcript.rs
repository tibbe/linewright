//! A run's transcript as `logs` shows it, end to end on its data model: a
//! session recorded from Claude Code, rendered compact, in full and as JSON
//! (`docs/specs/monitoring.md`, "logs").

use linewright::transcript::{self, SessionRecord, Transcript, Verbosity};
use serde_json::json;

/// A recorded implement session, trimmed: its first turn, and a second that a
/// follow-up started.
fn recorded() -> SessionRecord {
    SessionRecord {
        output: [
            include_str!("fixtures/transcript/turn-1.jsonl"),
            include_str!("fixtures/transcript/turn-2.jsonl"),
        ]
        .concat(),
        brief: Some("/linewright:implement\n\n# Your task\n\nShip gh.\n".into()),
        followups: vec!["Your answer did not parse as the result.\n\nSend it again.\n".into()],
    }
}

fn of(sessions: Vec<SessionRecord>) -> Transcript {
    Transcript {
        run: "105/implement-1".parse().unwrap(),
        sessions: sessions.iter().map(transcript::parse).collect(),
    }
}

#[test]
fn compact_shows_messages_in_full_and_each_tool_call_on_one_line() {
    let shown = transcript::render(&of(vec![recorded()]), Verbosity::Compact, false);

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
    let shown = transcript::render(&of(vec![recorded()]), Verbosity::Full, false);

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
    let transcript = of(vec![recorded()]);

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
            "tool": "Bash",
            "argument": "git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…",
            "account": "exit 0, 7 lines",
            "error": false,
        })
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

    let full = transcript::render(&of(vec![record.clone()]), Verbosity::Full, false);
    let compact = transcript::render(&of(vec![record]), Verbosity::Compact, false);

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
