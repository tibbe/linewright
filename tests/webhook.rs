//! Webhook deliveries, end to end as GitHub sends them: signed HTTP requests
//! to the listener `serve` runs with the secret set, and the log the operator
//! reads (`docs/specs/product.md`, "Webhooks").

mod common;

use common::Log;
use hmac::{Hmac, Mac};
use linewright::webhook::{Address, Listener, Secret, Webhooks};
use reqwest::StatusCode;
use sha2::Sha256;

const SECRET: &str = "It's a Secret to Everybody";

async fn listen() -> Listener {
    Listener::bind(&webhooks("127.0.0.1:0")).await.unwrap()
}

fn webhooks(address: &str) -> Webhooks {
    Webhooks {
        address: address.parse::<Address>().unwrap(),
        secret: Secret::new(SECRET.into()).unwrap(),
    }
}

/// The `X-Hub-Signature-256` GitHub sends with `body`.
fn signature(body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Sends a delivery of `event` and returns the response's status.
async fn deliver(
    listener: &Listener,
    event: &str,
    body: &str,
    signature: Option<&str>,
) -> StatusCode {
    let mut request = reqwest::Client::new()
        .post(format!("http://{}/", listener.local_address()))
        .header("X-GitHub-Event", event)
        .header("X-GitHub-Delivery", "72d3162e-cc78-11e3-81ab-4c9367dc0958")
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if let Some(signature) = signature {
        request = request.header("X-Hub-Signature-256", signature);
    }
    request.send().await.unwrap().status()
}

async fn signed(listener: &Listener, event: &str, body: &str) -> StatusCode {
    deliver(listener, event, body, Some(&signature(body))).await
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

    let status = deliver(&listener, "ping", "Hello, World!", Some(signature)).await;

    assert!(status.is_success(), "{status}");
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

        let status = deliver(&listener, "issues", body, signature).await;

        assert!(status.is_client_error(), "{signature:?}: {status}");
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

#[tokio::test]
async fn an_accepted_delivery_is_logged_with_its_event_and_what_it_names() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;
    let deliveries = [
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
            "check_suite",
            r#"{"check_suite":{"pull_requests":[{"number":11},{"number":12}]}}"#,
            "pull requests #11, #12",
        ),
        (
            "status",
            r#"{"sha":"6dcb09b","branches":[{"name":"linewright/7"}]}"#,
            "branch linewright/7",
        ),
        ("push", r#"{"ref":"refs/heads/main"}"#, "branch main"),
    ];

    for (event, body, _) in deliveries {
        let status = signed(&listener, event, body).await;
        assert!(status.is_success(), "{event}: {status}");
    }

    let accepted = log.lines("INFO");
    for (event, _, names) in deliveries {
        assert!(
            accepted
                .iter()
                .any(|line| line.contains(&format!("event={event} ")) && line.contains(names)),
            "{event} naming {names}: {accepted:?}"
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
async fn a_signed_delivery_whose_payload_is_not_json_is_accepted_and_logged_as_ignored() {
    let (log, _guard) = Log::capture();
    let listener = listen().await;
    let body = "payload=%7B%22issue%22%3A%7B%22number%22%3A7%7D%7D";

    let status = signed(&listener, "issues", body).await;

    assert!(status.is_success(), "{status}");
    let ignored = log.lines("WARN");
    assert_eq!(ignored.len(), 1, "{ignored:?}");
    assert!(ignored[0].contains("ignored"), "{}", ignored[0]);
    assert!(ignored[0].contains("application/json"), "{}", ignored[0]);
}

#[tokio::test]
async fn a_stopped_listener_takes_no_more_deliveries() {
    let listener = listen().await;
    let address = listener.local_address();

    listener.stop().await;

    let refused = tokio::net::TcpStream::connect(address).await;
    assert!(refused.is_err(), "still listening on {address}");
}
