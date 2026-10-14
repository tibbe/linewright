//! A run's transcript as `logs` shows it: each session read from what its
//! record keeps, Claude Code's stream of events turn by turn with the brief
//! and follow-ups that started the turns, and shown compact, in full or as
//! JSON (`docs/specs/monitoring.md`, "logs"). The stream's events are
//! Claude Code's: <https://code.claude.com/docs/en/headless>.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::Timestamp;
use crate::runs::RunId;
use crate::status::{Paint, utc};

/// The file in a session's record the runtime streams its events to, every
/// turn's after the last's.
pub const OUTPUT: &str = "output.jsonl";

/// The file in a session's record that holds its brief.
pub const BRIEF: &str = "brief.md";

/// The file in a session's record that holds its `n`th follow-up, from 1.
pub fn followup_file(n: usize) -> String {
    format!("followup-{n}.md")
}

/// What started a turn of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartedBy {
    Brief,
    FollowUp,
}

/// The line the runtime writes to [`OUTPUT`] before each turn's events: when
/// the turn started, and what started it.
#[derive(Serialize, Deserialize)]
struct TurnMarker {
    #[serde(rename = "type")]
    kind: MarkerKind,
    started_at: Timestamp,
    started_by: StartedBy,
}

#[derive(Serialize, Deserialize)]
enum MarkerKind {
    #[serde(rename = "linewright-turn")]
    Turn,
}

/// The marker of a turn that `started_by` started at `started_at`.
pub fn turn_marker(started_at: Timestamp, started_by: StartedBy) -> String {
    serde_json::to_string(&TurnMarker {
        kind: MarkerKind::Turn,
        started_at,
        started_by,
    })
    .expect("a marker serializes")
}

/// What a session's record keeps of its transcript.
#[derive(Clone, Debug, Default)]
pub struct SessionRecord {
    /// The runtime's stream of events, as [`OUTPUT`] holds it.
    pub output: String,
    /// The prompt that started the session.
    pub brief: Option<String>,
    /// The prompts that resumed it, in order.
    pub followups: Vec<String>,
}

impl SessionRecord {
    /// The record in `dir`, as far as it has been written.
    pub fn read(dir: &Path) -> Result<SessionRecord> {
        let read = |name: &str| -> Result<Option<String>> {
            let path = dir.join(name);
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
            }
        };
        let mut followups = vec![];
        while let Some(followup) = read(&followup_file(followups.len() + 1))? {
            followups.push(followup);
        }
        Ok(SessionRecord {
            output: read(OUTPUT)?.unwrap_or_default(),
            brief: read(BRIEF)?,
            followups,
        })
    }
}

/// A run's transcript: its sessions, in the order it started them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub run: RunId,
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// `None` when nothing in its record says.
    pub started_at: Option<Timestamp>,
    /// The directory it worked in, which its tools' paths are shown
    /// relative to.
    pub dir: Option<String>,
    pub turns: Vec<Turn>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub started_at: Option<Timestamp>,
    pub prompt: Prompt,
    pub entries: Vec<Entry>,
}

/// What started a turn, with its text, `None` when the record lacks it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Prompt {
    Brief(Option<String>),
    FollowUp(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: Option<Timestamp>,
    pub what: What,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum What {
    /// What the agent said.
    Message(String),
    Thinking(String),
    /// What the runtime added to the conversation, such as a skill's text,
    /// or a sub-agent's prompt.
    Context(String),
    Tool(Call),
    /// Any other event the runtime streamed, such as the end of a turn, as
    /// it streamed it.
    Event(Value),
    /// Anything else in an event, such as content of a kind `logs` does not
    /// know, as the runtime streamed it.
    Other(Value),
}

/// A tool call, with what it returned and, for a sub-agent's, the
/// sub-agent's work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub id: String,
    pub tool: Tool,
    pub input: Value,
    /// `None` while it runs.
    pub result: Option<CallResult>,
    /// How a call that went on in the background ended, once it has.
    pub background: Option<String>,
    pub entries: Vec<Entry>,
}

/// A tool, by its name in Claude Code: those `logs` gives an account of
/// their own, and any other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Tool {
    Bash,
    Read,
    Write,
    Edit,
    MultiEdit,
    NotebookEdit,
    Grep,
    Glob,
    Agent,
    /// What older Claude Code names `Agent`.
    Task,
    Skill,
    WebFetch,
    WebSearch,
    ToolSearch,
    Other(String),
}

