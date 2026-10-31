use crate::common;

use std::path::Path;
use std::time::Duration;

use common::*;
use linewright::api::{self, Api, ClaimError, Sources};
use linewright::clock::Clock;
use linewright::ending::{Ends, Event, Signal};
use linewright::factory::ServeEnd;
use linewright::host::UiHosts;
use linewright::web;
use linewright::webhook::{Listener, Pending};
use tokio_util::sync::CancellationToken;

struct Answering {
    serving: Serving,
    api: Api,
    ends: Ends,
    clock: std::sync::Arc<FakeClock>,
    listener: Option<Listener>,
    /// The web UI, on a port the system chose.
    ui: web::Listener,
    runtime_dir: tempfile::TempDir,
    _dir: tempfile::TempDir,
}

impl Answering {
    /// Serves `world`'s factory, polling hourly so its view holds still,
    /// with webhooks on if `webhooks`, and waits for its first poll.
    async fn start(world: World, webhooks: bool) -> Answering {
        let runtime_dir = tempfile::tempdir().unwrap();
        let claimed = api::claim(&socket_dir_in(runtime_dir.path()), REPO).unwrap();
        Answering::start_claimed(world, webhooks, runtime_dir, claimed).await
    }

    /// As [`Answering::start`], on the socket claimed in `runtime_dir`.
    async fn start_claimed(
        world: World,
        webhooks: bool,
        runtime_dir: tempfile::TempDir,
        claimed: api::Claimed,
    ) -> Answering {
        let answering = Answering::serve(
            world,
            webhooks,
            runtime_dir,
            claimed,
            Duration::from_hours(1),
        )
        .await;
        let mut served = answering.serving.served.clone();
        tokio::time::timeout(Duration::from_secs(10), served.wait_for(Option::is_some))
            .await
            .expect("the first poll")
            .unwrap();
        answering
    }

    /// Serves `world`'s factory, polling every `poll_interval`, with webhooks
    /// off, without waiting for its first poll.
    async fn start_polling(world: World, poll_interval: Duration) -> Answering {
        let runtime_dir = tempfile::tempdir().unwrap();
        let claimed = api::claim(&socket_dir_in(runtime_dir.path()), REPO).unwrap();
        Answering::serve(world, false, runtime_dir, claimed, poll_interval).await
    }

    async fn serve(
        world: World,
        webhooks: bool,
        runtime_dir: tempfile::TempDir,
        claimed: api::Claimed,
        poll_interval: Duration,
    ) -> Answering {
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
        let ends = factory.ends();
        let since = clock.now();
        // As `common::config` has it.
        let work_dir = dir.path().join("work");
        let serving = Serving::start(factory, poll_interval, pending);
        let sources = Sources {
            serving: linewright::status::Serving { since, max_runs: 4 },
            served: serving.served.clone(),
            polling: serving.polling.clone(),
            deliveries: listener.as_ref().map(Listener::tally),
            clock: clock.clone(),
            work_dir: linewright::runs::RepoWorkDir {
                base: work_dir,
                repo: REPO.parse().unwrap(),
            },
            ends: ends.clone(),
        };
        let ui = web::Ui {
            address: "127.0.0.1:0".parse().unwrap(),
            hosts: UiHosts::default(),
        };
        let ui = web::Listener::bind(&ui, sources.clone()).await.unwrap();
        let api = Api::serve(claimed, sources);
        Answering {
            serving,
            api,
            ends,
            clock,
            listener,
            ui,
            runtime_dir,
            _dir: dir,
        }
    }

    async fn lw(&self, args: &[&str]) -> Output {
        lw(self.runtime_dir.path(), args).await
    }

    /// Stops the listeners once serving ends of its own accord, as `serve`
    /// exits then and in its order, and gives what serving ended with.
    fn exits_once_served(self) -> tokio::task::JoinHandle<ServeEnd> {
        let Answering {
            serving,
            api,
            listener,
            ui,
            runtime_dir,
            _dir,
            ..
        } = self;
        tokio::spawn(async move {
            let end = serving.finished().await;
            if let Some(listener) = listener {
                within(STOPPING, "the webhook listener stops", listener.stop()).await;
            }
            within(STOPPING, "the web UI stops", ui.stop()).await;
            within(STOPPING, "the API stops", api.stop()).await;
            // Kept until `serve` exits.
            drop((runtime_dir, _dir));
            end
        })
    }

    async fn stop(self) {
        if let Some(listener) = self.listener {
            within(STOPPING, "the webhook listener stops", listener.stop()).await;
        }
        within(STOPPING, "the web UI stops", self.ui.stop()).await;
        within(STOPPING, "the API stops", self.api.stop()).await;
        within(STOPPING, "serving stops", self.serving.stop()).await;
    }
}

/// How long a test gives one `lw` to answer: more than the 5s `lw` asks a
/// serve that hangs up again for, and less than `eventually_async` waits, so a
/// hung `lw` in it is named.
const ANSWERING: Duration = Duration::from_secs(8);
/// How long a test gives a part of serve to stop.
const STOPPING: Duration = Duration::from_secs(30);

/// Waits up to `limit` for `future`, saying `what` did not happen when it
/// does not finish, so a hang fails its test rather than the whole run.
async fn within<T>(
    limit: Duration,
    what: impl std::fmt::Display,
    future: impl Future<Output = T>,
) -> T {
    tokio::time::timeout(limit, future)
        .await
        .unwrap_or_else(|_| panic!("{what}: not within {limit:?}"))
}

