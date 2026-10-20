use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::num::NonZeroUsize;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::Timestamp;
use crate::phrase::Phrase;
use crate::runs::Started;
use crate::status::Paint;

pub const OUTPUT: &str = "output.jsonl";

pub const BRIEF: &str = "brief.md";

/// The file in a session's record that holds its `n`th follow-up, from 1.
pub fn followup_file(n: usize) -> String {
    format!("followup-{n}.md")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartedBy {
    Brief,
    FollowUp,
}

/// The line the harness writes to [`OUTPUT`] before each turn's events: when
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

pub fn turn_marker(started_at: Timestamp, started_by: StartedBy) -> String {
    serde_json::to_string(&TurnMarker {
        kind: MarkerKind::Turn,
        started_at,
        started_by,
    })
    .expect("a marker serializes")
}

#[derive(Clone, Debug, Default)]
pub struct SessionRecord {
    pub output: String,
    pub brief: Option<String>,
    pub followups: Vec<String>,
}

impl SessionRecord {
    pub fn read(dir: &Path) -> Result<SessionRecord> {
        let mut record = SessionRecord::default();
        record.read_more(dir)?;
        Ok(record)
    }

    pub fn read_more(&mut self, dir: &Path) -> Result<()> {
        let read = |name: &str| -> Result<Option<String>> {
            let path = dir.join(name);
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
            }
        };
        if self.brief.is_none() {
            self.brief = read(BRIEF)?;
        }
        while let Some(followup) = read(&followup_file(self.followups.len() + 1))? {
            self.followups.push(followup);
        }
        let path = dir.join(OUTPUT);
        let mut added = vec![];
        match std::fs::File::open(&path) {
            Ok(mut file) => {
                file.seek(SeekFrom::Start(self.output.len() as u64))
                    .and_then(|_| file.read_to_end(&mut added))
                    .with_context(|| format!("reading {}", path.display()))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
        // The harness writes UTF-8, so what is read up to a character still
        // being written keeps its length.
        if let Err(e) = std::str::from_utf8(&added)
            && e.error_len().is_none()
        {
            added.truncate(e.valid_up_to());
        }
        self.output.push_str(&String::from_utf8_lossy(&added));
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(flatten)]
    pub run: Started,
    /// While the run is in flight, how far its transcript has been read.
    #[serde(default)]
    pub in_flight: Option<InFlight>,
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub started_at: Option<Timestamp>,
    pub dir: Option<String>,
    pub turns: Vec<Turn>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub started_at: Option<Timestamp>,
    pub prompt: Prompt,
    pub entries: Vec<Entry>,
}

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
    Message(String),
    Thinking(String),
    Context(String),
    Tool(Call),
    Event(Value),
    Other(Value),
}

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
    pub text: String,
    pub other: Vec<Value>,
    pub error: bool,
    pub details: Value,
}

/// Lines that do not read as events, such as one being written, are skipped.
pub fn parse(record: &SessionRecord) -> Session {
    let mut parser = Parser::default();
    for line in record.output.lines() {
        parser.line(line, record);
    }
    parser.finish()
}

fn is_progress(event: &Value) -> bool {
    event["type"] == "tool_progress"
        || ["thinking_tokens", "task_progress"]
            .contains(&event["subtype"].as_str().unwrap_or_default())
}

#[derive(Default)]
struct Parser {
    session: Session,
    calls: HashMap<String, EntryAt>,
    followups: usize,
    touched: Vec<Touch>,
    /// When the latest event that said so happened, or else its turn
    /// started.
    last_at: Option<Timestamp>,
}

/// Where an entry is: its turn, and its path through the calls that hold
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EntryAt {
    turn: usize,
    path: Vec<usize>,
}

enum Touch {
    Turn(usize),
    Added(EntryAt),
    Ended(EntryAt, Option<Timestamp>),
}

