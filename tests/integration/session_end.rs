//! A session's end in its transcript (`docs/specs/design_system.md`,
//! Transcripts, Running): Linewright records it once the session's work
//! stops, however it ends.

use crate::common;

use common::*;
use linewright::reconcile::WorkKind;
use serde_json::json;

/// The kinds of the events Linewright wrote to `session`, in order.
fn linewrights(session: &[serde_json::Value]) -> Vec<&str> {
    session
        .iter()
        .filter_map(|event| event["type"].as_str())
        .filter(|kind| kind.starts_with("linewright-"))
        .collect()
}

#[test_log::test(tokio::test)]
async fn a_session_whose_agent_answered_ends_with_its_end() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Triage, |_| {
        Ok(
            json!({"outcome": "ready-for-agent", "category": "bug", "comment": "Triaged."})
                .to_string(),
        )
    });
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);

    world.poll().await;

    let sessions = session_events(&world);
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        linewrights(&sessions[0]),
        [
            "linewright-clone",
            "linewright-clone-ended",
            "linewright-session-ended",
        ]
    );
    assert_eq!(
        sessions[0].last().unwrap()["type"],
        "linewright-session-ended"
    );
}

#[test_log::test(tokio::test)]
async fn a_session_whose_agent_failed_ends_with_its_end() {
    let mut world = World::new().await;
    world
        .harness
        .on(WorkKind::Triage, |_| failed_session("it crashed"));
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);

    world.poll().await;

    let sessions = session_events(&world);
    assert_ne!(sessions, [] as [std::vec::Vec<serde_json::Value>; 0]);
    for session in &sessions {
        assert_eq!(
            linewrights(session).last(),
            Some(&"linewright-session-ended"),
            "{session:#?}"
        );
    }
}