async fn lw(runtime_dir: &Path, args: &[&str]) -> Output {
    let args: Vec<&str> = ["--repo", REPO].iter().chain(args).copied().collect();
    lw_with(runtime_dir, &args, &[]).await
}

async fn lw_with(runtime_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let cwd = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(args)
        .current_dir(cwd.path())
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env("LINEWRIGHT_UI_ADDRESS", "127.0.0.1:0")
        .env_remove("LINEWRIGHT_WEBHOOK_SECRET")
        .envs(env.iter().copied())
        .kill_on_drop(true)
        .output();
    let output = within(ANSWERING, format!("lw {args:?} exits"), output)
        .await
        .unwrap();
    Output {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

async fn get(runtime_dir: &Path, host: &str, resource: &str) -> (u16, String) {
    request(runtime_dir, hyper::Method::GET, host, resource).await
}

async fn request(
    runtime_dir: &Path,
    method: hyper::Method,
    host: &str,
    resource: &str,
) -> (u16, String) {
    use http_body_util::BodyExt;
    let response = send(runtime_dir, method, host, resource).await;
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

async fn send(
    runtime_dir: &Path,
    method: hyper::Method,
    host: &str,
    resource: &str,
) -> hyper::Response<hyper::body::Incoming> {
    let path = api::socket_path(&socket_dir_in(runtime_dir), REPO);
    let stream = tokio::net::UnixStream::connect(path).await.unwrap();
    send_over(stream, method, Some(host), resource).await
}

/// A world whose work dir is `REPO`'s, as `serve`'s API reads it.
async fn with_ticket_and_automation() -> World {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    open_issue_linewright_labeled(
        &world.store,
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
                ".linewright/automations/weekly.md",
                "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    world
}

#[tokio::test]
async fn status_shows_linewright_then_the_managed_items_then_the_automations() {
    let answering = Answering::start(with_ticket_and_automation().await, false).await;

    let output = answering.lw(&["status"]).await;

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
            "automation weekly last run",
            "automation weekly next run",
        ],
        "{}",
        output.stdout
    );
    // To a pipe.
    assert!(!output.stdout.contains('\x1b'), "{:?}", output.stdout);
    answering.stop().await;
}

/// Waits until `count` polls have failed since the last that succeeded.
async fn polls_failed(answering: &Answering, count: u64) {
    let mut polling = answering.serving.polling.clone();
    let failed = polling.wait_for(|polling| {
        polling
            .as_ref()
            .and_then(|polling| polling.failed.as_ref())
            .is_some_and(|failed| failed.count.get() == count)
    });
    within(
        Duration::from_secs(10),
        format!("{count} failed polls"),
        failed,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn status_shows_the_polls_failed_since_the_last_that_succeeded_with_the_last_error_redacted()
{
    let world = World::with(|config| {
        config.work_dir = config.work_dir.join(REPO);
        config.secrets = secrets_of(&[("GITHUB_TOKEN", "ghp_sekrit")]);
    })
    .await;
    let store = world.store.clone();
    store.fail_reads("GitHub is down");
    let answering = Answering::start_polling(world, Duration::from_millis(500)).await;
    polls_failed(&answering, 1).await;
    answering.clock.advance(chrono::TimeDelta::minutes(5));
    store.fail_reads("GitHub refused ghp_sekrit");
    polls_failed(&answering, 2).await;
    // No poll follows a drain.
    answering.serving.drain();

    let text = answering.lw(&["status"]).await;

    // The fake clock ticks a second at each reading, so the times are
    // checked to the minute.
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    let line = text.stdout.lines().nth(3).unwrap_or_default();
    let (since, last) = line
        .strip_prefix("polls failed: 2 since 2026-10-01 12:00:")
        .and_then(|line| line.split_once(" UTC, the last at 2026-10-01 12:05:"))
        .unwrap_or_else(|| panic!("{}", text.stdout));
    assert_eq!(since.len(), 2, "{line}");
    assert_eq!(
        &last[2..],
        " UTC: taking a snapshot: GitHub refused [redacted]",
        "{line}"
    );
    answering.stop().await;
}

#[tokio::test]
async fn status_json_prints_the_api_answer_as_it_is() {
    let answering = Answering::start(with_ticket_and_automation().await, false).await;
    let (status, answer) = get(answering.runtime_dir.path(), "localhost", "/api/status").await;

    let output = answering.lw(&["status", "--json"]).await;

    assert_eq!(status, 200);
    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, format!("{answer}\n"));
    answering.stop().await;
}

#[tokio::test]
async fn status_shows_a_run_by_hand_from_its_record_as_the_last_run() {
    let world = with_ticket_and_automation().await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();
    let answering = Answering::start(world, false).await;

    let output = answering.lw(&["status"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert!(
        output
            .stdout
            .lines()
            .any(|line| line.starts_with("automation weekly last run: weekly/1 by hand, started ")),
        "{}",
        output.stdout
    );
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

    let output = answering.lw(&["status"]).await;

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert!(
        lines.iter().any(|line| line
            .strip_prefix("last webhook delivery accepted: 2026-10-01 12:")
            .is_some_and(|rest| rest.ends_with(" UTC"))),
        "{}",
        output.stdout
    );
    assert!(
        lines.contains(&"webhook deliveries refused: 2"),
        "{}",
        output.stdout
    );
    answering.stop().await;
}

#[tokio::test]
async fn a_second_serve_for_the_repository_exits_saying_one_already_runs() {
    let answering = Answering::start(World::new().await, false).await;

    let in_process = api::claim(&socket_dir_in(answering.runtime_dir.path()), REPO);
    let output = answering.lw(&["serve"]).await;

    assert!(
        matches!(in_process, Err(ClaimError::AlreadyServing { .. })),
        "the second claim was taken"
    );
    assert_eq!(output.code, Some(1));
    assert!(output.stderr.contains("already runs"), "{}", output.stderr);
    let (status, _) = get(answering.runtime_dir.path(), "localhost", "/api/status").await;
    assert_eq!(status, 200, "the first still answers");
    answering.stop().await;
}

#[tokio::test]
async fn status_without_serve_running_exits_3_saying_so() {
    let runtime_dir = tempfile::tempdir().unwrap();

    let output = lw(runtime_dir.path(), &["status"]).await;

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
        &["drain", "--no-such-flag"][..],
    ];

    for args in wrong {
        let output = lw(runtime_dir.path(), args).await;

        assert_eq!(output.code, Some(2), "{args:?}: {}", output.stderr);
        assert_eq!(output.stdout, "");
        assert_ne!(output.stderr, "");
    }
}

#[tokio::test]
async fn a_wrong_setting_outside_the_command_line_exits_1() {
    let runtime_dir = tempfile::tempdir().unwrap();

    let output = lw_with(
        runtime_dir.path(),
        &["--repo", REPO, "status"],
        &[("LINEWRIGHT_POLL_INTERVAL", "often")],
    )
    .await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(output.stderr.contains("poll-interval"), "{}", output.stderr);
}

#[tokio::test]
async fn runs_gives_the_same_answer_with_serve_running_and_without() {
    let answering = Answering::start(with_issue_for_triage().await, false).await;
    let work_dir = answering._dir.path().join("work");
    let work_dir = work_dir.to_str().unwrap();
    let args = ["--work-dir", work_dir, "runs", "1"];
    eventually_async(|| async { answering.lw(&args).await.stdout.contains("done") }).await;

    let served = [
        answering.lw(&args).await,
        answering.lw(&[&args[..], &["--json"]].concat()).await,
    ];
    let Answering {
        serving,
        api,
        runtime_dir,
        _dir,
        ..
    } = answering;
    within(STOPPING, "the API stops", api.stop()).await;
    within(STOPPING, "serving stops", serving.stop()).await;
    let read = [
        lw(runtime_dir.path(), &args).await,
        lw(runtime_dir.path(), &[&args[..], &["--json"]].concat()).await,
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
async fn a_run_whose_serve_died_is_listed_as_interrupted() {
    let world = with_issue_for_triage().await;
    world.harness.takes(Duration::from_mins(1));
    let answering = Answering::start(world, false).await;
    let work_dir = answering._dir.path().join("work");
    let work_dir = work_dir.to_str().unwrap();
    let args = ["--work-dir", work_dir, "runs"];
    eventually_async(|| async { !answering.lw(&args).await.stdout.is_empty() }).await;
    let Answering {
        serving,
        api,
        runtime_dir,
        _dir,
        ..
    } = answering;

    serving.kill().await;
    within(STOPPING, "the API stops", api.stop()).await;

    let output = lw(runtime_dir.path(), &args).await;
    let line = output.stdout.lines().next().unwrap_or_default();
    assert!(
        line.starts_with("1/triage-1: interrupted — #1 triage, started 2026-10-01 12:00:"),
        "{}",
        output.stdout
    );
    assert!(line.ends_with(" UTC"), "no duration: {line}");
}

#[tokio::test]
async fn explain_shows_where_the_item_is_when_serve_last_read_it_and_what_linewright_waits_for() {
    let answering = Answering::start(with_ticket_and_automation().await, false).await;

    let output = answering.lw(&["explain", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{}", output.stdout);
    assert_eq!(lines[0], "#1: Do the thing");
    assert_eq!(lines[1], "state: with the maintainer");
    assert!(
        lines[2]
            .strip_prefix("last read: 2026-10-01 ")
            .is_some_and(|rest| rest.ends_with(" UTC")),
        "{}",
        output.stdout
    );
    assert_eq!(
        lines[3],
        "waiting for: the maintainer: to label it `ready-for-agent` or `needs-triage`"
    );
    answering.stop().await;
}

#[tokio::test]
async fn explain_of_an_item_linewright_leaves_alone_says_why_as_serve_reads_it_now() {
    let world = with_ticket_and_automation().await;
    let closed = world.store.open_issue(
        "maintainer",
        "Old idea",
        "Maybe.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as_completed(closed).unwrap();
    let answering = Answering::start(world, false).await;

    let output = answering.lw(&["explain", &closed.0.to_string()]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{}", output.stdout);
    assert_eq!(lines[0], format!("{closed}: Old idea"));
    assert_eq!(lines[1], "state: closed issue");
    assert_eq!(lines[3], "left alone: it is closed");
    answering.stop().await;
}

#[tokio::test]
async fn explain_of_a_number_with_no_item_exits_4_saying_so() {
    let answering = Answering::start(World::new().await, false).await;

    let output = answering.lw(&["explain", "7"]).await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("#7"), "{}", output.stderr);
    answering.stop().await;
}

#[tokio::test]
async fn logs_gives_the_same_answer_with_serve_running_and_without() {
    let world = with_issue_for_triage().await;
    world
        .harness
        .on(linewright::reconcile::WorkKind::Triage, |call| {
            std::fs::write(
                call.spec.record.join(linewright::transcript::OUTPUT),
                include_str!("fixtures/transcript/turn-1.jsonl"),
            )
            .unwrap();
            Ok(serde_json::json!({
                "outcome": "ready-for-agent",
                "category": "bug",
                "comment": "## Agent Brief\n\n**Summary:** It crashes."
            })
            .to_string())
        });
    let answering = Answering::start(world, false).await;
    let work_dir = answering._dir.path().join("work");
    let work_dir = work_dir.to_str().unwrap();
    let runs = ["--work-dir", work_dir, "runs", "1"];
    eventually_async(|| async { answering.lw(&runs).await.stdout.contains("done") }).await;
    let ways: [&[&str]; 4] = [
        &["logs", "1"],
        &["logs", "1", "--verbose"],
        &["logs", "--run", "1/triage-1", "--json"],
        &["logs", "1", "--run", "1/triage-2"],
    ];
    let commands: Vec<Vec<&str>> = ways
        .iter()
        .map(|way| [&["--work-dir", work_dir][..], way].concat())
        .collect();

    let mut served = vec![];
    for command in &commands {
        served.push(answering.lw(command).await);
    }
    let Answering {
        serving,
        api,
        runtime_dir,
        _dir,
        ..
    } = answering;
    within(STOPPING, "the API stops", api.stop()).await;
    within(STOPPING, "serving stops", serving.stop()).await;
    let mut read = vec![];
    for command in &commands {
        read.push(lw(runtime_dir.path(), command).await);
    }

    for (from_serve, from_records) in served.iter().zip(&read) {
        assert_eq!(from_serve.code, from_records.code, "{}", from_serve.stderr);
        assert_eq!(from_serve.stdout, from_records.stdout);
    }
    assert_eq!(served[0].code, Some(0), "{}", served[0].stderr);
    assert!(
        served[0].stdout.starts_with(
            "run: 1/triage-1\nsession 1: started 2026-10-13 10:09:08 UTC\n 0:03  Bash("
        ),
        "{}",
        served[0].stdout
    );
    assert_eq!(served[3].code, Some(4), "{}", served[3].stderr);
    assert!(
        served[3].stderr.contains("there is no run 1/triage-2"),
        "{}",
        served[3].stderr
    );
}

fn line_starting<'a>(text: &'a str, start: &str) -> Option<&'a str> {
    text.lines().find(|line| line.starts_with(start))
}

fn runs_in_flight(answer: &str) -> usize {
    let answer: serde_json::Value = serde_json::from_str(answer).unwrap_or_default();
    answer["runs"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn status_and_explain_show_the_run_in_flight_until_it_ends() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let answering = Answering::start(world, false).await;
    eventually_async(|| async {
        runs_in_flight(&answering.lw(&["status", "--json"]).await.stdout) == 1
    })
    .await;

    let status = answering.lw(&["status"]).await;
    let explained = answering.lw(&["explain", "1"]).await;
    let status_json = answering.lw(&["status", "--json"]).await;
    let explain_json = answering.lw(&["explain", "1", "--json"]).await;
    drop(agent);
    eventually_async(|| async {
        runs_in_flight(&answering.lw(&["status", "--json"]).await.stdout) == 0
    })
    .await;
    let ended = answering.lw(&["explain", "1", "--json"]).await;

    assert!(
        line_starting(&status.stdout, "1/triage-1: ").is_some(),
        "{}",
        status.stdout
    );
    assert!(
        line_starting(&explained.stdout, "run in flight: 1/triage-1: ").is_some(),
        "{}",
        explained.stdout
    );
    let status_json: serde_json::Value = serde_json::from_str(&status_json.stdout).unwrap();
    let explain_json: serde_json::Value = serde_json::from_str(&explain_json.stdout).unwrap();
    assert_eq!(status_json["runs"][0]["id"], "1/triage-1");
    assert_eq!(explain_json["run_in_flight"]["id"], "1/triage-1");
    let ended: serde_json::Value = serde_json::from_str(&ended.stdout).unwrap();
    assert_eq!(ended["run_in_flight"], serde_json::Value::Null, "{ended}");
    answering.stop().await;
}

struct Watching {
    child: tokio::process::Child,
    stdout: std::sync::Arc<std::sync::Mutex<String>>,
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Watching {
    fn start(runtime_dir: &Path, args: &[&str]) -> Watching {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_lw"))
            .args(["--repo", REPO])
            .args(args)
            .env("XDG_RUNTIME_DIR", runtime_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdout = gathered(child.stdout.take().unwrap());
        let stderr = gathered(child.stderr.take().unwrap());
        Watching {
            child,
            stdout,
            stderr,
        }
    }

    fn shown(&self) -> String {
        self.stdout.lock().unwrap().clone()
    }

    async fn shows(&self, text: &str) {
        let shown = || self.shown().contains(text);
        let waited = tokio::time::timeout(Duration::from_secs(10), async {
            while !shown() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(waited.is_ok(), "{text:?} not in\n{}", self.shown());
    }

    async fn exits(mut self) -> (Option<i32>, String, String) {
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("it exits")
            .unwrap();
        // What it printed last.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let stderr = self.stderr.lock().unwrap().clone();
        (status.code(), self.shown(), stderr)
    }
}

fn gathered(
    mut from: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> std::sync::Arc<std::sync::Mutex<String>> {
    use tokio::io::AsyncReadExt;
    let gathered = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    tokio::spawn({
        let gathered = gathered.clone();
        async move {
            let mut buffer = [0; 4096];
            while let Ok(n @ 1..) = from.read(&mut buffer).await {
                let read = String::from_utf8_lossy(&buffer[..n]).into_owned();
                gathered.lock().unwrap().push_str(&read);
            }
        }
    });
    gathered
}

pub(crate) fn turn_1() -> Vec<String> {
    include_str!("fixtures/transcript/turn-1.jsonl")
        .lines()
        .map(|line| format!("{line}\n"))
        .collect()
}

#[tokio::test]
async fn logs_follow_shows_the_transcript_so_far_then_each_entry_as_the_agent_produces_it() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let answering = Answering::start(world, false).await;
    let turn = turn_1();
    // The marker, the start event and the first Bash call, with its result.
    agent.writes(&turn[..4].concat());
    eventually_async(|| async {
        answering
            .lw(&["logs", "1"])
            .await
            .stdout
            .contains("Bash(ls;")
    })
    .await;

    let follow = Watching::start(answering.runtime_dir.path(), &["logs", "1", "--follow"]);
    let json = Watching::start(
        answering.runtime_dir.path(),
        &["logs", "1", "--follow", "--json", "--verbose"],
    );
    follow.shows("→ exit 0, 19 lines\n").await;
    json.shows("\"exit 0, 19 lines\"").await;
    let so_far = follow.shown();
    // The second Bash call, its result still to come.
    agent.writes(&turn[4..6].concat());
    follow.shows("2>&1) → running\n").await;
    json.shows("\"running\"").await;
    let before_its_result = follow.shown();
    let running = answering
        .lw(&["logs", "1", "--json", "--verbose"])
        .await
        .stdout;
    agent.writes(&turn[6]);
    follow.shows("2>&1) → exit 2\n").await;
    json.shows("\"exit 2\"").await;
    let ended = answering
        .lw(&["logs", "1", "--json", "--verbose"])
        .await
        .stdout;
    agent.writes(&turn[7..].concat());
    follow.shows(" 2:59  {\"outcome\"").await;
    drop(agent);
    let (code, shown, _) = follow.exits().await;
    let (_, json, _) = json.exits().await;

    assert_eq!(
        so_far,
        "run: 1/triage-1\n\
         session 1: started 2026-10-13 10:09:08 UTC\n \
         0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit 0, 19 lines\n"
    );
    assert_eq!(
        before_its_result,
        format!(
            "{so_far} 0:14  Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → running\n"
        )
    );
    assert_eq!(code, Some(0));
    assert_eq!(shown.lines().count(), 18, "{shown}");
    assert!(
        shown.contains("\n 2:16  Agent(Standards review) → done, 2 tool uses, 16s\n"),
        "{shown}"
    );
    // `docs/specs/monitoring.md`, logs, Following in JSON and Running in
    // JSON: the second call's objects, as it ran and once it returned, are
    // those `logs --json` gave it then, but for when its end was.
    let second_call = |lines: &str| -> Vec<serde_json::Value> {
        json_objects(lines)
            .into_iter()
            .filter(|line| line["tool_use_id"] == "toolu_01WZp27kcvbZoQtwuKJUvLUt")
            .map(|mut line| {
                line.as_object_mut().unwrap().remove("at");
                line.as_object_mut().unwrap().remove("elapsed_seconds");
                line
            })
            .collect()
    };
    let followed = second_call(&json);
    assert_eq!(followed.len(), 2, "{json}");
    assert_eq!(
        followed[0]["error"],
        serde_json::Value::Null,
        "{followed:#?}"
    );
    assert_eq!(
        followed[0]["result"],
        serde_json::Value::Null,
        "{followed:#?}"
    );
    assert_eq!(followed[..1], second_call(&running));
    assert_eq!(followed[1..], second_call(&ended));
    answering.stop().await;
}

/// Asks the `serve` in `runtime_dir` for `resource` as server-sent events,
/// after the event `last_id` if one is given, and reads the first `wanted`
/// events, or all of them when `None`, until the stream ends.
async fn events(
    runtime_dir: &Path,
    resource: &str,
    last_id: Option<&str>,
    wanted: Option<usize>,
) -> Vec<api::ServerEvent> {
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let reading = tokio::spawn({
        let dir = socket_dir_in(runtime_dir);
        let resource = resource.to_string();
        let last_id = last_id.map(str::to_string);
        async move {
            api::get_events(&dir, REPO, &resource, last_id.as_deref(), |event| {
                let _ = sent.send(event);
            })
            .await
            .unwrap();
        }
    });
    let mut read = vec![];
    let collecting = async {
        while wanted.is_none_or(|wanted| read.len() < wanted)
            && let Some(event) = received.recv().await
        {
            read.push(event);
        }
    };
    tokio::time::timeout(Duration::from_secs(10), collecting)
        .await
        .expect("the events");
    reading.abort();
    read
}

fn event(id: Option<&str>, event: &str, data: &str) -> api::ServerEvent {
    api::ServerEvent {
        id: id.map(str::to_string),
        event: event.to_string(),
        data: data.to_string(),
    }
}

#[tokio::test]
async fn the_api_streams_a_transcript_as_events_that_a_reader_resumes_by_event_id() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let answering = Answering::start(world, false).await;
    let runtime_dir = answering.runtime_dir.path();
    let turn = turn_1();
    agent.writes(&turn[..4].concat());
    eventually_async(|| async {
        answering
            .lw(&["logs", "1"])
            .await
            .stdout
            .contains("Bash(ls;")
    })
    .await;

    let first = events(runtime_dir, "/api/logs/follow?item=1", None, Some(2)).await;
    agent.writes(&turn[4..].concat());
    drop(agent);
    let resumed = events(
        runtime_dir,
        "/api/logs/follow?item=1",
        first[1].id.as_deref(),
        None,
    )
    .await;
    let whole = events(runtime_dir, "/api/logs/follow?item=1", None, None).await;

    assert_eq!(
        first,
        [
            event(
                Some("1/triage-1:1.0.1"),
                "line",
                r#"{"type":"run","run":"1/triage-1","sessions":1}"#
            ),
            event(
                Some("1/triage-1:1.0.2"),
                "line",
                r#"{"type":"session","session":1,"started_at":"2026-10-13T10:09:08Z"}"#
            ),
        ]
    );
    assert_eq!(resumed[0].id.as_deref(), Some("1/triage-1:1.3.1"));
    assert_eq!([&first[..], &resumed[..]].concat(), whole);
    let (end, lines) = whole.split_last().unwrap();
    assert_eq!(lines.len(), 19);
    assert_eq!(*end, event(None, "end", r#"{"run":"1/triage-1"}"#));
    answering.stop().await;
}

#[tokio::test]
async fn logs_reads_the_records_when_serve_hangs_up_unanswered_as_it_stops() {
    let (agent, answering, runtime_dir) = triage_under_way().await;
    let served = answering.lw(&["logs", "1"]).await.stdout;
    let stopped = stops_as_asked(
        runtime_dir.path(),
        answering.runtime_dir.path(),
        Stops::AtRequest(1),
    );

    let work_dir = answering._dir.path().join("work");
    let output = lw(
        runtime_dir.path(),
        &["--work-dir", work_dir.to_str().unwrap(), "logs", "1"],
    )
    .await;

    stopped.await.unwrap();
    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, served);
    drop(agent);
    answering.stop().await;
}

#[tokio::test]
async fn logs_follow_carries_on_from_the_records_when_serve_hangs_up_unanswered_as_it_stops() {
    let (agent, answering, runtime_dir) = triage_under_way().await;
    // Asked for the transcript so far, then to follow it.
    let stopped = stops_as_asked(
        runtime_dir.path(),
        answering.runtime_dir.path(),
        Stops::AtRequest(2),
    );

    let follow = follow_through(&answering, runtime_dir.path());
    stopped.await.unwrap();
    agent.writes(&turn_1()[4..].concat());
    drop(agent);

    shows_the_rest_once_serve_stops(&answering, follow).await;
    answering.stop().await;
}

#[tokio::test]
async fn logs_follow_carries_on_from_the_records_when_serve_hangs_up_part_way_as_it_stops() {
    let (agent, answering, runtime_dir) = triage_under_way().await;
    let stopping = CancellationToken::new();
    let stopped = stops_as_asked(
        runtime_dir.path(),
        answering.runtime_dir.path(),
        Stops::OnceCancelled(stopping.clone()),
    );

    let follow = follow_through(&answering, runtime_dir.path());
    follow.shows("→ exit 0, 19 lines\n").await;
    let turn = turn_1();
    agent.writes(&turn[4..6].concat());
    follow.shows("→ running\n").await;
    stopping.cancel();
    stopped.await.unwrap();
    agent.writes(&turn[6..].concat());
    drop(agent);

    shows_the_rest_once_serve_stops(&answering, follow).await;
    answering.stop().await;
}

async fn triage_under_way() -> (HeldTurn, Answering, tempfile::TempDir) {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let answering = Answering::start(world, false).await;
    agent.writes(&turn_1()[..4].concat());
    eventually_async(|| async {
        answering
            .lw(&["logs", "1"])
            .await
            .stdout
            .contains("Bash(ls;")
    })
    .await;
    (agent, answering, tempfile::tempdir().unwrap())
}

fn follow_through(answering: &Answering, runtime_dir: &Path) -> Watching {
    let work_dir = answering._dir.path().join("work");
    Watching::start(
        runtime_dir,
        &[
            "--work-dir",
            work_dir.to_str().unwrap(),
            "logs",
            "1",
            "--follow",
        ],
    )
}

async fn shows_the_rest_once_serve_stops(answering: &Answering, follow: Watching) {
    let (code, shown, stderr) = follow.exits().await;

    assert_eq!(code, Some(0), "{stderr}");
    assert!(shown.contains(") → exit 2\n"), "{shown}");
    let logs = answering.lw(&["logs", "1"]).await.stdout;
    assert_eq!(shown.lines().last(), logs.lines().last(), "{shown}");
}

fn stand_in_socket(runtime_dir: &Path) -> (std::path::PathBuf, tokio::net::UnixListener) {
    let path = api::socket_path(&socket_dir_in(runtime_dir), REPO);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    (path, listener)
}

enum Stops {
    /// From 1.
    AtRequest(usize),
    OnceCancelled(CancellationToken),
}

/// Passes requests on to the `serve` in `serving_dir` until it stops, then,
/// as a stopping `serve` does, removes its socket and hangs up on the
/// requests still being answered.
fn stops_as_asked(
    runtime_dir: &Path,
    serving_dir: &Path,
    stops: Stops,
) -> tokio::task::JoinHandle<()> {
    let (path, listener) = stand_in_socket(runtime_dir);
    let serving = api::socket_path(&socket_dir_in(serving_dir), REPO);
    tokio::spawn(async move {
        let mut passing = tokio::task::JoinSet::new();
        let mut asked = 0;
        let cancelled = async {
            match &stops {
                Stops::OnceCancelled(stopping) => stopping.cancelled().await,
                Stops::AtRequest(_) => std::future::pending().await,
            }
        };
        tokio::pin!(cancelled);
        let unanswered = loop {
            let (mut request, _) = tokio::select! {
                accepted = listener.accept() => accepted.unwrap(),
                () = &mut cancelled => break None,
            };
            asked += 1;
            if matches!(stops, Stops::AtRequest(nth) if nth == asked) {
                break Some(request);
            }
            let mut answering = tokio::net::UnixStream::connect(&serving).await.unwrap();
            passing.spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut request, &mut answering).await;
            });
        };
        std::fs::remove_file(&path).unwrap();
        drop(listener);
        passing.shutdown().await;
        drop(unanswered);
    })
}

#[tokio::test]
async fn logs_follow_says_why_and_exits_1_when_serve_cannot_follow_the_run() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let answering = Answering::start(world, false).await;
    let turn = turn_1();
    agent.writes(&turn[..4].concat());
    eventually_async(|| async {
        answering
            .lw(&["logs", "1"])
            .await
            .stdout
            .contains("Bash(ls;")
    })
    .await;
    let follow = Watching::start(answering.runtime_dir.path(), &["logs", "1", "--follow"]);
    follow.shows("→ exit 0, 19 lines\n").await;

    let [record] = &files_named(answering._dir.path(), "items/1/triage-1.json")[..] else {
        panic!("one record of 1/triage-1");
    };
    std::fs::write(record, "not a record").unwrap();
    let (code, _, stderr) = follow.exits().await;

    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("reading the record of 1/triage-1"),
        "{stderr}"
    );
    drop(agent);
    answering.stop().await;
}

/// A triage held open, so its run stays in flight until the turn returned
/// is dropped, and `serve` answering with it in flight.
async fn triage_held() -> (HeldTurn, Answering) {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let answering = Answering::start(world, false).await;
    eventually_async(|| async {
        answering
            .lw(&["status"])
            .await
            .stdout
            .contains("runs in flight: 1 of")
    })
    .await;
    (agent, answering)
}

fn drain_line(status: &str) -> Option<&str> {
    status.lines().find(|line| line.starts_with("drain: "))
}

#[tokio::test]
async fn drain_names_the_run_in_flight_and_again_as_it_ends_and_returns_once_serve_has_exited() {
    let (agent, answering) = triage_held().await;
    let runtime_dir = answering.runtime_dir.path().to_path_buf();
    let exited = answering.exits_once_served();

    let drain = Watching::start(&runtime_dir, &["drain"]);
    drain.shows("waiting for: 1/triage-1\n").await;
    let before_it_ends = drain.shown();
    drop(agent);
    let (code, shown, stderr) = drain.exits().await;

    assert_eq!(before_it_ends, "drain: begun\nwaiting for: 1/triage-1\n");
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        shown,
        "drain: begun\nwaiting for: 1/triage-1\nended: 1/triage-1\nserve: exited\n"
    );
    assert_eq!(
        within(STOPPING, "serve exits", exited).await.unwrap(),
        ServeEnd::Drained
    );
}

#[tokio::test]
async fn drain_no_wait_returns_once_serve_has_taken_it_and_serve_goes_on_draining() {
    let (agent, answering) = triage_held().await;

    let output = answering.lw(&["drain", "--no-wait"]).await;
    let status = answering.lw(&["status"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "drain: begun\nwaiting for: 1/triage-1\n");
    assert!(
        drain_line(&status.stdout).is_some_and(|line| line.starts_with("drain: under way since ")),
        "{}",
        status.stdout
    );
    assert!(!answering.serving.has_ended());
    let exited = answering.exits_once_served();
    drop(agent);
    assert_eq!(
        within(STOPPING, "serve exits", exited).await.unwrap(),
        ServeEnd::Drained
    );
}

#[tokio::test]
async fn killing_a_waiting_drain_leaves_serve_draining() {
    let (agent, answering) = triage_held().await;
    let mut drain = Watching::start(answering.runtime_dir.path(), &["drain"]);
    drain.shows("waiting for: 1/triage-1\n").await;

    drain.child.kill().await.unwrap();
    let status = answering.lw(&["status"]).await;

    assert!(drain_line(&status.stdout).is_some(), "{}", status.stdout);
    assert!(!answering.serving.has_ended());
    let exited = answering.exits_once_served();
    drop(agent);
    assert_eq!(
        within(STOPPING, "serve exits", exited).await.unwrap(),
        ServeEnd::Drained
    );
}

#[tokio::test]
async fn drain_of_serve_draining_after_an_earlier_drain_says_so_and_waits_for_it_to_exit() {
    let (agent, answering) = triage_held().await;
    let earlier = answering.lw(&["drain", "--no-wait"]).await;
    assert_eq!(earlier.code, Some(0), "{}", earlier.stderr);
    let runtime_dir = answering.runtime_dir.path().to_path_buf();
    let exited = answering.exits_once_served();

    let drain = Watching::start(&runtime_dir, &["drain"]);
    drain.shows("waiting for: 1/triage-1\n").await;
    drop(agent);
    let (code, shown, stderr) = drain.exits().await;

    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        shown.lines().next(),
        Some("drain: already under way"),
        "{shown}"
    );
    assert_eq!(
        within(STOPPING, "serve exits", exited).await.unwrap(),
        ServeEnd::Drained
    );
}

#[tokio::test]
async fn drain_of_serve_stopping_says_so_and_waits_for_it_to_exit() {
    let (agent, answering) = triage_held().await;
    let Answering {
        serving,
        api,
        ends,
        runtime_dir,
        ..
    } = answering;
    ends.on(Event::Signal(Signal::Quit));
    // Its runs cancelled, `serve` answers until it exits.
    within(STOPPING, "serving stops", serving.finished()).await;
    drop(agent);

    let drain = Watching::start(runtime_dir.path(), &["drain"]);
    drain.shows("stop: already under way\n").await;
    within(STOPPING, "the API stops", api.stop()).await;
    let (code, shown, stderr) = drain.exits().await;

    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        shown.lines().next(),
        Some("stop: already under way"),
        "{shown}"
    );
}

#[tokio::test]
async fn drain_returns_once_serve_has_exited_though_another_has_started_since() {
    let (agent, answering) = triage_held().await;
    let restarted = World::new().await;
    restarted.clock.advance(chrono::TimeDelta::hours(1));
    let drain = Watching::start(answering.runtime_dir.path(), &["drain"]);
    drain.shows("waiting for: 1/triage-1\n").await;
    let Answering {
        serving,
        api,
        runtime_dir,
        _dir,
        ..
    } = answering;

    drop(agent);
    within(STOPPING, "serve drains", serving.finished()).await;
    within(STOPPING, "the API stops", api.stop()).await;
    let claimed = within(STOPPING, "the socket is claimed again", async {
        loop {
            // A process another test forks may hold the lock a moment.
            match api::claim(&socket_dir_in(runtime_dir.path()), REPO) {
                Ok(claimed) => return claimed,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await;
    let restarted = Answering::start_claimed(restarted, false, runtime_dir, claimed).await;
    let (code, shown, stderr) = drain.exits().await;

    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        shown,
        "drain: begun\nwaiting for: 1/triage-1\nended: 1/triage-1\nserve: exited\n"
    );
    restarted.stop().await;
}

#[tokio::test]
async fn a_drain_whose_run_outlasts_its_timeout_becomes_a_stop_at_the_deadline() {
    let (_agent, answering) = triage_held().await;
    let work_dir = answering._dir.path().join("work");
    let Answering {
        serving,
        api,
        clock,
        runtime_dir,
        _dir,
        ..
    } = answering;
    let drain = Watching::start(runtime_dir.path(), &["drain", "--timeout", "1h"]);
    drain.shows("waiting for: 1/triage-1\n").await;

    clock.advance(chrono::TimeDelta::hours(2));
    let end = within(STOPPING, "serve stops", serving.finished()).await;
    let runs = lw(
        runtime_dir.path(),
        &["--work-dir", work_dir.to_str().unwrap(), "runs"],
    )
    .await;
    within(STOPPING, "the API stops", api.stop()).await;
    let (code, shown, stderr) = drain.exits().await;

    assert_eq!(end, ServeEnd::Stop);
    assert!(
        runs.stdout.starts_with("1/triage-1: cancelled — "),
        "{}",
        runs.stdout
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        shown,
        "drain: begun\nwaiting for: 1/triage-1\ndrain: became a stop at its deadline\n\
         ended: 1/triage-1\nserve: exited\n"
    );
}

#[tokio::test]
async fn drain_waits_while_serve_still_holds_its_claim_with_its_socket_refusing() {
    use tokio::io::AsyncWriteExt;
    let answering = Answering::start(World::new().await, false).await;
    let runtime_dir = answering.runtime_dir.path().to_path_buf();
    let dir = socket_dir_in(&runtime_dir);
    let socket = api::socket_path(&dir, REPO);
    // Half a request, which `serve` waits for the rest of as it stops.
    let mut open = tokio::net::UnixStream::connect(&socket).await.unwrap();
    open.write_all(b"GET /api/status HTTP/1.1\r\nHost: localhost\r\n")
        .await
        .unwrap();
    let drain = Watching::start(&runtime_dir, &["drain"]);
    drain.shows("drain: begun\n").await;
    let Answering {
        serving,
        api,
        runtime_dir: _runtime_dir,
        _dir,
        ..
    } = answering;
    within(STOPPING, "serve drains", serving.finished()).await;
    let stopped = tokio::spawn(api.stop());
    eventually(|| {
        matches!(
            std::os::unix::net::UnixStream::connect(&socket),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused
        )
    })
    .await;

    // Four times as long as `drain` waits between asking.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let while_stopping = drain.shown();
    let claimed_while_stopping = api::claim(&dir, REPO);
    assert!(socket.exists());
    drop(open);
    let (code, shown, stderr) = drain.exits().await;
    let claimed = api::claim(&dir, REPO);

    assert!(
        matches!(
            claimed_while_stopping,
            Err(ClaimError::AlreadyServing { .. })
        ),
        "the claim was released while stopping"
    );
    assert_eq!(while_stopping, "drain: begun\n");
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(shown, "drain: begun\nserve: exited\n");
    assert!(claimed.is_ok(), "{:?}", claimed.err());
    within(STOPPING, "the API stops", stopped).await.unwrap();
}

#[tokio::test]
async fn status_names_a_file_in_the_repositorys_prompts_that_is_not_one_of_linewrights() {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/prompts/implment.md", "Implement it.\n")],
            "Tune the implementing",
        )
        .await
        .unwrap();
    let answering = Answering::start(world, false).await;

    let output = answering.lw(&["status"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert!(
        output.stdout.lines().any(|line| line
            == "repository file Linewright cannot use: `.linewright/prompts/implment.md`: \
                not one of Linewright's prompts"),
        "{}",
        output.stdout
    );
    answering.stop().await;
}
