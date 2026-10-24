use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::{Harness, SessionError, SessionSpec, Turn, UsageLimit};
use crate::domain::Timestamp;
use crate::redaction::Redaction;
use crate::session_env::Given;
use crate::store::github::TOKEN_VARIABLES;
use crate::transcript::{self, StartedBy};

pub struct ClaudeCode {
    pub program: String,
    pub bash: PathBuf,
    pub model: Option<String>,
    /// `None` gives sessions none, whatever Linewright's own environment
    /// holds.
    pub github_token: Option<String>,
    /// A session starts from these alone, and what Linewright sets itself.
    pub environment: Given,
    /// Applied to all a session says before Linewright keeps or answers it.
    pub redaction: Redaction,
}

const TEMPORARY_DIRECTORY: &str = "TMPDIR";
/// Docs: <https://code.claude.com/docs/en/settings#environment-variables>.
const SHELL: &str = "CLAUDE_CODE_SHELL";
/// Unlike `DISABLE_AUTOUPDATER`, also blocks `claude update`. Docs:
/// <https://code.claude.com/docs/en/env-vars>.
const NO_UPDATES: &str = "DISABLE_UPDATES";
/// The variables Claude Code reads a credential from, of those in the
/// namespaces the table of sessions' variables passes. Docs:
/// <https://code.claude.com/docs/en/env-vars>.
pub const CREDENTIAL_VARIABLES: [&str; 8] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_AWS_API_KEY",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
];