/// The tools known by name.
const KNOWN: [Tool; 14] = [
    Tool::Bash,
    Tool::Read,
    Tool::Write,
    Tool::Edit,
    Tool::MultiEdit,
    Tool::NotebookEdit,
    Tool::Grep,
    Tool::Glob,
    Tool::Agent,
    Tool::Task,
    Tool::Skill,
    Tool::WebFetch,
    Tool::WebSearch,
    Tool::ToolSearch,
];

impl Tool {
    pub fn name(&self) -> &str {
        match self {
            Tool::Bash => "Bash",
            Tool::Read => "Read",
            Tool::Write => "Write",
            Tool::Edit => "Edit",
            Tool::MultiEdit => "MultiEdit",
            Tool::NotebookEdit => "NotebookEdit",
            Tool::Grep => "Grep",
            Tool::Glob => "Glob",
            Tool::Agent => "Agent",
            Tool::Task => "Task",
            Tool::Skill => "Skill",
            Tool::WebFetch => "WebFetch",
            Tool::WebSearch => "WebSearch",
            Tool::ToolSearch => "ToolSearch",
            Tool::Other(name) => name,
        }
    }

    /// The key of the tool's main argument in its input, such as the
    /// command it runs.
    fn main_argument(&self) -> Option<&'static str> {
        match self {
            Tool::Bash => Some("command"),
            Tool::Read | Tool::Write | Tool::Edit | Tool::MultiEdit => Some("file_path"),
            Tool::NotebookEdit => Some("notebook_path"),
            Tool::Grep | Tool::Glob => Some("pattern"),
            Tool::Agent | Tool::Task => Some("description"),
            Tool::Skill => Some("skill"),
            Tool::WebFetch => Some("url"),
            Tool::WebSearch | Tool::ToolSearch => Some("query"),
            Tool::Other(_) => None,
        }
    }
}

impl From<String> for Tool {
    fn from(name: String) -> Tool {
        KNOWN
            .iter()
            .find(|known| known.name() == name)
            .cloned()
            .unwrap_or(Tool::Other(name))
    }
}

impl From<Tool> for String {
    fn from(tool: Tool) -> String {
        tool.name().to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallResult {
    /// Its text content.
    pub text: String,
    /// Its other content, such as images.
    pub other: Vec<Value>,
    pub error: bool,
    /// What the runtime says of the result beside its text, which differs
    /// from tool to tool.
    pub details: Value,
}

/// The session `record` keeps. Lines that do not read as events, such as
/// one being written, are skipped.
pub fn parse(record: &SessionRecord) -> Session {
    let mut parser = Parser {
        session: Session {
            started_at: None,
            dir: None,
            turns: vec![],
        },
        calls: HashMap::new(),
    };
    let mut followups = record.followups.iter();
    for line in record.output.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Ok(marker) = serde_json::from_value::<TurnMarker>(event.clone()) {
            let prompt = match marker.started_by {
                StartedBy::Brief => Prompt::Brief(record.brief.clone()),
                StartedBy::FollowUp => Prompt::FollowUp(followups.next().cloned()),
            };
            parser.begin_turn(Some(marker.started_at), prompt);
            continue;
        }
        if parser.session.turns.is_empty() {
            // Recorded before turns were marked.
            parser.begin_turn(None, Prompt::Brief(record.brief.clone()));
        }
        parser.event(event);
    }
    let mut session = parser.session;
    session.started_at = session.turns.first().and_then(|turn| {
        turn.started_at
            .or_else(|| turn.entries.iter().find_map(|entry| entry.at))
    });
    session
}

/// Whether `event` only counts progress on what other events show.
fn is_progress(event: &Value) -> bool {
    event["type"] == "tool_progress"
        || ["thinking_tokens", "task_progress"]
            .contains(&event["subtype"].as_str().unwrap_or_default())
}

/// A session as its events are read.
struct Parser {
    session: Session,
    /// Where each tool call is, by its ID.
    calls: HashMap<String, CallAt>,
}

/// Where a tool call is: its turn, and its path through the calls that hold
/// it.
#[derive(Clone)]
struct CallAt {
    turn: usize,
    path: Vec<usize>,
}

impl Parser {
    fn begin_turn(&mut self, started_at: Option<Timestamp>, prompt: Prompt) {
        self.session.turns.push(Turn {
            started_at,
            prompt,
            entries: vec![],
        });
    }

