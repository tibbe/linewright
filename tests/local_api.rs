//! The local API and `linewright status`, end to end at the operator's
//! level: a factory serving the local store with the API on a socket, and
//! the `linewright` program asking it (`docs/specs/monitoring.md`;
//! `docs/specs/architecture.md`, "The local API").

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use common::*;
use linewright::api::{self, Api, ClaimError, Sources};
use linewright::clock::Clock;
use linewright::webhook::{Listener, Pending};

const REPO: &str = "owner/widgets";

/// A world's factory serving with the API answering on a socket under its
/// own runtime directory, as `serve` does.
struct Answering {
    serving: Serving,
    api: Api,
    listener: Option<Listener>,
    /// Stands in for `XDG_RUNTIME_DIR`.
    runtime_dir: tempfile::TempDir,
    /// The world's directory, which its store is in.
    _dir: tempfile::TempDir,
}

impl Answering {
    /// Serves `world`'s factory, polling hourly so its view holds still,
    /// with webhooks on if `webhooks`, and waits for its first poll.
    async fn start(world: World, webhooks: bool) -> Answering {
        let runtime_dir = tempfile::tempdir().unwrap();
        let claimed = api::claim(&socket_dir(runtime_dir.path()), REPO).unwrap();
        let pending = Pending::default();
        let listener = if webhooks {
            Some(
                Listener::bind(
                    &common::webhooks("127.0.0.1:0"),
                    pending.clone(),
                    world.clock.clone(),
                )
                .await
                .unwrap(),
            )
        } else {
            None
        };
        let World {
            clock,
            factory,
            dir,
            ..
        } = world;
        let since = clock.now();
        // As `common::config` has it.
        let work_dir = dir.path().join("work");
        let serving = Serving::start(factory, Duration::from_secs(3600), pending);
        let api = Api::serve(
            claimed,
            Sources {
                serving: linewright::status::Serving { since, max_runs: 4 },
                view: serving.view.clone(),
                deliveries: listener.as_ref().map(Listener::tally),
                clock,
                work_dir: linewright::runs::RepoWorkDir {
                    base: work_dir,
                    repo: REPO.into(),
                },
            },
        );
        let mut view = serving.view.clone();
        tokio::time::timeout(Duration::from_secs(10), view.wait_for(Option::is_some))
            .await
            .expect("the first poll")
            .unwrap();
        Answering {
            serving,
            api,
            listener,
            runtime_dir,
            _dir: dir,
        }
    }

    /// Runs `linewright --repo owner/widgets` with `args`, as the operator
    /// would on this host.
    async fn linewright(&self, args: &[&str]) -> Output {
        linewright(self.runtime_dir.path(), args).await
    }

    async fn stop(self) {
        if let Some(listener) = self.listener {
            listener.stop().await;
        }
        self.api.stop().await;
        self.serving.stop().await;
    }
}

/// Where the sockets go with `XDG_RUNTIME_DIR` set to `runtime_dir`.
fn socket_dir(runtime_dir: &Path) -> std::path::PathBuf {
    api::socket_dir(Some(runtime_dir.to_path_buf()), Path::new("/unused"), 0)
}

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs `linewright --repo owner/widgets` with `args` and `XDG_RUNTIME_DIR`
/// set to `runtime_dir`, in an empty working directory.
async fn linewright(runtime_dir: &Path, args: &[&str]) -> Output {
    let args: Vec<&str> = ["--repo", REPO].iter().chain(args).copied().collect();
    linewright_with(runtime_dir, &args, &[]).await
}

/// Runs `linewright` with `args`, `env` and `XDG_RUNTIME_DIR` set to
/// `runtime_dir`, in an empty working directory.
async fn linewright_with(runtime_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let cwd = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_linewright"))
        .args(args)
        .current_dir(cwd.path())
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env_remove("LINEWRIGHT_WEBHOOK_SECRET")
        .envs(env.iter().copied())
        .output()
        .await
        .unwrap();
    Output {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// Sends `GET resource` with `host` as its `Host` to the socket of the
/// `serve` in `runtime_dir`, and returns the response's status and body.
async fn get(runtime_dir: &Path, host: &str, resource: &str) -> (u16, String) {
    use http_body_util::BodyExt;
    let path = api::socket_path(&socket_dir(runtime_dir), REPO);
    let stream = tokio::net::UnixStream::connect(path).await.unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .unwrap();
    tokio::spawn(connection);
    let request = hyper::Request::get(resource)
        .header("Host", host)
        .body(http_body_util::Empty::<axum::body::Bytes>::new())
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

async fn with_ticket_and_workflow() -> World {
    let world = World::new().await;
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-human"],
    );
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/workflows/weekly.md",
                "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
            )],
            "Add a workflow",
        )
        .await
        .unwrap();
    world
}