impl Parser {
    fn line(&mut self, line: &str, record: &SessionRecord) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if let Ok(marker) = serde_json::from_value::<TurnMarker>(event.clone()) {
            let prompt = match marker.started_by {
                StartedBy::Brief => Prompt::Brief(record.brief.clone()),
                StartedBy::FollowUp => {
                    self.followups += 1;
                    Prompt::FollowUp(record.followups.get(self.followups - 1).cloned())
                }
            };
            self.begin_turn(Some(marker.started_at), prompt);
            return;
        }
        if self.session.turns.is_empty() {
            // Recorded before turns were marked.
            self.begin_turn(None, Prompt::Brief(record.brief.clone()));
        }
        self.event(event);
    }

    fn finish(self) -> Session {
        let mut session = self.session;
        session.started_at = started_at(&session);
        session
    }

    fn begin_turn(&mut self, started_at: Option<Timestamp>, prompt: Prompt) {
        self.last_at = started_at.or(self.last_at);
        self.session.turns.push(Turn {
            started_at,
            prompt,
            entries: vec![],
        });
        self.touched.push(Touch::Turn(self.session.turns.len() - 1));
    }

    fn event(&mut self, event: Value) {
        let turn = self.session.turns.len() - 1;
        // An event that does not say when it happened happened after the
        // one before it, as the agent wrote them, wherever its entry is.
        let at = timestamp(&event["timestamp"]).or(self.last_at);
        self.last_at = at;
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
                            if let Some((returned, call)) = self.call(&block["tool_use_id"]) {
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
                                self.touched.push(Touch::Ended(returned, at));
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
                        if let Some((ended, call)) = self.call(&event["tool_use_id"]) {
                            call.background = event["status"].as_str().map(str::to_string);
                            // Only a call that has returned shows how it
                            // went on in the background.
                            if call.result.is_some() {
                                self.touched.push(Touch::Ended(ended, at));
                            }
                        }
                    }
                    _ => {}
                }
                let what = What::Event(event);
                self.push(turn, None, Entry { at, what });
            }
        }
    }

    fn push(&mut self, turn: usize, parent: Option<&[usize]>, entry: Entry) {
        let id = if let What::Tool(call) = &entry.what {
            Some(call.id.clone())
        } else {
            None
        };
        let entries = match parent {
            Some(path) => &mut call_at(&mut self.session.turns[turn].entries, path).entries,
            None => &mut self.session.turns[turn].entries,
        };
        entries.push(entry);
        let mut path = parent.map(<[usize]>::to_vec).unwrap_or_default();
        path.push(entries.len() - 1);
        let at = EntryAt { turn, path };
        if let Some(id) = id {
            self.calls.insert(id, at.clone());
        }
        self.touched.push(Touch::Added(at));
    }

    fn call(&mut self, id: &Value) -> Option<(EntryAt, &mut Call)> {
        let at = self.calls.get(id.as_str()?)?.clone();
        let call = call_at(&mut self.session.turns[at.turn].entries, &at.path);
        Some((at, call))
    }
}

