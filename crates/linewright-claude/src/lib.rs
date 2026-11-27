use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use async_trait::async_trait;
use linewright_core::child;
use linewright_core::cost::{Figures, Spend, Tokens, TurnSpend, Usd};
use linewright_core::domain::Timestamp;
use linewright_core::harness::{
    GhReach, Harness, HarnessSettings, Program, Reported, SessionError, SessionSpec, Turn,
    UsageLimit, Variables,
};
use linewright_core::redaction::Redaction;
use linewright_core::session_env::{GH_CONFIG_DIR, Given, TEMPORARY_DIRECTORY};
use linewright_core::store::TOKEN_VARIABLES;
use linewright_core::transcript::{self, StartedBy};
use linewright_core::work_settings::{Effort, Model};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Claude Code, running sessions with its settings.
pub struct ClaudeCode(pub HarnessSettings);

/// Without it Claude Code picks the operator's `$SHELL`. It still sources
/// the host's `~/.bashrc`, if there is one, and has no switch to stop that,
/// so a session is not insulated from it. Docs:
/// <https://code.claude.com/docs/en/settings#environment-variables>.
const SHELL: &str = "CLAUDE_CODE_SHELL";
/// Ranked above every other choice of effort, `--effort` and the settings
/// included: <https://code.claude.com/docs/en/model-config#adjust-effort-level>.
const EFFORT: &str = "CLAUDE_CODE_EFFORT_LEVEL";
/// Unlike `DISABLE_AUTOUPDATER`, also blocks `claude update`. Docs:
/// <https://code.claude.com/docs/en/env-vars>.
const NO_UPDATES: &str = "DISABLE_UPDATES";
/// Has every start-up failure end in a `result` naming its
/// `startup_failure_reason`, where most would otherwise be told on standard
/// error alone: <https://code.claude.com/docs/en/env-vars>.
const STARTUP_FAILURE_RESULTS: &str = "CLAUDE_CODE_STARTUP_FAILURE_RESULTS";
/// The variables Claude Code reads a credential from, of those in the
/// namespaces the table of sessions' variables passes. Docs:
/// <https://code.claude.com/docs/en/env-vars>.
const CREDENTIAL_VARIABLES: [&str; 8] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_AWS_API_KEY",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
];
/// The variables Claude Code reads request headers from, one `Name: value`
/// per line, of those in the namespaces the table of sessions' variables
/// passes. Docs: <https://code.claude.com/docs/en/env-vars>.
const HEADER_VARIABLES: [&str; 1] = ["ANTHROPIC_CUSTOM_HEADERS"];

/// A variable Linewright sets for sessions itself.
#[derive(Clone, Copy)]
enum Own {
    TemporaryDirectory,
    Shell,
    NoUpdates,
    Effort,
    GhToken,
    GithubToken,
    StartupFailureResults,
    GhConfigDir,
}

impl Own {
    /// Those `session-env` may not set.
    const NOT_SET_BY_OPERATOR: [Own; 7] = [
        Own::TemporaryDirectory,
        Own::Shell,
        Own::NoUpdates,
        Own::Effort,
        Own::GhToken,
        Own::GithubToken,
        Own::StartupFailureResults,
    ];

    const ALL: [Own; Own::NOT_SET_BY_OPERATOR.len() + 1] = {
        let mut all = [Own::GhConfigDir; Own::NOT_SET_BY_OPERATOR.len() + 1];
        let mut i = 0;
        while i < Own::NOT_SET_BY_OPERATOR.len() {
            all[i] = Own::NOT_SET_BY_OPERATOR[i];
            i += 1;
        }
        all
    };

    const fn name(self) -> &'static str {
        match self {
            Own::TemporaryDirectory => TEMPORARY_DIRECTORY,
            Own::Shell => SHELL,
            Own::NoUpdates => NO_UPDATES,
            Own::Effort => EFFORT,
            Own::GhToken => TOKEN_VARIABLES[0],
            Own::GithubToken => TOKEN_VARIABLES[1],
            Own::StartupFailureResults => STARTUP_FAILURE_RESULTS,
            Own::GhConfigDir => GH_CONFIG_DIR,
        }
    }
}

/// Claude Code's settings that Linewright sets for a session itself.
const SETTINGS: [&str; 4] = [SHELL, NO_UPDATES, EFFORT, STARTUP_FAILURE_RESULTS];

/// Claude Code's program.
pub const PROGRAM: Program = Program {
    name: "claude",
    variables: Variables {
        credentials: &CREDENTIAL_VARIABLES,
        headers: &HEADER_VARIABLES,
        settings: &SETTINGS,
    },
};

