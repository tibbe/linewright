use crate::common;

use common::*;
use linewright::webhook::{Listener, Pending};

async fn listen() -> Listener {
    Listener::bind(
        &webhooks("127.0.0.1:0"),
        Pending::default(),
        std::sync::Arc::new(linewright::clock::SystemClock),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn the_listener_logs_the_address_it_listens_on() {
    let (log, _guard) = Log::capture();

    let listener = listen().await;

    let address = listener.local_address().to_string();
    let info = log.lines("INFO");
    assert!(
        info.iter().any(|line| line.contains(&address)),
        "{address}: {info:?}"
    );
}

#[tokio::test]
async fn a_delivery_signed_as_github_documents_is_accepted() {
    // The example in GitHub's documentation
    // (<https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries#testing-the-webhook-payload-validation>).
    let listener = listen().await;
    let signature = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

    let response = Request {
        event: Some("ping"),
        signature: Some(signature.into()),
        ..Request::signed_json("Hello, World!")
    }
    .send(&listener)
    .await;

    assert!(response.status.is_success(), "{}", response.status);
}

#[tokio::test]
async fn a_delivery_with_a_missing_or_wrong_signature_is_refused_and_logged() {
    let body = r#"{"issue":{"number":7}}"#;
    let other = signature(r#"{"issue":{"number":8}}"#);
    let deliveries = [
        (None, "no signature"),
        (Some(other.as_str()), "does not match"),
        (Some("sha256=zz"), "does not match"),
    ];

    for (signature, reason) in deliveries {
        let (log, _guard) = Log::capture();
        let listener = listen().await;

        let response = Request {
            event: Some("issues"),
            signature: signature.map(str::to_string),
            ..Request::signed_json(body)
        }
        .send(&listener)
        .await;

        assert!(
            response.status.is_client_error(),
            "{signature:?}: {}",
            response.status
        );
        assert_eq!(response.body, "", "{signature:?}");
        let refused = log.lines("WARN");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].contains(reason), "{}", refused[0]);
        let info = log.lines("INFO");
        assert!(
            info.iter().all(|line| !line.contains("issue #7")),
            "{info:?}"
        );
    }
}

const DELIVERIES: [(&str, &str, &str); 9] = [
    (
        "issues",
        r#"{"action":"labeled","issue":{"number":7}}"#,
        "issue #7",
    ),
    (
        "issue_comment",
        r#"{"issue":{"number":8,"pull_request":{"url":"x"}}}"#,
        "pull request #8",
    ),
    (
        "pull_request",
        r#"{"number":9,"pull_request":{"number":9}}"#,
        "pull request #9",
    ),
    (
        "pull_request_review",
        r#"{"pull_request":{"number":10}}"#,
        "pull request #10",
    ),
    (
        "pull_request_review_comment",
        r#"{"pull_request":{"number":12}}"#,
        "pull request #12",
    ),
    (
        "check_run",
        r#"{"check_run":{"head_sha":"9b2c4f1"}}"#,
        "commit 9b2c4f1",
    ),
    (
        "check_suite",
        r#"{"check_suite":{"head_sha":"7e8e2de","pull_requests":[{"number":11}]}}"#,
        "commit 7e8e2de",
    ),
    (
        "status",
        r#"{"sha":"6dcb09b","branches":[{"name":"linewright/7"}]}"#,
        "commit 6dcb09b",
    ),
    (
        "push",
        r#"{"ref":"refs/heads/main"}"#,
        "a push to branch main",
    ),
];

fn assert_each_accepted(log: &Log) {
    let accepted = log.lines("INFO");
    for (event, _, names) in DELIVERIES {
        assert!(
            accepted
                .iter()
                .any(|line| line.contains(&format!("event={event} ")) && line.contains(names)),
            "{event} naming {names}: {accepted:?}"
        );
    }
}

#[tokio::test]
async fn a_form_encoded_delivery_is_verified_against_its_body_as_sent() {
    let payload = r#"{"issue":{"number":7}}"#;
    let deliveries = [
        (None, "no signature"),
        // Over the payload rather than the body that carries it.
        (Some(signature(payload)), "does not match"),
    ];

    for (signature, reason) in deliveries {
        let (log, _guard) = Log::capture();
        let listener = listen().await;

        let response = Request {
            event: Some("issues"),
            signature: signature.clone(),
            ..Request::signed_form(payload)
        }
        .send(&listener)
        .await;

        assert_eq!(
            response.status,
            reqwest::StatusCode::UNAUTHORIZED,
            "{signature:?}"
        );
        assert_eq!(response.body, "", "{signature:?}");
        let refused = log.lines("WARN");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].contains(reason), "{}", refused[0]);
    }
}

