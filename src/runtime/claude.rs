//! Claude Code as the agent runtime: one headless `claude -p` process per
//! turn, without permission prompts, streaming JSON events. Docs:
//! <https://code.claude.com/docs/en/headless>.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::{Runtime, SessionError, SessionSpec, Turn};
use crate::store::github::TOKEN_VARIABLES;

pub struct ClaudeCode {
    pub program: String,
    /// The `bash` every session runs its commands in, whatever shell the
    /// operator uses (`docs/specs/product.md`, "Operation").
    pub bash: PathBuf,
    /// `None` uses the runtime's default model.
    pub model: Option<String>,
    /// The GitHub token sessions get, as `gh` reads it. `None` gives them
    /// none, whatever Linewright's own environment holds.
    pub github_token: Option<String>,
}

/// The stream's last line: <https://code.claude.com/docs/en/headless#get-structured-output>.
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

/// What every turn reads: only the repository's configuration and agent
/// instructions, beside managed policy, never the operator's user-level
/// setup, and no memory of its own (`docs/specs/product.md`, "Repository").
/// The settings outrank the repository's, so it cannot turn auto-memory back
/// on. claude.ai connectors come with the operator's login and `CLAUDE.md`
/// files with every directory above the clone, whatever the setting sources,
/// so the settings turn those off too. Docs:
/// <https://code.claude.com/docs/en/agent-sdk/claude-code-features#what-settingsources-does-not-control>,
/// <https://code.claude.com/docs/en/memory>.
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
    });
    [
        "--setting-sources".into(),
        "project,local".into(),
        "--settings".into(),
        settings.to_string(),
    ]
}

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
        session_args: [&str; 2],
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
            .args(session_args)
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
        // A session's GitHub token is the one it was built with, never one
        // inherited: production sessions read GitHub and write to it only
        // through their result, a rule the brief keeps, and replay sessions
        // get none (`docs/specs/product.md`, "Replay").
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
        let output_path = spec.record.join("output.jsonl");
        let read_stdout = async {
            let mut output = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&output_path)
                .await
                .ok();
            let mut lines = BufReader::new(stdout).lines();
            let mut result = None;
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(file) = output.as_mut() {
                    let _ = file.write_all(format!("{line}\n").as_bytes()).await;
                }
                let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                match event["type"].as_str() {
                    Some("result") => result = serde_json::from_value::<ResultEvent>(event).ok(),
                    Some("assistant") => {
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
            result
        };
        let read_stderr = async {
            let mut errors = String::new();
            let _ = stderr.read_to_string(&mut errors).await;
            log.lock().expect("the log's lock is not poisoned").errors = errors;
        };
        let outcome = tokio::time::timeout_at(spec.deadline, async {
            let (result, (), ()) = tokio::join!(read_stdout, read_stderr, feed);
            (result, child.wait().await)
        })
        .await;
        let session = session_args[1].to_string();
        copy_transcript(&session, &spec.record.join("trace.jsonl")).await;
        let log = log.into_inner().expect("the log's lock is not poisoned");
        let _ = tokio::fs::write(spec.record.join("stderr.log"), &log.errors).await;
        let Ok((result, status)) = outcome else {
            return Err(SessionError::Failed {
                reason: "it timed out".into(),
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

/// Copies the runtime's own transcript of the session beside its record:
/// `<config>/projects/<encoded cwd>/<session>.jsonl`
/// (<https://code.claude.com/docs/en/sessions>).
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
        self.turn(spec, ["--session-id", &session], &spec.prompt)
            .await
    }

    async fn resume(
        &self,
        spec: &SessionSpec,
        session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError> {
        self.turn(spec, ["--resume", session], prompt).await
    }
}