#[tokio::test]
async fn status_shows_linewright_then_the_tracked_items_then_the_workflows() {
    let answering = Answering::start(with_ticket_and_workflow().await, false).await;

    let output = answering.linewright(&["status"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    let labels: Vec<&str> = lines
        .iter()
        .map(|line| line.split_once(':').map_or(*line, |(label, _)| label))
        .collect();
    assert_eq!(
        labels,
        [
            "serving since",
            "last poll",
            "next poll",
            "runs in flight",
            "",
            "#1",
            "",
            "workflow weekly last run",
            "workflow weekly next run",
        ],
        "{}",
        output.stdout
    );
    assert_eq!(lines[3], "runs in flight: 0 of 4");
    assert_eq!(lines[5], "#1: with the maintainer — Do the thing");
    answering.stop().await;
}

#[tokio::test]
async fn status_to_a_pipe_is_not_coloured() {
    let answering = Answering::start(with_ticket_and_workflow().await, false).await;

    let output = answering.linewright(&["status"]).await;

    assert!(!output.stdout.contains('\x1b'), "{:?}", output.stdout);
    answering.stop().await;
}

#[tokio::test]
async fn status_json_prints_the_api_answer_as_it_is() {
    let answering = Answering::start(with_ticket_and_workflow().await, false).await;
    let (status, answer) = get(answering.runtime_dir.path(), "localhost", "/status").await;

    let output = answering.linewright(&["status", "--json"]).await;

    assert_eq!(status, 200);
    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, format!("{answer}\n"));
    let json: serde_json::Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(json["items"][0]["number"], 1);
    assert_eq!(json["items"][0]["state"], "with-the-maintainer");
    answering.stop().await;
}

#[tokio::test]
async fn with_webhooks_on_status_shows_the_last_accepted_delivery_and_those_refused() {
    let answering = Answering::start(World::new().await, true).await;
    let listener = answering.listener.as_ref().unwrap();
    signed(listener, "ping", "{}").await;
    let unsigned = Request {
        signature: None,
        ..Request::signed_json("{}")
    };
    unsigned.send(listener).await;
    unsigned.send(listener).await;

    let output = answering.linewright(&["status"]).await;

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert!(
        lines.iter().any(|line| line
            .strip_prefix("last delivery accepted: 2026-10-01 12:")
            .is_some_and(|rest| rest.ends_with(" UTC"))),
        "{}",
        output.stdout
    );
    assert!(
        lines.contains(&"deliveries refused: 2"),
        "{}",
        output.stdout
    );
    answering.stop().await;
}

#[tokio::test]
async fn a_request_whose_host_is_not_local_is_refused() {
    let answering = Answering::start(World::new().await, false).await;
    let dir = answering.runtime_dir.path();

    let refused = [
        get(dir, "evil.example", "/status").await,
        get(dir, "evil.example:80", "/status").await,
        get(dir, "localhost.evil.example", "/status").await,
    ];
    let answered = [
        get(dir, "localhost", "/status").await,
        get(dir, "127.0.0.1:8080", "/status").await,
        get(dir, "[::1]:8080", "/status").await,
    ];

    for (status, body) in refused {
        assert_eq!(status, 403, "{body}");
        assert!(!body.contains("owner/widgets"), "{body}");
    }
    for (status, body) in answered {
        assert_eq!(status, 200, "{body}");
    }
    answering.stop().await;
}

#[tokio::test]
async fn only_serves_user_can_open_the_socket() {
    let answering = Answering::start(World::new().await, false).await;
    let dir = socket_dir(answering.runtime_dir.path());
    let socket = api::socket_path(&dir, REPO);

    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&socket), 0o600);
    answering.stop().await;
}