    fn event(&mut self, event: Value) {
        let turn = self.session.turns.len() - 1;
        let at = timestamp(&event["timestamp"]);
        // Under the call that started the sub-agent whose event it is.
        let parent = event["parent_tool_use_id"]
            .as_str()
            .and_then(|id| self.calls.get(id))
            .filter(|call| call.turn == turn)
            .map(|call| call.path.clone());
        let parent = parent.as_deref();
        let blocks = event["message"]["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        match event["type"].as_str() {
            Some("assistant") => {
                for block in blocks {
                    let what = match block["type"].as_str() {
                        Some("text") => What::Message(text(&block["text"])),
                        Some("thinking") => What::Thinking(text(&block["thinking"])),
                        Some("tool_use") => What::Tool(Call {
                            id: text(&block["id"]),
                            tool: Tool::from(text(&block["name"])),
                            input: block["input"].clone(),
                            result: None,
                            background: None,
                            entries: vec![],
                        }),
                        _ => What::Other(block),
                    };
                    // A thinking block can hold only a signature.
                    if !matches!(&what, What::Message(t) | What::Thinking(t) if t.trim().is_empty())
                    {
                        self.push(turn, parent, Entry { at, what });
                    }
                }
            }
            Some("user") => {
                if let Some(prompt) = event["message"]["content"].as_str() {
                    let what = What::Context(prompt.to_string());
                    self.push(turn, parent, Entry { at, what });
                }
                let results = blocks
                    .iter()
                    .filter(|block| block["type"] == "tool_result")
                    .count();
                for block in blocks {
                    let what = match block["type"].as_str() {
                        Some("tool_result") => {
                            if let Some(call) = self.call(&block["tool_use_id"]) {
                                let (text, other) = result_content(&block["content"]);
                                call.result = Some(CallResult {
                                    text,
                                    other,
                                    error: block["is_error"].as_bool().unwrap_or(false),
                                    details: if results == 1 {
                                        event["tool_use_result"].clone()
                                    } else {
                                        Value::Null
                                    },
                                });
                            }
                            continue;
                        }
                        Some("text") => What::Context(text(&block["text"])),
                        _ => What::Other(block),
                    };
                    self.push(turn, parent, Entry { at, what });
                }
                // Beside more than one result, it is not known whose the
                // details are.
                if results > 1 && !event["tool_use_result"].is_null() {
                    let details = serde_json::json!({"tool_use_result": event["tool_use_result"]});
                    let what = What::Other(details);
                    self.push(turn, parent, Entry { at, what });
                }
            }
            _ if is_progress(&event) => {}
            _ => {
                match event["subtype"].as_str() {
                    Some("init") => {
                        self.session.dir = event["cwd"].as_str().map(str::to_string);
                    }
                    Some("task_notification") => {
                        if let Some(call) = self.call(&event["tool_use_id"]) {
                            call.background = event["status"].as_str().map(str::to_string);
                        }
                    }
                    _ => {}
                }
                let what = What::Event(event);
                self.push(turn, None, Entry { at, what });
            }
        }
    }

    /// Adds `entry` to `turn`, under the call at `parent` if there is one.
    fn push(&mut self, turn: usize, parent: Option<&[usize]>, entry: Entry) {
        let id = match &entry.what {
            What::Tool(call) => Some(call.id.clone()),
            _ => None,
        };
        let entries = match parent {
            Some(path) => &mut call_at(&mut self.session.turns[turn].entries, path).entries,
            None => &mut self.session.turns[turn].entries,
        };
        entries.push(entry);
        if let Some(id) = id {
            let mut path = parent.map(<[usize]>::to_vec).unwrap_or_default();
            path.push(entries.len() - 1);
            self.calls.insert(id, CallAt { turn, path });
        }
    }

    /// The call with the ID `id`, if it has been read.
    fn call(&mut self, id: &Value) -> Option<&mut Call> {
        let CallAt { turn, path } = self.calls.get(id.as_str()?)?;
        Some(call_at(&mut self.session.turns[*turn].entries, path))
    }
}

fn timestamp(value: &Value) -> Option<Timestamp> {
    value.as_str().and_then(|text| text.parse().ok())
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

/// A tool result's content: its text, and its blocks of other kinds.
fn result_content(content: &Value) -> (String, Vec<Value>) {
    match content {
        Value::String(text) => (text.clone(), vec![]),
        Value::Array(blocks) => {
            let (texts, other): (Vec<&Value>, Vec<&Value>) =
                blocks.iter().partition(|block| block["type"] == "text");
            let texts: Vec<&str> = texts
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect();
            (texts.join("\n"), other.into_iter().cloned().collect())
        }
        _ => (String::new(), vec![]),
    }
}

fn call_at<'a>(entries: &'a mut [Entry], path: &[usize]) -> &'a mut Call {
    let (first, rest) = path.split_first().expect("a call's path is not empty");
    let What::Tool(call) = &mut entries[*first].what else {
        unreachable!("a path leads to calls only")
    };
    if rest.is_empty() {
        call
    } else {
        call_at(&mut call.entries, rest)
    }
}