/// What Linewright sets for a session, over any variable of the same name
/// the session's `environment` gives it: of `Own::ALL`, those it has a
/// value for. `gh_config` is the configuration of `gh` a gate serves, if
/// any.
fn set_for_session<'a>(
    scratch: &'a Path,
    bash: &'a Path,
    github_token: &'a str,
    effort: Option<Effort>,
    gh_config: Option<&'a Path>,
) -> Vec<(&'static str, &'a OsStr)> {
    Own::ALL
        .into_iter()
        .filter_map(|own| {
            let value = match own {
                Own::TemporaryDirectory => Some(scratch.as_os_str()),
                Own::Shell => Some(bash.as_os_str()),
                Own::NoUpdates | Own::StartupFailureResults => Some(OsStr::new("1")),
                Own::Effort => effort.map(|effort| OsStr::new(effort.name())),
                Own::GhToken | Own::GithubToken => Some(OsStr::new(github_token)),
                Own::GhConfigDir => gh_config.map(Path::as_os_str),
            };
            Some((own.name(), value?))
        })
        .collect()
}

/// <https://code.claude.com/docs/en/headless#get-structured-output>
#[derive(Deserialize)]
struct ResultEvent {
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    terminal_reason: Option<String>,
    #[serde(default)]
    api_error_status: Option<serde_json::Value>,
    #[serde(default, rename = "modelUsage")]
    model_usage: Option<serde_json::Value>,
    #[serde(default)]
    startup_failure_reason: Option<String>,
    #[serde(default)]
    errors: serde_json::Value,
}

/// What a turn's last `result` says of how it ended.
#[derive(Debug, PartialEq)]
enum Said {
    Finished {
        message: String,
    },
    /// Any `startup_failure_reason`, known or not:
    /// <https://code.claude.com/docs/en/env-vars>.
    FailedToStart {
        reason: String,
        said: Option<String>,
    },
    ApiFailed {
        said: String,
    },
    /// Any other result, `kind` being its `subtype`.
    Errored {
        kind: String,
        said: Option<String>,
    },
}

impl From<ResultEvent> for Said {
    fn from(event: ResultEvent) -> Said {
        if let Some(reason) = event.startup_failure_reason {
            let first_error = event.errors[0]
                .as_str()
                .and_then(|error| error.lines().next())
                .map(str::to_string);
            return Said::FailedToStart {
                reason,
                said: event.result.filter(|said| !said.is_empty()).or(first_error),
            };
        }
        let api_failed = event.terminal_reason.as_deref() == Some("api_error")
            || event
                .api_error_status
                .as_ref()
                .is_some_and(|status| !status.is_null());
        match event {
            ResultEvent {
                is_error: false,
                result: Some(message),
                ..
            } => Said::Finished { message },
            event if api_failed => Said::ApiFailed {
                said: event.result.unwrap_or(event.subtype),
            },
            event => Said::Errored {
                kind: event.subtype,
                said: event.result,
            },
        }
    }
}

/// Whether the harness's transcript of a session holds the last message of
/// the turn it streamed: the two share the message's `uuid`,
/// <https://code.claude.com/docs/en/agent-sdk/file-checkpointing>.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kept {
    Holds,
    Lacks,
    /// There is no transcript of the session to hold it.
    Absent,
    /// The turn streamed no message of the session's own with a `uuid`.
    NothingToCheck,
}

/// Whether `transcript`, if there is one, holds `last_message`.
fn kept(transcript: Option<&str>, last_message: Option<&str>) -> Kept {
    let Some(last_message) = last_message else {
        return Kept::NothingToCheck;
    };
    let Some(transcript) = transcript else {
        return Kept::Absent;
    };
    let holds = transcript.lines().any(|line| {
        line.contains(last_message)
            && serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|entry| entry["uuid"] == last_message)
    });
    if holds { Kept::Holds } else { Kept::Lacks }
}

/// How a turn's process ended, before its deadline.
struct TurnEnd {
    /// What its last `result`, if any, said.
    said: Option<Said>,
    refusal: Option<UsageLimit>,
    status: std::io::Result<std::process::ExitStatus>,
    transcript: Kept,
    /// The last line the harness wrote to standard error, if any.
    stderr_last_line: Option<String>,
}