#[tokio::test]
async fn status_reaches_the_serve_of_the_repository_whatever_the_case_of_its_name() {
    let answering = Answering::start(World::new().await, false).await;
    let dir = answering.runtime_dir.path();

    let same = linewright_with(dir, &["--repo", "Owner/Widgets", "status"], &[]).await;
    let other = linewright_with(dir, &["--repo", "owner/gadgets", "status"], &[]).await;

    assert_eq!(same.code, Some(0), "{}", same.stderr);
    assert_eq!(other.code, Some(3), "{}", other.stderr);
    answering.stop().await;
}

#[tokio::test]
async fn a_second_serve_for_the_repository_exits_saying_one_already_runs() {
    let answering = Answering::start(World::new().await, false).await;

    let in_process = api::claim(&socket_dir(answering.runtime_dir.path()), REPO);
    let output = answering.linewright(&["serve"]).await;

    assert!(
        matches!(in_process, Err(ClaimError::AlreadyServing { .. })),
        "the second claim was taken"
    );
    assert_eq!(output.code, Some(1));
    assert!(output.stderr.contains("already runs"), "{}", output.stderr);
    let (status, _) = get(answering.runtime_dir.path(), "localhost", "/status").await;
    assert_eq!(status, 200, "the first still answers");
    answering.stop().await;
}

#[tokio::test]
async fn a_socket_a_dead_serve_left_behind_does_not_block_a_new_one() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let dir = socket_dir(runtime_dir.path());
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = api::socket_path(&dir, REPO);
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    std::fs::write(socket.with_extension("lock"), "").unwrap();
    assert!(socket.exists());

    let claimed = api::claim(&dir, REPO);

    assert!(claimed.is_ok(), "{:?}", claimed.err());
}

#[tokio::test]
async fn once_a_serve_stops_another_can_start() {
    let answering = Answering::start(World::new().await, false).await;
    let dir = socket_dir(answering.runtime_dir.path());
    let Answering {
        serving,
        api,
        runtime_dir: _runtime_dir,
        ..
    } = answering;

    api.stop().await;
    serving.stop().await;

    // A process another test forks holds a copy of the lock until it runs
    // its program, so the lock may stay taken for a moment.
    eventually(|| api::claim(&dir, REPO).is_ok()).await;
}

#[tokio::test]
async fn status_without_serve_running_exits_3_saying_so() {
    let runtime_dir = tempfile::tempdir().unwrap();

    let output = linewright(runtime_dir.path(), &["status"]).await;

    assert_eq!(output.code, Some(3));
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("not running"), "{}", output.stderr);
    assert!(output.stderr.contains(REPO), "{}", output.stderr);
}

#[tokio::test]
async fn a_wrong_command_line_exits_2() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let wrong = [
        &["status", "--no-such-flag"][..],
        &["status", "--poll-interval", "often"][..],
    ];

    for args in wrong {
        let output = linewright(runtime_dir.path(), args).await;

        assert_eq!(output.code, Some(2), "{args:?}: {}", output.stderr);
        assert_eq!(output.stdout, "");
        assert!(!output.stderr.is_empty());
    }
}

#[tokio::test]
async fn a_wrong_setting_outside_the_command_line_exits_1() {
    let runtime_dir = tempfile::tempdir().unwrap();

    let output = linewright_with(
        runtime_dir.path(),
        &["--repo", REPO, "status"],
        &[("LINEWRIGHT_POLL_INTERVAL", "often")],
    )
    .await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(output.stderr.contains("poll-interval"), "{}", output.stderr);
}

/// A world whose factory records its runs where `serve` and
/// `linewright --work-dir <dir>/work` look for those of `owner/widgets`,
/// with an issue for triage, which triage readies.
async fn with_issue_for_triage() -> World {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    world
        .runtime
        .on(linewright::reconcile::WorkKind::Triage, |_| {
            json(serde_json::json!({
                "outcome": "ready-for-agent",
                "category": "bug",
                "comment": "## Agent Brief\n\n**Summary:** It crashes."
            }))
        });
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world
}