/// What Linewright sets for a session, over any variable of the same name
/// `environment` gives it.
fn set_for_session<'a>(
    scratch: &'a Path,
    bash: &'a Path,
    github_token: Option<&'a str>,
) -> Vec<(&'static str, &'a OsStr)> {
    let mut variables = vec![
        (TEMPORARY_DIRECTORY, scratch.as_os_str()),
        // Claude Code otherwise picks the operator's `$SHELL`. It still
        // sources the host's `~/.bashrc`, if there is one, and has no
        // switch to stop that, so a session is not insulated from it.
        (SHELL, bash.as_os_str()),
        // Every session runs the Claude Code the operator installed.
        (NO_UPDATES, OsStr::new("1")),
    ];
    if let Some(token) = github_token {
        variables.extend(TOKEN_VARIABLES.map(|variable| (variable, OsStr::new(token))));
    }
    variables
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
fn isolation_args(clone: &Path) -> [String; 4] {
    let mut excludes = Vec::new();
    for dir in clone.ancestors().skip(1) {
        let pattern = escape_glob(dir.to_string_lossy().trim_end_matches('/'));
        excludes.push(format!("{pattern}/*.md"));
        // A clone under this `.claude` keeps its own instructions.
        if !clone.starts_with(dir.join(".claude")) {
            excludes.push(format!("{pattern}/.claude/**"));
        }
    }
    let settings = serde_json::json!({
        "autoMemoryEnabled": false,
        "disableClaudeAiConnectors": true,
        "claudeMdExcludes": excludes,
        "disableBundledSkills": true,
        "disableWorkflows": true,
        "disableRemoteControl": true,
        "disableArtifact": true,
        "skillOverrides": BTreeMap::from(UNUSED_SKILLS.map(|skill| (skill, "off"))),
        "permissions": { "deny": UNUSED_TOOLS },
    });
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

/// `path` as a glob that matches it alone. Claude Code's globs treat
/// parentheses as special too, so `globset::escape` won't do.
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
    /// The variables Linewright sets for a session given `github_token`.
    pub fn variables_set_for_sessions(github_token: Option<&str>) -> Vec<&'static str> {
        set_for_session(Path::new(""), Path::new(""), github_token)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Each line is redacted as it comes, and what the turn answers once
    /// more: its result is text of its own, such as JSON, that can hold a
    /// secret escaped once more than the line does.
    async fn turn(
        &self,
        spec: &SessionSpec,
        session: Session<'_>,
        prompt: &str,
    ) -> Result<Turn, SessionError> {
        self.unredacted_turn(spec, session, prompt)
            .await
            .map(|turn| turn.redacted(&self.redaction))
            .map_err(|error| error.redacted(&self.redaction))
    }

    async fn unredacted_turn(
        &self,
        spec: &SessionSpec,
        session: Session<'_>,
        prompt: &str,
    ) -> Result<Turn, SessionError> {
        // Claude Code matches `CLAUDE.md` files by their real path.
        let clone = std::fs::canonicalize(&spec.dir).unwrap_or_else(|_| spec.dir.clone());
        let mut command = Command::new(&self.program);
        command
            .args([
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "bypassPermissions",
            ])
            .args(isolation_args(&clone))
            .arg("--plugin-dir")
            .arg(&spec.plugin)
            .args(session.args())
            .current_dir(&spec.dir)
            .env_clear()
            .envs(self.environment.iter())
            .envs(set_for_session(
                &spec.scratch,
                &self.bash,
                self.github_token.as_deref(),
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(model) = &self.model {
            command.args(["--model", model]);
        }
        let mut child = command
            .spawn()
            .map_err(|e| SessionError::Infrastructure(format!("starting {}: {e}", self.program)))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let log = Mutex::new(Log::default());
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
                let marker = transcript::turn_marker(chrono::Utc::now(), session.started_by());
                let _ = file.write_all(format!("{marker}\n").as_bytes()).await;
            }
            let mut lines = BufReader::new(stdout).lines();
            let mut result = None;
            let mut refusal = None;
            let mut underway = false;
            while let Ok(Some(line)) = lines.next_line().await {
                let line = self.redaction.apply(&line);
                if let Some(file) = output.as_mut() {
                    let _ = file.write_all(format!("{line}\n").as_bytes()).await;
                }
                let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                match event["type"].as_str() {
                    Some("result") => result = serde_json::from_value::<ResultEvent>(event).ok(),
                    Some("rate_limit_event") => {
                        let event = serde_json::from_value::<RateLimitEvent>(event).ok();
                        refusal = event.and_then(|e| e.refusal()).or(refusal);
                    }
                    Some("assistant") => {
                        underway = true;
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
            (result, refusal, underway)
        };
        let read_stderr = async {
            let mut errors = String::new();
            let _ = stderr.read_to_string(&mut errors).await;
            log.lock().expect("the log's lock is not poisoned").errors =
                self.redaction.apply(&errors);
        };
        let outcome = tokio::time::timeout_at(spec.deadline, async {
            let ((result, refusal, underway), (), ()) =
                tokio::join!(read_stdout, read_stderr, feed);
            (result, refusal, underway, child.wait().await)
        })
        .await;
        let session = session.id().to_string();
        copy_transcript(
            &session,
            &spec.record.join("transcript.jsonl"),
            &self.redaction,
        )
        .await;
        let log = log.into_inner().expect("the log's lock is not poisoned");
        let _ = tokio::fs::write(spec.record.join("stderr.log"), &log.errors).await;
        let Ok((result, refusal, underway, status)) = outcome else {
            return Err(SessionError::TimedOut {
                log_tail: log.tail(),
            });
        };
        match result {
            Some(ResultEvent {
                is_error: false,
                result: Some(message),
                ..
            }) => Ok(Turn {
                session,
                message,
                log_tail: log.tail(),
            }),
            _ if let Some(limit) = refusal => Err(SessionError::UsageLimit {
                limit,
                underway: underway.then_some(session),
            }),
            Some(event)
                if event.terminal_reason.as_deref() == Some("api_error")
                    || event
                        .api_error_status
                        .as_ref()
                        .is_some_and(|s| !s.is_null()) =>
            {
                Err(SessionError::Infrastructure(format!(
                    "the harness's API failed ({})",
                    event.result.unwrap_or(event.subtype)
                )))
            }
            Some(event) => Err(SessionError::Failed {
                reason: format!(
                    "the session ended with `{}`{}",
                    event.subtype,
                    event.result.map(|r| format!(": {r}")).unwrap_or_default()
                ),
                log_tail: log.tail(),
            }),
            None => Err(SessionError::Failed {
                reason: match status {
                    Ok(status) => format!("the harness exited ({status}) without a result"),
                    Err(e) => format!("the harness could not be waited for: {e}"),
                },
                log_tail: log.tail(),
            }),
        }
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

/// The transcript is at `<config>/projects/<encoded cwd>/<session>.jsonl`:
/// <https://code.claude.com/docs/en/sessions>.
async fn copy_transcript(session: &str, to: &Path, redaction: &Redaction) {
    let Some(config) = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
    else {
        return;
    };
    let Ok(mut projects) = tokio::fs::read_dir(config.join("projects")).await else {
        return;
    };
    while let Ok(Some(project)) = projects.next_entry().await {
        let transcript = project.path().join(format!("{session}.jsonl"));
        if let Ok(transcript) = tokio::fs::read(&transcript).await {
            let transcript = redaction.apply(&String::from_utf8_lossy(&transcript));
            let _ = tokio::fs::write(to, transcript).await;
            return;
        }
    }
    tracing::warn!(session, "no transcript found for the session");
}

#[async_trait]
impl Harness for ClaudeCode {
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