/// The turn of `session` that ended as `end` says, or its error.
fn classify(end: TurnEnd, session: String, log_tail: String) -> Result<Turn, SessionError> {
    let with_stderr = |what: String| match &end.stderr_last_line {
        Some(line) => format!("{what}: {line}"),
        None => what,
    };
    let exited_cleanly = end
        .status
        .as_ref()
        .is_ok_and(std::process::ExitStatus::success);
    match end.said {
        Some(Said::Finished { message })
            if exited_cleanly && matches!(end.transcript, Kept::Holds | Kept::NothingToCheck) =>
        {
            Ok(Turn {
                session,
                message,
                log_tail,
            })
        }
        Some(Said::Finished { .. }) if !exited_cleanly => Err(SessionError::Infrastructure(
            with_stderr(match end.status {
                Ok(status) => format!("the harness exited ({status}) after its result"),
                Err(e) => format!("the harness could not be waited for after its result ({e})"),
            }),
        )),
        Some(Said::Finished { .. }) if end.transcript == Kept::Absent => {
            Err(SessionError::Infrastructure(with_stderr(
                "the harness kept no transcript of the session".into(),
            )))
        }
        Some(Said::Finished { .. }) => Err(SessionError::Infrastructure(with_stderr(
            "the harness did not keep all of the session's transcript".into(),
        ))),
        _ if let Some(limit) = end.refusal => Err(SessionError::UsageLimit { limit, session }),
        Some(Said::FailedToStart { reason, said }) => {
            let what = format!("the harness could not start ({reason})");
            Err(SessionError::Infrastructure(match said {
                Some(said) => format!("{what}: {said}"),
                None => with_stderr(what),
            }))
        }
        Some(Said::ApiFailed { said }) => Err(SessionError::Infrastructure(format!(
            "the harness's API failed ({said})"
        ))),
        Some(Said::Errored { kind, said }) => Err(SessionError::Errored {
            kind,
            result: said,
            log_tail,
        }),
        None => Err(SessionError::NoResult {
            why: match end.status {
                Ok(status) => format!("the harness exited ({status}) without a result"),
                Err(e) => format!("the harness could not be waited for: {e}"),
            },
            log_tail,
        }),
    }
}

/// The first version of Claude Code whose `result` gives the session's
/// figures so far, the turns it resumed included, rather than its own
/// process's: <https://code.claude.com/docs/en/agent-sdk/cost-tracking>,
/// <https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md>.
const SESSION_FIGURES_FROM: Version = Version(2, 1, 277);

/// A `result` whose figures may be zeroed:
/// <https://code.claude.com/docs/en/agent-sdk/cost-tracking>.
const ZEROED_FIGURES: &str = "error_during_execution";

/// A model's `costBasis` when Claude Code could not price it:
/// <https://code.claude.com/docs/en/agent-sdk/cost-tracking>.
const UNPRICED: &str = "unknown";

/// A version of Claude Code, as its `init` event names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl std::str::FromStr for Version {
    type Err = ();

    fn from_str(named: &str) -> Result<Version, ()> {
        let mut parts = named.split('.').map(|part| part.parse().map_err(|_| ()));
        let version = Version(
            parts.next().ok_or(())??,
            parts.next().ok_or(())??,
            parts.next().ok_or(())??,
        );
        match parts.next() {
            None => Ok(version),
            Some(_) => Err(()),
        }
    }
}

/// What the harness of `version` says of a turn's spend in its last
/// `result`: <https://code.claude.com/docs/en/agent-sdk/cost-tracking>.
fn turn_spend(version: Option<Version>, result: Option<&ResultEvent>) -> TurnSpend {
    let Some(result) = result.filter(|result| result.subtype != ZEROED_FIGURES) else {
        return TurnSpend::None;
    };
    let Some(usage) = result
        .model_usage
        .as_ref()
        .and_then(serde_json::Value::as_object)
    else {
        return TurnSpend::None;
    };
    let Some(version) = version else {
        return TurnSpend::None;
    };
    let figures = Figures(
        usage
            .iter()
            .map(|(model, used)| (model.clone(), model_spend(used)))
            .collect(),
    );
    if version >= SESSION_FIGURES_FROM {
        TurnSpend::OfSessionSoFar(figures)
    } else {
        TurnSpend::OfTurn(figures)
    }
}

/// What a model used, as a `result`'s `modelUsage` gives it.
fn model_spend(used: &serde_json::Value) -> Spend {
    let count = |kind: &str| used[kind].as_u64();
    let tokens = (|| {
        Some(Tokens {
            input: count("inputTokens")?,
            output: count("outputTokens")?,
            cache_write: count("cacheCreationInputTokens")?,
            cache_read: count("cacheReadInputTokens")?,
        })
    })();
    let usd = if used["costBasis"] == UNPRICED {
        None
    } else {
        used["costUSD"].as_f64().and_then(Usd::new)
    };
    Spend { tokens, usd }
}