/// How much of the transcript to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verbosity {
    /// The agent's messages, each tool call on a line, and each follow-up
    /// marked.
    Compact,
    /// Everything: also the thinking, the tools' full input and results, the
    /// brief and follow-ups, and the runtime's other events.
    Full,
}

/// One line of the transcript as `logs` shows it, or one JSON object of it.
/// Its fields keep their names and meanings (`docs/specs/monitoring.md`,
/// "Output").
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Line<'a> {
    /// What the lines after it are of.
    Header(Header<'a>),
    /// A line of a session, at a time in it.
    Entry(Stamped<'a>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Header<'a> {
    Run {
        run: &'a RunId,
        sessions: usize,
    },
    Session {
        session: usize,
        started_at: Option<Timestamp>,
    },
}

#[derive(Debug, Serialize)]
struct Stamped<'a> {
    #[serde(flatten)]
    stamp: Stamp,
    #[serde(flatten)]
    what: EntryLine<'a>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Stamp {
    /// The session's number in the run, from 1.
    session: usize,
    at: Option<Timestamp>,
    /// Since the session started.
    elapsed_seconds: Option<i64>,
    /// How many sub-agents deep it is.
    depth: usize,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum EntryLine<'a> {
    Brief {
        text: &'a str,
    },
    FollowUp {
        summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<&'a str>,
    },
    Message {
        text: &'a str,
    },
    Thinking {
        text: &'a str,
    },
    Context {
        text: &'a str,
    },
    Event {
        event: &'a Value,
    },
    Other {
        other: &'a Value,
    },
    Tool {
        tool: &'a str,
        argument: String,
        account: String,
        error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<&'a Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<&'a str>,
        /// The result's content other than its text.
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<&'a [Value]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<&'a Value>,
    },
}

/// The lines `logs` shows of `transcript`.
fn lines(transcript: &Transcript, verbosity: Verbosity) -> Vec<Line<'_>> {
    let mut lines = vec![Line::Header(Header::Run {
        run: &transcript.run,
        sessions: transcript.sessions.len(),
    })];
    for (i, session) in transcript.sessions.iter().enumerate() {
        let number = i + 1;
        lines.push(Line::Header(Header::Session {
            session: number,
            started_at: session.started_at,
        }));
        let mut walk = Walk {
            lines: &mut lines,
            verbosity,
            number,
            session,
            last: session.started_at,
        };
        for turn in &session.turns {
            walk.turn(turn);
        }
    }
    lines
}

/// The lines of one session, as they are added.
struct Walk<'a, 'l> {
    lines: &'l mut Vec<Line<'a>>,
    verbosity: Verbosity,
    /// The session's number in the run.
    number: usize,
    session: &'a Session,
    /// The time of the last line, which a line with none of its own takes.
    last: Option<Timestamp>,
}

