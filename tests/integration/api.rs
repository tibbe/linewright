use crate::common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::StatusCode;
use common::*;
use linewright::api::{self, Api, ClaimError, RequestError, Sources};
use linewright::clock::Clock;
use linewright::ending::Ends;
use linewright::runs::RepoWorkDir;
use tokio_util::sync::CancellationToken;

/// What `serve`'s local API answers from before its first poll, over a work
/// directory holding nothing but what a test writes there.
fn sources(work_dir: RepoWorkDir) -> Sources {
    let clock = Arc::new(FakeClock::new());
    // Kept open by the receivers `Sources` holds.
    let (_, served) = tokio::sync::watch::channel(None);
    let (_, polling) = tokio::sync::watch::channel(None);
    Sources {
        serving: linewright::status::Serving {
            since: clock.now(),
            max_runs: 4,
        },
        served,
        polling,
        deliveries: None,
        clock,
        work_dir,
        ends: Ends::new(),
    }
}

/// The local API's router, driven in-process, over a work directory holding
/// `files`.
struct Routed {
    router: Router,
    ends: Ends,
    stopping: CancellationToken,
    _work_dir: tempfile::TempDir,
}

impl Routed {
    fn over(files: &[(&str, &str)]) -> Routed {
        let (dir, work_dir) = work_dir_holding(files);
        let sources = sources(work_dir);
        let ends = sources.ends.clone();
        let stopping = CancellationToken::new();
        Routed {
            router: api::router(sources, stopping.clone()),
            ends,
            stopping,
            _work_dir: dir,
        }
    }

    async fn response(
        &self,
        method: hyper::Method,
        host: Option<&str>,
        path: &str,
    ) -> axum::response::Response {
        routed(&self.router, method, host, path).await
    }

    async fn answer(&self, method: hyper::Method, host: Option<&str>, path: &str) -> (u16, String) {
        status_and_body(self.response(method, host, path).await).await
    }
}

impl Drop for Routed {
    /// Ends the event streams.
    fn drop(&mut self) {
        self.stopping.cancel();
    }
}

/// `docs/specs/monitoring.md`, Commands: the local API answers under
/// `/api/`, and serves none of the web UI.
#[tokio::test]
async fn the_api_answers_under_api_and_nothing_else() {
    let routed = Routed::over(&[]);
    let elsewhere = [
        (hyper::Method::GET, "/"),
        (hyper::Method::GET, "/items/1"),
        (hyper::Method::GET, "/events/items/1"),
        (hyper::Method::GET, "/server"),
        (hyper::Method::GET, "/assets/linewright.css"),
        (hyper::Method::GET, "/status"),
        (hyper::Method::GET, "/logs?item=1"),
        (hyper::Method::GET, "/logs/follow?item=1"),
        (hyper::Method::GET, "/stop"),
        (hyper::Method::POST, "/drain"),
    ];

    for (method, path) in elsewhere {
        let (status, body) = routed.answer(method, Some("localhost"), path).await;

        assert_eq!(status, 404, "{path}: {body}");
        assert_eq!(body, r#"{"error":"no such resource"}"#, "{path}");
    }
    let (status, body) = routed
        .answer(hyper::Method::GET, Some("localhost"), "/api/status")
        .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn a_request_whose_host_is_not_local_is_refused() {
    let routed = Routed::over(&[]);

    for host in [Some("evil.example"), None] {
        let (status, body) = routed.answer(hyper::Method::GET, host, "/api/status").await;

        assert_eq!(status, 403, "{host:?}");
        assert_eq!(
            body, r#"{"error":"only requests whose `Host` is local are answered"}"#,
            "{host:?}"
        );
    }
}

#[tokio::test]
async fn a_drain_whose_host_is_not_local_is_refused_and_changes_nothing() {
    let routed = Routed::over(&[]);

    let (status, _) = routed
        .answer(hyper::Method::POST, Some("evil.example"), "/api/drain")
        .await;

    assert_eq!(status, 403);
    assert!(!routed.ends.draining().is_cancelled());
    assert!(!routed.ends.stopping().is_cancelled());
}

#[tokio::test]
async fn following_a_transcript_passes_through_a_proxy_unbuffered_unchanged_and_uncached() {
    let record = item_run_record(1, "triage", 1, Some("done"), &serde_json::json!([]));
    let routed = Routed::over(&[("runs/items/1/triage-1.json", &record)]);

    let response = routed
        .response(
            hyper::Method::GET,
            Some("localhost"),
            "/api/logs/follow?item=1",
        )
        .await;

    assert_eq!(response.status(), 200);
    assert_passes_through_a_proxy(response.headers());
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// `docs/specs/monitoring.md`, Commands: only the user `serve` runs as can
/// open its socket.
#[tokio::test]
async fn a_claim_lets_only_its_user_open_the_socket() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let dir = socket_dir_in(runtime_dir.path());

    let _claimed = api::claim(&dir, REPO).unwrap();

    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&api::socket_path(&dir, REPO)), 0o600);
}

/// GitHub names repositories case-insensitively.
#[test]
fn a_repositorys_socket_is_the_same_whatever_the_case_of_its_name() {
    let dir = Path::new("/run/user/1000/linewright");

    assert_eq!(
        api::socket_path(dir, "Owner/Widgets"),
        api::socket_path(dir, "owner/widgets")
    );
    assert_ne!(
        api::socket_path(dir, "owner/gadgets"),
        api::socket_path(dir, "owner/widgets")
    );
}