#[tokio::test]
async fn an_accepted_delivery_is_logged_with_its_event_and_what_it_names() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;

    for (event, body, _) in DELIVERIES {
        let status = signed(&listener, event, body).await;
        assert!(status.is_success(), "{event}: {status}");
    }

    assert_each_accepted(&log);
}

#[tokio::test]
async fn a_delivery_sent_form_encoded_names_what_its_json_counterpart_does() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;

    for (event, payload, _) in DELIVERIES {
        let status = signed_form(&listener, event, payload).await;
        assert!(status.is_success(), "{event}: {status}");
    }

    assert!(log.lines("WARN").is_empty(), "{:?}", log.lines("WARN"));
    assert_each_accepted(&log);
}

#[tokio::test]
async fn a_content_type_with_parameters_is_read_by_its_media_type() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;
    let content_types = [
        (
            "application/json; charset=utf-8",
            Request::signed_json(r#"{"issue":{"number":7}}"#),
        ),
        (
            "Application/JSON",
            Request::signed_json(r#"{"issue":{"number":8}}"#),
        ),
        (
            "application/x-www-form-urlencoded;charset=UTF-8",
            Request::signed_form(r#"{"issue":{"number":9}}"#),
        ),
    ];

    for (content_type, request) in content_types {
        let response = Request {
            event: Some("issues"),
            content_type: Some(content_type),
            ..request
        }
        .send(&listener)
        .await;
        assert!(
            response.status.is_success(),
            "{content_type}: {}",
            response.status
        );
    }

    let accepted = log.lines("INFO");
    for names in ["issue #7", "issue #8", "issue #9"] {
        assert!(
            accepted.iter().any(|line| line.contains(names)),
            "{names}: {accepted:?}"
        );
    }
}

#[tokio::test]
async fn the_same_delivery_sent_twice_is_accepted_both_times() {
    let listener = listen().await;
    let body = r#"{"issue":{"number":7}}"#;

    let first = signed(&listener, "issues", body).await;
    let second = signed(&listener, "issues", body).await;

    assert!(first.is_success(), "{first}");
    assert!(second.is_success(), "{second}");
}

#[tokio::test]
async fn an_event_outside_the_table_is_accepted_and_ignored() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;

    let ping = signed(&listener, "ping", r#"{"zen":"Keep it logically awesome."}"#).await;
    let star = signed(&listener, "star", r#"{"action":"created"}"#).await;

    assert!(ping.is_success(), "{ping}");
    assert!(star.is_success(), "{star}");
    assert!(log.lines("WARN").is_empty(), "{:?}", log.lines("WARN"));
    let ignored: Vec<String> = log
        .lines("INFO")
        .into_iter()
        .filter(|line| line.contains("ignored"))
        .collect();
    assert_eq!(ignored.len(), 2, "{ignored:?}");
}

#[tokio::test]
async fn a_signed_delivery_that_names_no_event_or_that_cannot_be_read_is_refused_saying_why() {
    let form = "payload=%7B%22issue%22%3A%7B%22number%22%3A7%7D%7D";
    let deliveries = [
        (
            None,
            Some("application/json"),
            r#"{"issue":{"number":7}}"#,
            "names no event",
        ),
        (
            Some("issues"),
            Some("application/x-www-form-urlencoded"),
            r#"{"issue":{"number":7}}"#,
            "no `payload` field",
        ),
        (
            Some("issues"),
            Some("application/x-www-form-urlencoded"),
            "zen=%7B%7D",
            "no `payload` field",
        ),
        (
            Some("issues"),
            Some("application/x-www-form-urlencoded"),
            "payload=not+json",
            "`issues` payload",
        ),
        (
            Some("issues"),
            Some("application/x-www-form-urlencoded"),
            "payload=%7B%22issue%22%3A%7B%22title%22%3A%22x%22%7D%7D",
            "missing field `number`",
        ),
        (
            Some("issues"),
            None,
            r#"{"issue":{"number":7}}"#,
            "content type",
        ),
        (
            Some("issues"),
            Some("text/plain"),
            r#"{"issue":{"number":7}}"#,
            "`text/plain`",
        ),
        (
            Some("issues"),
            Some("application/json"),
            form,
            "`issues` payload",
        ),
        (
            Some("issues"),
            Some("application/json"),
            r#"{"action":"labeled","issue":{"title":"x"}}"#,
            "missing field `number`",
        ),
        (
            Some("check_run"),
            Some("application/json"),
            r#"{"action":"completed"}"#,
            "missing field `check_run`",
        ),
    ];

    for (event, content_type, body, reason) in deliveries {
        let (log, _guard) = Log::capture();
        let listener = listen().await;

        let response = Request {
            event,
            content_type,
            ..Request::signed_json(body)
        }
        .send(&listener)
        .await;

        let case = format!("{event:?} {content_type:?} {body}");
        assert!(
            response.status.is_client_error(),
            "{case}: {}",
            response.status
        );
        assert!(response.body.contains(reason), "{case}: {}", response.body);
        let refused = log.lines("WARN");
        assert_eq!(refused.len(), 1, "{case}: {refused:?}");
        assert!(refused[0].contains("refused"), "{case}: {}", refused[0]);
        assert!(refused[0].contains(reason), "{case}: {}", refused[0]);
    }
}

#[tokio::test]
async fn a_signed_delivery_of_an_event_outside_the_table_is_ignored_whatever_its_payload() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;
    let deliveries = [
        (Some("application/json"), r#"{"deployment":{"id":1}}"#),
        (Some("application/json"), "not json"),
        (Some("application/x-www-form-urlencoded"), "payload=%7B%7D"),
        (None, "anything"),
    ];

    for (content_type, body) in deliveries {
        let response = Request {
            event: Some("deployment"),
            content_type,
            ..Request::signed_json(body)
        }
        .send(&listener)
        .await;

        assert!(
            response.status.is_success(),
            "{content_type:?} {body}: {}",
            response.status
        );
    }

    assert!(log.lines("WARN").is_empty(), "{:?}", log.lines("WARN"));
    let ignored: Vec<String> = log
        .lines("INFO")
        .into_iter()
        .filter(|line| line.contains("ignored"))
        .collect();
    assert_eq!(ignored.len(), deliveries.len(), "{ignored:?}");
}

#[tokio::test]
async fn a_signed_push_to_something_other_than_a_branch_is_accepted_and_ignored() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;

    let status = signed(&listener, "push", r#"{"ref":"refs/tags/v1.0"}"#).await;

    assert!(status.is_success(), "{status}");
    assert!(log.lines("WARN").is_empty(), "{:?}", log.lines("WARN"));
    let info = log.lines("INFO");
    assert!(info.iter().any(|line| line.contains("ignored")), "{info:?}");
}

#[tokio::test]
async fn a_stopped_listener_takes_no_more_deliveries() {
    let listener = listen().await;
    let address = listener.local_address();

    listener.stop().await;

    let refused = tokio::net::TcpStream::connect(address).await;
    assert!(refused.is_err(), "still listening on {address}");
}