impl<'a> Walk<'a, '_> {
    fn push(&mut self, at: Option<Timestamp>, depth: usize, what: EntryLine<'a>) {
        let at = at.or(self.last);
        self.last = at;
        let stamp = Stamp {
            session: self.number,
            at,
            elapsed_seconds: at
                .zip(self.session.started_at)
                .map(|(at, started)| (at - started).num_seconds()),
            depth,
        };
        self.lines.push(Line::Entry(Stamped { stamp, what }));
    }

    fn turn(&mut self, turn: &'a Turn) {
        let full = self.verbosity == Verbosity::Full;
        match &turn.prompt {
            Prompt::Brief(Some(text)) if full => {
                self.push(turn.started_at, 0, EntryLine::Brief { text });
            }
            Prompt::Brief(_) => {}
            Prompt::FollowUp(prompt) => {
                let prompt = prompt.as_deref();
                let summary = prompt.map_or(String::new(), summary);
                let text = prompt.filter(|_| full);
                self.push(turn.started_at, 0, EntryLine::FollowUp { summary, text });
            }
        }
        self.entries(&turn.entries, 0);
    }

    fn entries(&mut self, entries: &'a [Entry], depth: usize) {
        let full = self.verbosity == Verbosity::Full;
        for entry in entries {
            let what = match &entry.what {
                What::Message(text) => EntryLine::Message { text },
                What::Thinking(text) if full => EntryLine::Thinking { text },
                What::Context(text) if full => EntryLine::Context { text },
                What::Event(event) if full => EntryLine::Event { event },
                What::Other(other) if full => EntryLine::Other { other },
                What::Thinking(_) | What::Context(_) | What::Event(_) | What::Other(_) => continue,
                What::Tool(call) => {
                    let shown = call.result.as_ref().filter(|_| full);
                    EntryLine::Tool {
                        tool: call.tool.name(),
                        argument: argument(call, self.session.dir.as_deref()),
                        account: account(call),
                        error: call.result.as_ref().is_some_and(|result| result.error),
                        input: full.then_some(&call.input),
                        result: shown.map(|result| result.text.as_str()),
                        content: shown
                            .map(|result| result.other.as_slice())
                            .filter(|other| !other.is_empty()),
                        details: shown
                            .map(|result| &result.details)
                            .filter(|details| !details.is_null()),
                    }
                }
            };
            self.push(entry.at, depth, what);
            if let What::Tool(call) = &entry.what {
                self.entries(&call.entries, depth + 1);
            }
        }
    }
}

/// The longest a tool's argument shows, in characters.
const ARGUMENT_CHARS: usize = 80;

/// The call's main argument, such as the command it ran or the file it
/// read, on one line.
fn argument(call: &Call, dir: Option<&str>) -> String {
    let value = call
        .tool
        .main_argument()
        .and_then(|key| call.input[key].as_str())
        .or_else(|| {
            call.input
                .as_object()
                .and_then(|input| input.values().find_map(Value::as_str))
        })
        .unwrap_or_default();
    let value = match dir {
        Some(dir) => value.replace(&format!("{dir}/"), ""),
        None => value.to_string(),
    };
    one_line(&value, ARGUMENT_CHARS)
}

/// The first line of `text`, cut to `chars`, ending with `…` when there is
/// more.
fn one_line(text: &str, chars: usize) -> String {
    let text = text.trim();
    let first = text.lines().next().unwrap_or_default();
    let cut: String = first.chars().take(chars).collect();
    if cut.len() < text.len() {
        format!("{}…", cut.trim_end())
    } else {
        cut
    }
}

/// A follow-up's first line.
fn summary(prompt: &str) -> String {
    let prompt = prompt.trim();
    match prompt.split_once('\n') {
        Some((first, _)) => format!("{} …", first.trim_end()),
        None => prompt.to_string(),
    }
}

