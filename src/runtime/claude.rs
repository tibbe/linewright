use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::{Runtime, SessionError, SessionSpec, Turn, UsageLimit};
use crate::domain::Timestamp;
use crate::store::github::TOKEN_VARIABLES;
use crate::transcript::{self, StartedBy};

pub struct ClaudeCode {
    pub program: String,
    pub bash: PathBuf,
    pub model: Option<String>,
    /// `None` gives sessions none, whatever Linewright's own environment
    /// holds.
    pub github_token: Option<String>,
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
    async fn turn(
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
            .env("TMPDIR", &spec.scratch)
            // Claude Code otherwise picks the operator's `$SHELL`. It still
            // sources the host's `~/.bashrc`, if there is one, and has no
            // switch to stop that, so a session is not insulated from it.
            // Docs: <https://code.claude.com/docs/en/settings#environment-variables>.
            .env("CLAUDE_CODE_SHELL", &self.bash)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(model) = &self.model {
            command.args(["--model", model]);
        }
        // Never inherit a GitHub token from Linewright's environment.
        for variable in TOKEN_VARIABLES {
            match &self.github_token {
                Some(token) => command.env(variable, token),
                None => command.env_remove(variable),
            };
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
                spec.output.heard();
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
            log.lock().expect("the log's lock is not poisoned").errors = errors;
        };
        let outcome = tokio::time::timeout_at(spec.deadline, async {
            let ((result, refusal, underway), (), ()) =
                tokio::join!(read_stdout, read_stderr, feed);
            (result, refusal, underway, child.wait().await)
        })
        .await;
        let session = session.id().to_string();
        copy_transcript(&session, &spec.record.join("transcript.jsonl")).await;
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
                    "the agent runtime's API failed ({})",
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
                    Ok(status) => format!("the agent runtime exited ({status}) without a result"),
                    Err(e) => format!("the agent runtime could not be waited for: {e}"),
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
async fn copy_transcript(session: &str, to: &Path) {
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
        if tokio::fs::copy(&transcript, to).await.is_ok() {
            return;
        }
    }
    tracing::warn!(session, "no transcript found for the session");
}

#[async_trait]
impl Runtime for ClaudeCode {
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