/// Claude Code streams it as it refuses the session, and as the limit nears.
#[derive(Deserialize)]
struct RateLimitEvent {
    rate_limit_info: RateLimitInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitInfo {
    status: RateLimitStatus,
    #[serde(default, with = "chrono::serde::ts_seconds_option")]
    resets_at: Option<Timestamp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RateLimitStatus {
    Rejected,
    /// Allowed, or allowed with a warning.
    #[serde(other)]
    Other,
}

impl RateLimitEvent {
    fn refusal(&self) -> Option<UsageLimit> {
        match self.rate_limit_info {
            RateLimitInfo {
                status: RateLimitStatus::Rejected,
                resets_at: Some(resets_at),
            } => Some(UsageLimit { resets_at }),
            _ => None,
        }
    }
}

/// The settings outrank the repository's, so it cannot turn auto-memory back
/// on. claude.ai connectors and the `CLAUDE.md` files above the clone load
/// whatever the setting sources, so the settings turn those off too:
/// <https://code.claude.com/docs/en/agent-sdk/claude-code-features#what-settingsources-does-not-control>,
/// <https://code.claude.com/docs/en/memory>.
///
/// A tool is left out only by denying it by its bare name:
/// <https://code.claude.com/docs/en/permissions>. A `Skill(name)` deny rule
/// leaves the skill listed, so skills are turned off in `skillOverrides`:
/// <https://code.claude.com/docs/en/skills>.
///
/// The session's model and effort go in the settings too, so that the
/// repository's cannot choose another:
/// <https://code.claude.com/docs/en/model-config>. `effortLevel` does not
/// take `max`, which `--effort` and the variable set alone.
///
/// `clone` is the clone's real path, as Claude Code matches `CLAUDE.md`
/// files by their absolute paths:
/// <https://code.claude.com/docs/en/memory#exclude-specific-claude-md-files>.
fn isolation_args(clone: &Path, model: &Model, effort: Effort) -> [String; 4] {
    let mut excludes = Vec::new();
    for dir in clone.ancestors().skip(1) {
        let pattern = escape_glob(dir.to_string_lossy().trim_end_matches('/'));
        excludes.push(format!("{pattern}/*.md"));
        let clone_is_under_its_claude_dir = clone.starts_with(dir.join(".claude"));
        if !clone_is_under_its_claude_dir {
            excludes.push(format!("{pattern}/.claude/**"));
        }
    }
    let mut settings = serde_json::json!({
        "autoMemoryEnabled": false,
        "disableClaudeAiConnectors": true,
        "claudeMdExcludes": excludes,
        "disableBundledSkills": true,
        "disableWorkflows": true,
        "disableRemoteControl": true,
        "disableArtifact": true,
        "skillOverrides": BTreeMap::from(UNUSED_SKILLS.map(|skill| (skill, "off"))),
        "permissions": { "deny": UNUSED_TOOLS },
        "model": model.as_str(),
    });
    if effort != Effort::Max {
        settings["effortLevel"] = effort.name().into();
    }
    [
        "--setting-sources".into(),
        "project,local".into(),
        "--settings".into(),
        settings.to_string(),
    ]
}

/// Bundled skills that `disableBundledSkills` keeps.
const UNUSED_SKILLS: [&str; 3] = ["design", "doctor", "plugin-authoring"];

const UNUSED_TOOLS: [&str; 17] = [
    "AskUserQuestion",
    "CronCreate",
    "CronDelete",
    "CronList",
    "DesignSync",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "ListAgents",
    "NotebookEdit",
    "PushNotification",
    "RemoteTrigger",
    "ReportFindings",
    "ScheduleWakeup",
    "SendMessage",
    "ShareOnboardingGuide",
];

/// `path` as a `claudeMdExcludes` glob that matches it alone:
/// <https://code.claude.com/docs/en/memory#exclude-specific-claude-md-files>.
/// Claude Code's globs treat parentheses as special too, so
/// `globset::escape` won't do.
fn escape_glob(path: &str) -> String {
    path.chars()
        .map(|c| match c {
            '?' | '*' | '[' | ']' | '{' | '}' | '(' | ')' => format!("[{c}]"),
            c => c.to_string(),
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Session<'a> {
    Start(&'a str),
    Resume(&'a str),
}

impl<'a> Session<'a> {
    fn id(self) -> &'a str {
        match self {
            Session::Start(id) | Session::Resume(id) => id,
        }
    }

    fn started_by(self) -> StartedBy {
        match self {
            Session::Start(_) => StartedBy::Brief,
            Session::Resume(_) => StartedBy::FollowUp,
        }
    }

    fn args(self) -> [&'a str; 2] {
        match self {
            Session::Start(id) => ["--session-id", id],
            Session::Resume(id) => ["--resume", id],
        }
    }
}

const TAIL_LINES: usize = 30;

/// What a session said, kept as it streams so a timed-out session still
/// has a log tail.
#[derive(Default)]
struct Log {
    said: VecDeque<String>,
    errors: String,
}

impl Log {
    fn last_error_line(&self) -> Option<String> {
        self.errors
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .map(str::to_string)
    }

    fn tail(&self) -> String {
        let errors = self.errors.trim();
        if errors.is_empty() {
            self.said.iter().cloned().collect::<Vec<_>>().join("\n")
        } else {
            let lines: Vec<&str> = errors.lines().collect();
            lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
        }
    }
}

impl ClaudeCode {
    /// A turn of `session`, with all it keeps and answers redacted. The
    /// output it keeps is redacted line by line, and the turn or error it
    /// answers once more: a result is text of its own, such as JSON, that
    /// can hold a secret escaped once more than its line does. Its start
    /// and its end, with its spend, are reported as they come.
    async fn turn(
        &self,
        spec: &SessionSpec,
        session: Session<'_>,
        prompt: &str,
    ) -> Result<Turn, SessionError> {
        spec.report.report(Reported::TurnStarted);
        let (turn, spend) = self.unredacted_turn(spec, session, prompt).await;
        spec.report.report(Reported::TurnEnded(spend));
        turn.map(|turn| turn.redacted(&self.0.redaction))
            .map_err(|error| error.redacted(&self.0.redaction))
    }

    async fn unredacted_turn(
        &self,
        spec: &SessionSpec,
        session: Session<'_>,
        prompt: &str,
    ) -> (Result<Turn, SessionError>, TurnSpend) {
        let clone = std::fs::canonicalize(&spec.dir).unwrap_or_else(|_| spec.dir.clone());
        let environment = self.environment(
            &spec.environment,
            &spec.scratch,
            Some(spec.effort),
            &spec.gh,
        );
        let mut command = child::contained(&self.0.container, &self.0.program);
        command
            .args([
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "bypassPermissions",
            ])
            .args(isolation_args(&clone, &spec.model, spec.effort))
            .arg("--plugin-dir")
            .arg(&spec.plugin)
            .args(session.args())
            .current_dir(&spec.dir)
            .env_clear()
            .envs(environment.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(["--model", spec.model.as_str()])
            .args(["--effort", spec.effort.name()]);
        let mut child = match command.spawn().await {
            Ok(child) => child,
            Err(e) => {
                let error =
                    SessionError::Infrastructure(format!("starting {}: {e}", self.0.program));
                return (Err(error), TurnSpend::OfTurn(Figures::default()));
            }
        };
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let log = Mutex::new(Log::default());
        let spend = Mutex::new(TurnSpend::None);
        let feed = async {
            let _ = stdin.write_all(prompt.as_bytes()).await;
            drop(stdin);
        };
        let output_path = spec.record.join(transcript::OUTPUT);
        let read_stdout = async {
            let mut output = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&output_path)
                .await
                .ok();
            if let Some(file) = output.as_mut() {
                let marker = transcript::turn_marker(self.0.clock.now(), session.started_by());
                let _ = file.write_all(format!("{marker}\n").as_bytes()).await;
            }
            let mut lines = BufReader::new(stdout).lines();
            let mut result = None;
            let mut refusal = None;
            let mut version = None;
            let mut last_message = None;
            while let Ok(Some(line)) = lines.next_line().await {
                let line = self.0.redaction.apply(&line);
                if let Some(file) = output.as_mut() {
                    let _ = file.write_all(format!("{line}\n").as_bytes()).await;
                }
                let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                match event["type"].as_str() {
                    // <https://code.claude.com/docs/en/agent-sdk/typescript#sdksystemmessage>
                    Some("system") if event["subtype"] == "init" => {
                        if let Some(model) = event["model"].as_str() {
                            spec.report.report(Reported::Model(model.to_string()));
                        }
                        version = event["claude_code_version"]
                            .as_str()
                            .and_then(|named| named.parse::<Version>().ok());
                    }
                    Some("result") => {
                        result = serde_json::from_value::<ResultEvent>(event).ok();
                        *spend.lock().expect("the spend's lock is not poisoned") =
                            turn_spend(version, result.as_ref());
                    }
                    Some("rate_limit_event") => {
                        let event = serde_json::from_value::<RateLimitEvent>(event).ok();
                        refusal = event.and_then(|e| e.refusal()).or(refusal);
                    }
                    Some("assistant") => {
                        if event["parent_tool_use_id"].is_null()
                            && let Some(uuid) = event["uuid"].as_str()
                        {
                            last_message = Some(uuid.to_string());
                        }
                        let mut log = log.lock().expect("the log's lock is not poisoned");
                        for text in assistant_text(&event) {
                            log.said.push_back(text);
                            if log.said.len() > TAIL_LINES {
                                log.said.pop_front();
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Some(file) = output.as_mut() {
                let marker = transcript::turn_ended_marker(self.0.clock.now());
                let _ = file.write_all(format!("{marker}\n").as_bytes()).await;
            }
            (result, refusal, last_message)
        };
        let read_stderr = async {
            let mut errors = String::new();
            let _ = stderr.read_to_string(&mut errors).await;
            log.lock().expect("the log's lock is not poisoned").errors =
                self.0.redaction.apply(&errors);
        };
        let outcome = tokio::select! {
            biased;
            ran = async {
                let ((result, refusal, last_message), (), ()) =
                    tokio::join!(read_stdout, read_stderr, feed);
                (result, refusal, last_message, child.wait().await)
            } => Ok(ran),
            () = spec.waits.deadline().reached() => Err(()),
        };
        let session = session.id().to_string();
        let last_message = match &outcome {
            Ok((_, _, last_message, _)) => last_message.as_deref(),
            Err(()) => None,
        };
        let transcript = copy_transcript(
            &session,
            configuration_dir(&environment, &spec.dir),
            last_message,
            &spec.record.join("transcript.jsonl"),
            &self.0.redaction,
        )
        .await;
        let log = log.into_inner().expect("the log's lock is not poisoned");
        let _ = tokio::fs::write(spec.record.join("stderr.log"), &log.errors).await;
        let spend = spend
            .into_inner()
            .expect("the spend's lock is not poisoned");
        let Ok((result, refusal, _, status)) = outcome else {
            child.end().await;
            let timed_out = SessionError::TimedOut {
                log_tail: log.tail(),
            };
            return (Err(timed_out), spend);
        };
        let end = TurnEnd {
            said: result.map(Said::from),
            refusal,
            status,
            transcript,
            stderr_last_line: log.last_error_line(),
        };
        let turn = classify(end, session, log.tail());
        (turn, spend)
    }
}

fn assistant_text(event: &serde_json::Value) -> Vec<String> {
    event["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .flat_map(|text| text.lines().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// Copies the harness's transcript of `session` in `config` to `to`,
/// redacted, and says whether it holds the turn's `last_message`, as the
/// turn streamed it, redacted too. The transcript is at
/// `<config>/projects/<encoded cwd>/<session>.jsonl`:
/// <https://code.claude.com/docs/en/sessions>.
async fn copy_transcript(
    session: &str,
    config: Option<PathBuf>,
    last_message: Option<&str>,
    to: &Path,
    redaction: &Redaction,
) -> Kept {
    let found = match config {
        Some(config) => find_transcript(&config, session).await,
        None => None,
    };
    let transcript = found.map(|transcript| redaction.apply(&String::from_utf8_lossy(&transcript)));
    match &transcript {
        Some(transcript) => {
            let _ = tokio::fs::write(to, transcript).await;
        }
        None => tracing::warn!(session, "no transcript found for the session"),
    }
    kept(transcript.as_deref(), last_message)
}

/// The harness's configuration directory for a turn run in `dir` with
/// `environment`: its `CLAUDE_CONFIG_DIR`, or else `.claude` in its home,
/// a later value of a variable winning over an earlier one as in a spawned
/// command's: <https://code.claude.com/docs/en/env-vars>. Without `HOME`,
/// the home is the account's, as Node's `os.homedir()` finds it:
/// <https://nodejs.org/api/os.html#oshomedir>.
fn configuration_dir(environment: &[(OsString, OsString)], dir: &Path) -> Option<PathBuf> {
    let value = |name: &str| {
        environment
            .iter()
            .rfind(|(variable, _)| variable == name)
            .map(|(_, value)| PathBuf::from(value))
            .filter(|value| !value.as_os_str().is_empty())
    };
    match value("CLAUDE_CONFIG_DIR") {
        Some(config) => Some(dir.join(config)),
        None => value("HOME")
            .or_else(dirs::home_dir)
            .map(|home| home.join(".claude")),
    }
}

async fn find_transcript(config: &Path, session: &str) -> Option<Vec<u8>> {
    let mut projects = tokio::fs::read_dir(config.join("projects")).await.ok()?;
    while let Ok(Some(project)) = projects.next_entry().await {
        let transcript = project.path().join(format!("{session}.jsonl"));
        if let Ok(transcript) = tokio::fs::read(&transcript).await {
            return Some(transcript);
        }
    }
    None
}

#[async_trait]
impl Harness for ClaudeCode {
    fn environment(
        &self,
        given: &Given,
        scratch: &Path,
        effort: Option<Effort>,
        gh: &GhReach,
    ) -> Vec<(OsString, OsString)> {
        let set = set_for_session(
            scratch,
            &self.0.bash,
            &self.0.github_token,
            effort,
            gh.config_dir(),
        );
        given
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .chain(
                set.into_iter()
                    .map(|(name, value)| (name.into(), value.to_os_string())),
            )
            .collect()
    }

    async fn start(&self, spec: &SessionSpec) -> Result<Turn, SessionError> {
        let session = uuid::Uuid::new_v4().to_string();
        self.turn(spec, Session::Start(&session), &spec.prompt)
            .await
    }

    async fn resume(
        &self,
        spec: &SessionSpec,
        session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError> {
        self.turn(spec, Session::Resume(session), prompt).await
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    use linewright_core::session_env::table_passes;

    use super::*;

    #[test_log::test]
    fn the_table_passes_every_variable_claude_code_reads_a_credential_from() {
        for name in CREDENTIAL_VARIABLES {
            assert!(table_passes(name), "{name}");
        }
    }

    #[test_log::test]
    fn the_table_passes_every_variable_claude_code_reads_request_headers_from() {
        for name in HEADER_VARIABLES {
            assert!(table_passes(name), "{name}");
        }
    }

    fn exited(code: i32) -> std::process::ExitStatus {
        std::process::ExitStatus::from_raw(code << 8)
    }

    fn finished() -> Said {
        Said::Finished {
            message: "Done.".into(),
        }
    }

    /// A turn that ended as `said`, exiting 0 with nothing to check in its
    /// transcript and nothing on standard error.
    fn ended(said: Option<Said>) -> TurnEnd {
        TurnEnd {
            said,
            refusal: None,
            status: Ok(exited(0)),
            transcript: Kept::NothingToCheck,
            stderr_last_line: None,
        }
    }

    fn classified(end: TurnEnd) -> Result<Turn, SessionError> {
        classify(end, "session-1".into(), "the tail".into())
    }

    fn infrastructure(end: TurnEnd) -> String {
        match classified(end) {
            Err(SessionError::Infrastructure(reason)) => reason,
            other => panic!("expected an infrastructure failure, got {other:?}"),
        }
    }

    #[test_log::test]
    fn a_result_with_text_and_a_clean_exit_finishes_the_turn() {
        for transcript in [Kept::Holds, Kept::NothingToCheck] {
            let turn = classified(TurnEnd {
                transcript,
                ..ended(Some(finished()))
            })
            .unwrap();

            assert_eq!(turn.session, "session-1");
            assert_eq!(turn.message, "Done.");
            assert_eq!(turn.log_tail, "the tail");
        }
    }

    #[test_log::test]
    fn an_exit_with_an_error_after_a_result_outranks_a_transcript_lacking_its_message() {
        let reason = infrastructure(TurnEnd {
            status: Ok(exited(2)),
            transcript: Kept::Lacks,
            ..ended(Some(finished()))
        });

        assert_eq!(
            reason,
            "the harness exited (exit status: 2) after its result"
        );
    }

    #[test_log::test]
    fn a_harness_that_cannot_be_waited_for_after_its_result_is_an_infrastructure_failure() {
        let reason = infrastructure(TurnEnd {
            status: Err(std::io::Error::other("no child")),
            ..ended(Some(finished()))
        });

        assert_eq!(
            reason,
            "the harness could not be waited for after its result (no child)"
        );
    }

    #[test_log::test]
    fn a_start_up_failure_says_what_its_result_says_over_standard_error() {
        let reason = infrastructure(TurnEnd {
            status: Ok(exited(1)),
            stderr_last_line: Some("an error".into()),
            ..ended(Some(Said::FailedToStart {
                reason: "some_reason_yet_unknown".into(),
                said: Some("Settings are not valid JSON".into()),
            }))
        });

        assert_eq!(
            reason,
            "the harness could not start (some_reason_yet_unknown): Settings are not valid JSON"
        );
    }

    #[test_log::test]
    fn a_start_up_failure_saying_nothing_names_its_reason_alone() {
        let reason = infrastructure(TurnEnd {
            status: Ok(exited(1)),
            ..ended(Some(Said::FailedToStart {
                reason: "config_file_invalid".into(),
                said: None,
            }))
        });

        assert_eq!(reason, "the harness could not start (config_file_invalid)");
    }

    #[test_log::test]
    fn a_refusal_without_a_finished_result_is_the_usage_limit() {
        let limit = UsageLimit {
            resets_at: "2026-11-23T08:00:00Z".parse().unwrap(),
        };

        let error = classified(TurnEnd {
            refusal: Some(limit),
            status: Ok(exited(1)),
            ..ended(Some(Said::ApiFailed {
                said: "limit reached".into(),
            }))
        })
        .unwrap_err();

        let SessionError::UsageLimit {
            limit: refused,
            session,
        } = error
        else {
            panic!("expected the usage limit, got {error:?}");
        };
        assert_eq!(refused, limit);
        assert_eq!(session, "session-1");
    }

    #[test_log::test]
    fn an_error_result_and_its_exit_with_an_error_are_the_turns_error() {
        let error = classified(TurnEnd {
            status: Ok(exited(1)),
            stderr_last_line: Some("an error".into()),
            ..ended(Some(Said::Errored {
                kind: "error_max_turns".into(),
                said: None,
            }))
        })
        .unwrap_err();

        let SessionError::Errored { kind, result, .. } = error else {
            panic!("expected the turn's error, got {error:?}");
        };
        assert_eq!(kind, "error_max_turns");
        assert_eq!(result, None);
    }

    #[test_log::test]
    fn a_result_and_a_clean_exit_with_no_transcript_is_an_infrastructure_failure() {
        let reason = infrastructure(TurnEnd {
            transcript: Kept::Absent,
            ..ended(Some(finished()))
        });

        assert_eq!(reason, "the harness kept no transcript of the session");
    }

    #[test_log::test]
    fn a_failure_of_the_harnesss_api_is_an_infrastructure_failure() {
        let reason = infrastructure(TurnEnd {
            status: Ok(exited(1)),
            ..ended(Some(Said::ApiFailed {
                said: "API Error: 529 Overloaded".into(),
            }))
        });

        assert_eq!(
            reason,
            "the harness's API failed (API Error: 529 Overloaded)"
        );
    }

    #[test_log::test]
    fn an_exit_without_a_result_is_no_result() {
        let error = classified(TurnEnd {
            status: Ok(exited(1)),
            stderr_last_line: Some("an error".into()),
            ..ended(None)
        })
        .unwrap_err();

        let SessionError::NoResult { why, log_tail } = error else {
            panic!("expected no result, got {error:?}");
        };
        assert_eq!(why, "the harness exited (exit status: 1) without a result");
        assert_eq!(log_tail, "the tail");
    }

    const TRANSCRIPT: &str = "\
{\"type\":\"user\",\"uuid\":\"m-1\",\"parentUuid\":null}
{\"type\":\"assistant\",\"uuid\":\"m-2\",\"parentUuid\":\"m-1\"}
";

    #[test_log::test]
    fn a_transcript_holds_a_message_it_has_an_entry_for() {
        assert_eq!(kept(Some(TRANSCRIPT), Some("m-2")), Kept::Holds);
    }

    #[test_log::test]
    fn a_transcript_naming_a_message_only_as_another_entrys_parent_lacks_it() {
        let transcript = TRANSCRIPT.lines().next().unwrap();

        assert_eq!(kept(Some(transcript), Some("m-2")), Kept::Lacks);
        assert_eq!(
            kept(
                Some("{\"type\":\"user\",\"uuid\":\"m-3\",\"parentUuid\":\"m-2\"}"),
                Some("m-2")
            ),
            Kept::Lacks
        );
    }

    #[test_log::test]
    fn no_transcript_holds_a_message() {
        assert_eq!(kept(None, Some("m-2")), Kept::Absent);
    }

    #[test_log::test]
    fn a_turn_that_streamed_no_message_has_nothing_to_check() {
        assert_eq!(kept(Some(TRANSCRIPT), None), Kept::NothingToCheck);
        assert_eq!(kept(None, None), Kept::NothingToCheck);
    }

    fn environment(variables: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        variables
            .iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect()
    }

    #[test_log::test]
    fn the_harness_is_configured_in_the_last_claude_config_dir_of_the_turn_over_its_home() {
        let environment = environment(&[
            ("HOME", "/home/linewright"),
            ("CLAUDE_CONFIG_DIR", "/etc/claude"),
            ("CLAUDE_CONFIG_DIR", "/srv/claude"),
        ]);

        assert_eq!(
            configuration_dir(&environment, Path::new("/clone")),
            Some(PathBuf::from("/srv/claude"))
        );
    }

    #[test_log::test]
    fn a_relative_claude_config_dir_is_in_the_directory_the_turn_runs_in() {
        let environment = environment(&[("CLAUDE_CONFIG_DIR", ".claude-config")]);

        assert_eq!(
            configuration_dir(&environment, Path::new("/clone")),
            Some(PathBuf::from("/clone/.claude-config"))
        );
    }

    #[test_log::test]
    fn without_a_claude_config_dir_the_harness_is_configured_in_its_home() {
        let environment = environment(&[("CLAUDE_CONFIG_DIR", ""), ("HOME", "/home/linewright")]);

        assert_eq!(
            configuration_dir(&environment, Path::new("/clone")),
            Some(PathBuf::from("/home/linewright/.claude"))
        );
    }
}