fn timestamp(value: &Value) -> Option<Timestamp> {
    value.as_str().and_then(|text| text.parse().ok())
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

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
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => {
            (String::new(), vec![])
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verbosity {
    Compact,
    Full,
}

impl Verbosity {
    pub fn of(verbose: bool) -> Verbosity {
        if verbose {
            Verbosity::Full
        } else {
            Verbosity::Compact
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Line {
    Header(Header),
    Entry(Box<Stamped>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Header {
    Run {
        #[serde(flatten)]
        run: Started,
        sessions: usize,
    },
    Session {
        session: usize,
        started_at: Option<Timestamp>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stamped {
    #[serde(flatten)]
    pub(crate) stamp: Stamp,
    #[serde(flatten)]
    pub(crate) what: EntryLine,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Stamp {
    /// The session's number in the run, from 1.
    pub(crate) session: usize,
    pub(crate) at: Option<Timestamp>,
    /// Since the session started.
    pub(crate) elapsed_seconds: Option<i64>,
    #[serde(flatten)]
    pub(crate) under: Under,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "UnderFields", into = "UnderFields")]
pub(crate) enum Under {
    Session,
    SubAgent { depth: NonZeroUsize, parent: String },
}

impl Under {
    fn depth(&self) -> usize {
        match self {
            Under::Session => 0,
            Under::SubAgent { depth, .. } => depth.get(),
        }
    }

    fn sub_agent(&self, call: &str) -> Under {
        Under::SubAgent {
            depth: NonZeroUsize::MIN.saturating_add(self.depth()),
            parent: call.to_string(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct UnderFields {
    depth: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_tool_use_id: Option<String>,
}

impl From<Under> for UnderFields {
    fn from(under: Under) -> UnderFields {
        UnderFields {
            depth: under.depth(),
            parent_tool_use_id: match under {
                Under::Session => None,
                Under::SubAgent { parent, .. } => Some(parent),
            },
        }
    }
}

impl TryFrom<UnderFields> for Under {
    type Error = &'static str;

    fn try_from(fields: UnderFields) -> Result<Under, &'static str> {
        match (NonZeroUsize::new(fields.depth), fields.parent_tool_use_id) {
            (None, None) => Ok(Under::Session),
            (Some(depth), Some(parent)) => Ok(Under::SubAgent { depth, parent }),
            (None, Some(_)) => Err("a line of the session's own work must have no parent call"),
            (Some(_), None) => Err("a line of a sub-agent's work must have a parent call"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum EntryLine {
    Brief {
        text: String,
    },
    FollowUp {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Message {
        text: String,
    },
    Thinking {
        text: String,
    },
    Context {
        text: String,
    },
    Event {
        event: Value,
    },
    Other {
        other: Value,
    },
    Tool(ToolLine),
}

/// A tool call's line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ToolLine {
    pub(crate) tool_use_id: String,
    pub(crate) tool: String,
    pub(crate) argument: String,
    pub(crate) account: String,
    pub(crate) error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) input: Option<Value>,
    /// `None` while it runs, or unless at full verbosity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) details: Option<Value>,
}

fn lines(transcript: &Transcript, verbosity: Verbosity) -> Vec<Line> {
    let mut lines = vec![Line::Header(Header::Run {
        run: transcript.run.clone(),
        sessions: transcript.sessions.len(),
    })];
    for (i, session) in transcript.sessions.iter().enumerate() {
        // Of a run in flight, the session it is in shows once it starts a
        // turn, as following it does.
        let current = transcript.in_flight.is_some() && i + 1 == transcript.sessions.len();
        if current && session.turns.is_empty() {
            continue;
        }
        let mut stamper = Stamper::new(i + 1, session.started_at);
        lines.push(stamper.header());
        for turn in &session.turns {
            if let Some(what) = prompt_line(turn, verbosity) {
                lines.push(stamper.line(turn.started_at, Under::Session, what));
            }
            entry_lines(
                &turn.entries,
                &Under::Session,
                session,
                verbosity,
                &mut stamper,
                &mut lines,
            );
        }
    }
    lines
}

fn entry_lines(
    entries: &[Entry],
    under: &Under,
    session: &Session,
    verbosity: Verbosity,
    stamper: &mut Stamper,
    lines: &mut Vec<Line>,
) {
    for entry in entries {
        if let Some(what) = entry_line(entry, session, verbosity) {
            lines.push(stamper.line(entry.at, under.clone(), what));
        }
        if let What::Tool(call) = &entry.what {
            let sub_agent = under.sub_agent(&call.id);
            entry_lines(
                &call.entries,
                &sub_agent,
                session,
                verbosity,
                stamper,
                lines,
            );
        }
    }
}

struct Stamper {
    number: usize,
    started_at: Option<Timestamp>,
    last: Option<Timestamp>,
}

impl Stamper {
    fn new(number: usize, started_at: Option<Timestamp>) -> Stamper {
        Stamper {
            number,
            started_at,
            last: started_at,
        }
    }

    fn header(&self) -> Line {
        Line::Header(Header::Session {
            session: self.number,
            started_at: self.started_at,
        })
    }

    fn line(&mut self, at: Option<Timestamp>, under: Under, what: EntryLine) -> Line {
        let at = at.or(self.last);
        self.last = at;
        let stamp = Stamp {
            session: self.number,
            at,
            elapsed_seconds: at
                .zip(self.started_at)
                .map(|(at, started)| (at - started).num_seconds()),
            under,
        };
        Line::Entry(Box::new(Stamped { stamp, what }))
    }
}

fn prompt_line(turn: &Turn, verbosity: Verbosity) -> Option<EntryLine> {
    let full = verbosity == Verbosity::Full;
    match &turn.prompt {
        Prompt::Brief(Some(text)) if full => Some(EntryLine::Brief { text: text.clone() }),
        Prompt::Brief(_) => None,
        Prompt::FollowUp(prompt) => Some(EntryLine::FollowUp {
            summary: prompt.as_deref().map_or(String::new(), summary),
            text: prompt.clone().filter(|_| full),
        }),
    }
}

fn entry_line(entry: &Entry, session: &Session, verbosity: Verbosity) -> Option<EntryLine> {
    let full = verbosity == Verbosity::Full;
    Some(match &entry.what {
        What::Message(text) => EntryLine::Message { text: text.clone() },
        What::Thinking(text) if full => EntryLine::Thinking { text: text.clone() },
        What::Context(text) if full => EntryLine::Context { text: text.clone() },
        What::Event(event) if full => EntryLine::Event {
            event: event.clone(),
        },
        What::Other(other) if full => EntryLine::Other {
            other: other.clone(),
        },
        What::Thinking(_) | What::Context(_) | What::Event(_) | What::Other(_) => return None,
        What::Tool(call) => {
            let shown = call.result.as_ref().filter(|_| full);
            EntryLine::Tool(ToolLine {
                tool_use_id: call.id.clone(),
                tool: call.tool.name().to_string(),
                argument: argument(call, session.dir.as_deref()),
                account: account(call),
                error: call.result.as_ref().is_some_and(|result| result.error),
                input: full.then(|| call.input.clone()),
                result: shown.map(|result| result.text.clone()),
                content: shown
                    .map(|result| result.other.clone())
                    .filter(|other| !other.is_empty()),
                details: shown
                    .map(|result| result.details.clone())
                    .filter(|details| !details.is_null()),
            })
        }
    })
}

/// The lines shown only ever grow, the same whatever pieces the records are read in.
pub struct Follower {
    run: Started,
    verbosity: Verbosity,
    sessions: Vec<Followed>,
    after: Option<After>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InFlight {
    /// To no line: none of its sessions has written a whole one yet.
    Unread,
    ReadTo(ReadTo),
}

/// How far a run's transcript has been read: to the `line`th line of the
/// output of its `session`th session, the session from 1 and the line from
/// 0, before its first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReadTo {
    pub session: usize,
    pub line: usize,
}

/// Where a line of a followed transcript was made: the `nth`, from 1, of
/// those made once the transcript was read to `read`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Made {
    pub read: ReadTo,
    pub nth: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    Line(Made),
    Read(ReadTo),
}

impl After {
    /// Whether the line made at `made` comes after `self`.
    fn precedes(self, made: Made) -> bool {
        match self {
            After::Line(line) => made > line,
            After::Read(read) => made.read > read,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowedLine {
    pub made: Made,
    pub line: Line,
}

pub fn whole_lines(text: &str) -> &str {
    &text[..text.rfind('\n').map_or(0, |end| end + 1)]
}

struct Followed {
    number: usize,
    parser: Parser,
    bytes_read: usize,
    lines_read: usize,
    stamper: Option<Stamper>,
}

impl Follower {
    pub fn new(run: Started, verbosity: Verbosity, after: Option<After>) -> Follower {
        Follower {
            run,
            verbosity,
            sessions: vec![],
            after,
        }
    }

    /// A line still being written waits.
    pub fn update(&mut self, sessions: &[SessionRecord]) -> Vec<FollowedLine> {
        self.read(sessions, false)
    }

    pub fn end(&mut self, sessions: &[SessionRecord]) -> Vec<FollowedLine> {
        let mut lines = self.read(sessions, true);
        if self.sessions.is_empty() {
            let read = ReadTo {
                session: 1,
                line: 0,
            };
            lines.extend(made(self.after, read, vec![self.header(0)]));
        }
        lines
    }

    fn header(&self, sessions: usize) -> Line {
        Line::Header(Header::Run {
            run: self.run.clone(),
            sessions,
        })
    }

    fn read(&mut self, sessions: &[SessionRecord], ended: bool) -> Vec<FollowedLine> {
        let mut lines = vec![];
        for (i, record) in sessions.iter().enumerate() {
            let session = i + 1;
            let mut shown = vec![];
            if i == self.sessions.len() {
                if i == 0 {
                    shown.push(self.header(sessions.len()));
                }
                self.sessions.push(Followed::new(session));
            }
            let followed = &mut self.sessions[i];
            let whole = ended || i + 1 < sessions.len();
            let unread = &record.output[followed.bytes_read.min(record.output.len())..];
            let complete = if whole { unread } else { whole_lines(unread) };
            for line in complete.lines() {
                let read = ReadTo {
                    session,
                    line: followed.lines_read,
                };
                lines.extend(made(self.after, read, std::mem::take(&mut shown)));
                followed.parser.line(line, record);
                followed.lines_read += 1;
                followed.show(self.verbosity, &mut shown);
            }
            followed.bytes_read += complete.len();
            // A session that ended having shown nothing shows as `logs`
            // shows it: its header alone.
            if whole {
                followed.open(&mut shown);
            }
            let read = ReadTo {
                session,
                line: followed.lines_read,
            };
            lines.extend(made(self.after, read, shown));
        }
        lines
    }
}

fn made(after: Option<After>, read: ReadTo, lines: Vec<Line>) -> Vec<FollowedLine> {
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| FollowedLine {
            made: Made { read, nth: i + 1 },
            line,
        })
        .filter(|followed| after.is_none_or(|after| after.precedes(followed.made)))
        .collect()
}

impl Followed {
    fn new(number: usize) -> Followed {
        Followed {
            number,
            parser: Parser::default(),
            bytes_read: 0,
            lines_read: 0,
            stamper: None,
        }
    }

    fn open(&mut self, lines: &mut Vec<Line>) {
        open(&mut self.stamper, self.number, &self.parser.session, lines);
    }

    fn show(&mut self, verbosity: Verbosity, lines: &mut Vec<Line>) {
        let session = &self.parser.session;
        for touch in std::mem::take(&mut self.parser.touched) {
            let stamper = open(&mut self.stamper, self.number, session, lines);
            let (at, ended_at) = match touch {
                Touch::Turn(turn) => {
                    let turn = &session.turns[turn];
                    if let Some(what) = prompt_line(turn, verbosity) {
                        lines.push(stamper.line(turn.started_at, Under::Session, what));
                    }
                    continue;
                }
                Touch::Added(at) => (at, None),
                Touch::Ended(at, ended_at) => (at, Some(ended_at)),
            };
            let (entry, under) = entry_at(session, &at);
            let time = ended_at.unwrap_or(entry.at);
            if let Some(what) = entry_line(entry, session, verbosity) {
                lines.push(stamper.line(time, under, what));
            }
        }
    }
}

fn open<'a>(
    opened: &'a mut Option<Stamper>,
    number: usize,
    session: &Session,
    lines: &mut Vec<Line>,
) -> &'a mut Stamper {
    opened.get_or_insert_with(|| {
        let stamper = Stamper::new(number, started_at(session));
        lines.push(stamper.header());
        stamper
    })
}

fn started_at(session: &Session) -> Option<Timestamp> {
    session.turns.first().and_then(|turn| {
        turn.started_at
            .or_else(|| turn.entries.iter().find_map(|entry| entry.at))
    })
}

fn entry_at<'a>(session: &'a Session, at: &EntryAt) -> (&'a Entry, Under) {
    let mut entries = &session.turns[at.turn].entries;
    let mut under = Under::Session;
    let (last, holders) = at.path.split_last().expect("an entry's path is not empty");
    for &i in holders {
        let What::Tool(call) = &entries[i].what else {
            unreachable!("a path leads through calls only")
        };
        entries = &call.entries;
        under = under.sub_agent(&call.id);
    }
    (&entries[*last], under)
}

/// The longest a tool's argument shows, in characters.
const ARGUMENT_CHARS: usize = 80;

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

fn summary(prompt: &str) -> String {
    let prompt = prompt.trim();
    match prompt.split_once('\n') {
        Some((first, _)) => format!("{} …", first.trim_end()),
        None => prompt.to_string(),
    }
}

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
        Tool::Read => lines(details["file"]["numLines"].as_u64().map_or_else(
            || result.text.lines().count(),
            |n| usize::try_from(n).unwrap_or(usize::MAX),
        )),
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
                let uses_word = if uses == 1 { "use" } else { "uses" };
                Phrase::from(format!("done, {uses} tool {uses_word}, "))
                    .duration(seconds, None)
                    .to_string()
            }
            Some("async_launched") => in_background("started in the background"),
            _ => one_line(&result.text, ARGUMENT_CHARS),
        },
        Tool::Grep
        | Tool::Glob
        | Tool::WebFetch
        | Tool::WebSearch
        | Tool::ToolSearch
        | Tool::Other(_) => {
            if result.text.trim().is_empty() {
                "done".into()
            } else {
                one_line(&result.text, ARGUMENT_CHARS)
            }
        }
    }
}

pub fn render(transcript: &Transcript, verbosity: Verbosity, colour: bool) -> String {
    lines(transcript, verbosity)
        .iter()
        .map(|line| render_line(line, colour))
        .collect()
}

/// `line` as text, coloured if `colour`, ending in a newline.
pub fn render_line(line: &Line, colour: bool) -> String {
    let paint = Paint(colour);
    let mut shown = String::new();
    let Stamped { stamp, what } = match line {
        Line::Header(header) => {
            let _ = match header {
                Header::Run { run, sessions } => {
                    let by_hand = if run.by_hand() { ", by hand" } else { "" };
                    let run = paint.bold(run.id());
                    let none = if *sessions == 0 {
                        " (it started no session)"
                    } else {
                        ""
                    };
                    writeln!(shown, "run: {run}{by_hand}{none}")
                }
                Header::Session {
                    session,
                    started_at: Some(at),
                } => {
                    let started = Phrase::from(format!("session {session}: started ")).time(*at);
                    writeln!(shown, "{started}")
                }
                Header::Session { session, .. } => writeln!(shown, "session {session}"),
            };
            return shown;
        }
        Line::Entry(stamped) => &**stamped,
    };
    let time = stamp.elapsed_seconds.map_or(String::new(), elapsed);
    let indent = "  ".repeat(stamp.under.depth());
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
        EntryLine::Tool(ToolLine {
            tool_use_id: _,
            tool,
            argument,
            account,
            error,
            input,
            result,
            content,
            details,
        }) => {
            let account = if *error {
                paint.red(account)
            } else {
                account.clone()
            };
            let pretty = |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default();
            let input = input
                .as_ref()
                .map(|input| indented("input: ", &pretty(input)));
            let result = result.as_ref().map(|result| indented("result: ", result));
            let content = content.as_ref().map(|content| {
                let content: Vec<String> = content.iter().map(Value::to_string).collect();
                indented("content: ", &content.join("\n"))
            });
            let details = details
                .as_ref()
                .map(|details| indented("details: ", &pretty(details)));
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
    shown
}

fn indented(label: &str, text: &str) -> Vec<String> {
    let (first, rest) = block(label, text);
    std::iter::once(first)
        .chain(rest)
        .map(|line| format!("  {line}"))
        .collect()
}

fn block(label: &str, text: &str) -> (String, Vec<String>) {
    let mut lines = text.trim_end().lines();
    let first = format!("{label}{}", lines.next().unwrap_or_default());
    (first, lines.map(str::to_string).collect())
}

/// The time since a session started, as an entry's line starts with it.
pub(crate) fn elapsed(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn json_lines(transcript: &Transcript, verbosity: Verbosity) -> String {
    lines(transcript, verbosity).iter().map(json_line).collect()
}

/// `line` as JSON, ending in a newline.
pub fn json_line(line: &Line) -> String {
    serde_json::to_string(line).expect("the lines serialize") + "\n"
}