#[tokio::test]
async fn runs_gives_the_same_answer_with_serve_running_and_without() {
    let answering = Answering::start(with_issue_for_triage().await, false).await;
    let work_dir = answering._dir.path().join("work");
    let work_dir = work_dir.to_str().unwrap();
    let args = ["--work-dir", work_dir, "runs", "1"];
    eventually_async(|| async { answering.linewright(&args).await.stdout.contains("done") }).await;

    let served = [
        answering.linewright(&args).await,
        answering
            .linewright(&[&args[..], &["--json"]].concat())
            .await,
    ];
    let Answering {
        serving,
        api,
        runtime_dir,
        _dir,
        ..
    } = answering;
    api.stop().await;
    serving.stop().await;
    let read = [
        linewright(runtime_dir.path(), &args).await,
        linewright(runtime_dir.path(), &[&args[..], &["--json"]].concat()).await,
    ];

    for (from_serve, from_records) in served.iter().zip(&read) {
        assert_eq!(from_serve.code, Some(0), "{}", from_serve.stderr);
        assert_eq!(from_records.code, Some(0), "{}", from_records.stderr);
        assert_eq!(from_serve.stdout, from_records.stdout);
    }
    assert!(
        read[0]
            .stdout
            .starts_with("1/triage-1: done — #1 triage, started "),
        "{}",
        read[0].stdout
    );
}

#[tokio::test]
async fn a_run_in_flight_is_listed_so_and_once_a_stop_cancels_it_as_cancelled() {
    let world = with_issue_for_triage().await;
    world.runtime.takes(Duration::from_secs(60));
    let answering = Answering::start(world, false).await;
    let work_dir = answering._dir.path().join("work");
    let work_dir = work_dir.to_str().unwrap();
    let args = ["--work-dir", work_dir, "runs"];
    eventually_async(|| async { !answering.linewright(&args).await.stdout.is_empty() }).await;

    let in_flight = answering.linewright(&args).await;
    let Answering {
        serving,
        api,
        runtime_dir,
        _dir,
        ..
    } = answering;
    api.stop().await;
    serving.stop().await;
    let stopped = linewright(runtime_dir.path(), &args).await;

    assert!(
        in_flight
            .stdout
            .starts_with("1/triage-1: in flight — #1 triage, started "),
        "{}",
        in_flight.stdout
    );
    assert!(
        stopped
            .stdout
            .starts_with("1/triage-1: cancelled — #1 triage, started "),
        "{}",
        stopped.stdout
    );
}

#[tokio::test]
async fn runs_of_an_item_with_none_exits_4_with_serve_running() {
    let answering = Answering::start(World::new().await, false).await;

    let output = answering.linewright(&["runs", "7"]).await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert!(
        output.stderr.contains("#7 has no runs"),
        "{}",
        output.stderr
    );
    answering.stop().await;
}

#[tokio::test]
async fn a_run_whose_serve_died_is_listed_as_interrupted() {
    let world = with_issue_for_triage().await;
    world.runtime.takes(Duration::from_secs(60));
    let answering = Answering::start(world, false).await;
    let work_dir = answering._dir.path().join("work");
    let work_dir = work_dir.to_str().unwrap();
    let args = ["--work-dir", work_dir, "runs"];
    eventually_async(|| async { !answering.linewright(&args).await.stdout.is_empty() }).await;
    let Answering {
        serving,
        api,
        runtime_dir,
        _dir,
        ..
    } = answering;

    serving.kill().await;
    api.stop().await;

    let output = linewright(runtime_dir.path(), &args).await;
    let line = output.stdout.lines().next().unwrap_or_default();
    assert!(
        line.starts_with("1/triage-1: interrupted — #1 triage, started 2026-10-01 12:00:"),
        "{}",
        output.stdout
    );
    assert!(line.ends_with(" UTC"), "no duration: {line}");
}

#[tokio::test]
async fn a_resource_serve_does_not_have_is_an_error_not_a_missing_item() {
    let answering = Answering::start(World::new().await, false).await;

    let answer = api::get_answer(&socket_dir(answering.runtime_dir.path()), REPO, "/nope").await;

    assert!(
        matches!(answer, Err(api::RequestError::Other(_))),
        "{:?}",
        answer.err()
    );
    answering.stop().await;
}