#[tokio::test]
async fn a_claim_is_held_until_the_api_stops_and_then_can_be_taken_again() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let dir = socket_dir_in(runtime_dir.path());
    let (_work_dir, work_dir) = work_dir_holding(&[]);
    let api = Api::serve(api::claim(&dir, REPO).unwrap(), sources(work_dir));

    let claimed_while_served = api::is_claimed(&dir, REPO).unwrap();
    let second_claim = api::claim(&dir, REPO);
    tokio::time::timeout(Duration::from_secs(10), api.stop())
        .await
        .expect("the API stops");

    assert!(claimed_while_served);
    assert!(
        matches!(second_claim, Err(ClaimError::AlreadyServing { .. })),
        "the second claim was taken"
    );
    assert!(!api::socket_path(&dir, REPO).exists());
    // A process another test forks holds a copy of the lock until it runs
    // its program, so the lock may stay taken for a moment.
    eventually(|| api::claim(&dir, REPO).is_ok()).await;
}

/// The socket directory under `runtime_dir`, made as a `serve` makes it:
/// open to its user only.
fn private_socket_dir(runtime_dir: &Path) -> std::path::PathBuf {
    let dir = socket_dir_in(runtime_dir);
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// The private directory a `serve` that was killed claimed, with the socket
/// and the lock it left behind.
fn left_by_a_killed_serve(runtime_dir: &Path) -> std::path::PathBuf {
    let dir = private_socket_dir(runtime_dir);
    let socket = api::socket_path(&dir, REPO);
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    // The kernel released its lock as it was killed.
    std::fs::write(socket.with_extension("lock"), "").unwrap();
    dir
}

#[test]
fn a_socket_a_killed_serve_left_behind_is_not_claimed() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let dir = left_by_a_killed_serve(runtime_dir.path());

    assert!(api::socket_path(&dir, REPO).exists());
    assert!(!api::is_claimed(&dir, REPO).unwrap());
}

#[test]
fn nothing_is_claimed_where_no_serve_has_run() {
    let runtime_dir = tempfile::tempdir().unwrap();

    assert!(!api::is_claimed(&socket_dir_in(runtime_dir.path()), REPO).unwrap());
}

#[tokio::test]
async fn a_socket_a_dead_serve_left_behind_does_not_block_a_new_one() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let dir = left_by_a_killed_serve(runtime_dir.path());

    let claimed = api::claim(&dir, REPO);

    assert!(claimed.is_ok(), "{:?}", claimed.err());
}

#[tokio::test]
async fn a_serve_starting_while_drain_looks_at_the_claim_takes_it() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let dir = private_socket_dir(runtime_dir.path());
    let lock = std::fs::File::create(api::socket_path(&dir, REPO).with_extension("lock")).unwrap();
    // As `drain` looks, held for longer.
    lock.lock_shared().unwrap();
    let looked = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(lock);
    });

    let claimed = tokio::task::spawn_blocking(move || api::claim(&dir, REPO))
        .await
        .unwrap();

    looked.await.unwrap();
    assert!(claimed.is_ok(), "{:?}", claimed.err());
}

#[test]
fn an_answer_naming_what_is_missing_is_a_missing_item_and_any_other_error_is_not() {
    let missing = api::check(
        StatusCode::NOT_FOUND,
        &Bytes::from(r#"{"error":"there is no issue or pull request #7","missing":"item"}"#),
    );
    let no_resource = api::check(
        StatusCode::NOT_FOUND,
        &Bytes::from(r#"{"error":"no such resource"}"#),
    );
    let not_json = api::check(StatusCode::BAD_GATEWAY, &Bytes::from("Bad Gateway"));

    assert!(
        matches!(&missing, Err(RequestError::NotFound(error)) if error == "there is no issue or pull request #7"),
        "{missing:?}"
    );
    assert!(
        matches!(&no_resource, Err(RequestError::Other(e))
            if e.to_string() == "`serve` answered 404 Not Found: no such resource"),
        "{no_resource:?}"
    );
    assert!(
        matches!(&not_json, Err(RequestError::Other(e))
            if e.to_string() == "`serve` answered 502 Bad Gateway: Bad Gateway"),
        "{not_json:?}"
    );
    assert!(api::check(StatusCode::OK, &Bytes::from("{}")).is_ok());
}

/// `docs/specs/monitoring.md`, Errors: 3 with no `serve` running, 4 for no
/// such item or run, and 1 for any other error.
#[test]
fn each_refusal_exits_with_its_own_status() {
    let refusals = [
        (
            RequestError::NotServing {
                repo: REPO.into(),
                path: "/run/user/1000/linewright/x.sock".into(),
            },
            3,
        ),
        (
            RequestError::NotFound("there is no run 1/triage-2".into()),
            4,
        ),
        (RequestError::HungUp(anyhow::anyhow!("hung up")), 1),
        (RequestError::Other(anyhow::anyhow!("failed")), 1),
    ];

    for (refusal, code) in refusals {
        assert_eq!(refusal.exit_code(), code, "{refusal:?}");
    }
}