/// A short account of the call's result, such as the lines read or the
/// exit status.
fn account(call: &Call) -> String {
    let Some(result) = &call.result else {
        return "running".into();
    };
    let details = &result.details;
    let lines = |n: usize| format!("{n} {}", if n == 1 { "line" } else { "lines" });
    let in_background = |started: &str| match &call.background {
        Some(status) => format!("{status} in the background"),
        None => started.to_string(),
    };
    if result.error {
        let text = result.text.trim();
        if call.tool == Tool::Bash
            && let Some(code) = text
                .lines()
                .next()
                .and_then(|first| first.strip_prefix("Exit code "))
        {
            return format!("exit {code}");
        }
        let text = text
            .trim_start_matches("<tool_use_error>")
            .trim_end_matches("</tool_use_error>");
        return format!("failed: {}", one_line(text, ARGUMENT_CHARS));
    }
    match call.tool {
        Tool::Bash if details["interrupted"] == true => "interrupted".into(),
        Tool::Bash if details["backgroundTaskId"].is_string() => {
            in_background("moved to the background")
        }
        Tool::Bash => match result.text.lines().count() {
            0 => "exit 0, no output".into(),
            n => format!("exit 0, {}", lines(n)),
        },
        Tool::Read => lines(
            details["file"]["numLines"]
                .as_u64()
                .map_or_else(|| result.text.lines().count(), |n| n as usize),
        ),
        Tool::Write => format!(
            "wrote {}",
            lines(
                call.input["content"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .count()
            )
        ),
        Tool::Edit | Tool::MultiEdit | Tool::NotebookEdit => "edited".into(),
        Tool::Skill => "loaded".into(),
        Tool::Agent | Tool::Task => match details["status"].as_str() {
            Some("completed") => {
                let uses = details["totalToolUseCount"].as_u64().unwrap_or(0);
                let seconds = details["totalDurationMs"].as_u64().unwrap_or(0) / 1000;
                format!(
                    "done, {uses} tool {}, {}",
                    if uses == 1 { "use" } else { "uses" },
                    humantime::format_duration(std::time::Duration::from_secs(seconds))
                )
            }
            Some("async_launched") => in_background("started in the background"),
            _ => one_line(&result.text, ARGUMENT_CHARS),
        },
        _ if result.text.trim().is_empty() => "done".into(),
        _ => one_line(&result.text, ARGUMENT_CHARS),
    }
}

/// `transcript` as text, coloured if `colour`.
pub fn render(transcript: &Transcript, verbosity: Verbosity, colour: bool) -> String {
    let paint = Paint(colour);
    let mut shown = String::new();
    for line in lines(transcript, verbosity) {
        let Stamped { stamp, what } = match line {
            Line::Header(header) => {
                let _ = match header {
                    Header::Run { run, sessions: 0 } => {
                        let run = paint.bold(&run.to_string());
                        writeln!(shown, "run: {run} (it started no session)")
                    }
                    Header::Run { run, .. } => {
                        writeln!(shown, "run: {}", paint.bold(&run.to_string()))
                    }
                    Header::Session {
                        session,
                        started_at: Some(at),
                    } => writeln!(shown, "session {session}: started {}", utc(at)),
                    Header::Session { session, .. } => writeln!(shown, "session {session}"),
                };
                continue;
            }
            Line::Entry(stamped) => stamped,
        };
        let time = stamp.elapsed_seconds.map_or(String::new(), elapsed);
        let indent = "  ".repeat(stamp.depth);
        let (first, rest): (String, Vec<String>) = match what {
            EntryLine::Message { text } => block("", text),
            EntryLine::Thinking { text } => block(&paint.yellow("thinking: "), text),
            EntryLine::Context { text } => block("context: ", text),
            EntryLine::Event { event } => (format!("event: {event}"), vec![]),
            EntryLine::Other { other } => (format!("other: {other}"), vec![]),
            EntryLine::Brief { text } => block("brief: ", text),
            EntryLine::FollowUp {
                text: Some(text), ..
            } => block("follow-up: ", text),
            EntryLine::FollowUp { summary, .. } => (format!("follow-up: {summary}"), vec![]),
            EntryLine::Tool {
                tool,
                argument,
                account,
                error,
                input,
                result,
                content,
                details,
            } => {
                let account = if error { paint.red(&account) } else { account };
                let pretty =
                    |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default();
                let input = input.map(|input| indented("input: ", &pretty(input)));
                let result = result.map(|result| indented("result: ", result));
                let content = content.map(|content| {
                    let content: Vec<String> = content.iter().map(Value::to_string).collect();
                    indented("content: ", &content.join("\n"))
                });
                let details = details.map(|details| indented("details: ", &pretty(details)));
                (
                    format!("{}({argument}) → {account}", paint.bold(tool)),
                    [input, result, content, details]
                        .into_iter()
                        .flatten()
                        .flatten()
                        .collect(),
                )
            }
        };
        let _ = writeln!(shown, "{time:>5}  {indent}{first}");
        for more in rest {
            if more.trim().is_empty() {
                shown.push('\n');
            } else {
                let _ = writeln!(shown, "       {indent}{more}");
            }
        }
    }
    shown
}

/// `text` after `label`, every line of it indented a step.
fn indented(label: &str, text: &str) -> Vec<String> {
    let (first, rest) = block(label, text);
    std::iter::once(first)
        .chain(rest)
        .map(|line| format!("  {line}"))
        .collect()
}

/// `text` after `label`: its first line, and the lines after it.
fn block(label: &str, text: &str) -> (String, Vec<String>) {
    let mut lines = text.trim_end().lines();
    let first = format!("{label}{}", lines.next().unwrap_or_default());
    (first, lines.map(str::to_string).collect())
}

/// `seconds` as `m:ss`, or `h:mm:ss` from an hour.
fn elapsed(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `transcript` as JSON, one object per line.
pub fn json_lines(transcript: &Transcript, verbosity: Verbosity) -> String {
    lines(transcript, verbosity)
        .iter()
        .map(|line| serde_json::to_string(line).expect("the lines serialize") + "\n")
        .collect()
}
