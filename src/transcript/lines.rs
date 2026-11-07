//! The CLI's layout of a transcript (`docs/specs/monitoring.md`, logs): its
//! lines, as text and as JSON.

use std::fmt::Write as _;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    After, CallResult, Change, Changed, Clock, CloneEnded, Entry, EntryAt, Feed, Layout, Made,
    Maintenance, MaintenanceEnded, MaintenanceRan, MaintenanceStatus, Numbering, Place, Prompt,
    ReadTo, START, Session, SessionClone, SessionRecord, Transcript, Turn, What, shown_command,
    summary,
};
use crate::domain::Timestamp;
use crate::phrase::Phrase;
use crate::runs::Started;
use crate::status::Paint;

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
    Clone(CloneLine),
    Maintenance(MaintenanceLine),
}

/// The line of the making of a session's clone: how it ended, once it has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CloneFields", into = "CloneFields")]
pub(crate) struct CloneLine {
    pub(crate) ended: Option<CloneEnded>,
}

/// A clone line as JSON: both null while the clone is being made, and the
/// error null when it was made (`docs/specs/monitoring.md`, logs, The clone).
#[derive(Serialize, Deserialize)]
struct CloneFields {
    duration_seconds: Option<f64>,
    error: Option<String>,
}

impl From<CloneLine> for CloneFields {
    fn from(line: CloneLine) -> CloneFields {
        match line.ended {
            None => CloneFields {
                duration_seconds: None,
                error: None,
            },
            Some(ended) => CloneFields {
                duration_seconds: Some(ended.duration_seconds),
                error: ended.error,
            },
        }
    }
}

impl TryFrom<CloneFields> for CloneLine {
    type Error = &'static str;

    fn try_from(fields: CloneFields) -> Result<CloneLine, &'static str> {
        let ended = match (fields.duration_seconds, fields.error) {
            (None, None) => None,
            (Some(duration_seconds), error) => Some(CloneEnded {
                duration_seconds,
                error,
            }),
            (None, Some(_)) => return Err("a clone line with an error must have a duration"),
        };
        Ok(CloneLine { ended })
    }
}

/// A session's maintenance command's line: the command, and how it ended once it
/// has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MaintenanceFields", into = "MaintenanceFields")]
pub(crate) struct MaintenanceLine {
    pub(crate) command: String,
    pub(crate) ended: Option<MaintenanceEnded>,
}

/// A maintenance line as JSON: what it holds once the command has ended is null
/// until then (`docs/specs/monitoring.md`, logs, Running in JSON).
#[derive(Serialize, Deserialize)]
struct MaintenanceFields {
    command: String,
    status: Option<MaintenanceStatus>,
    duration_seconds: Option<f64>,
    output: Option<String>,
}

impl From<MaintenanceLine> for MaintenanceFields {
    fn from(line: MaintenanceLine) -> MaintenanceFields {
        let ended = line.ended;
        MaintenanceFields {
            command: line.command,
            status: ended.as_ref().map(|ended| ended.status),
            duration_seconds: ended.as_ref().map(|ended| ended.duration_seconds),
            output: ended.map(|ended| ended.output),
        }
    }
}

impl TryFrom<MaintenanceFields> for MaintenanceLine {
    type Error = &'static str;

    fn try_from(fields: MaintenanceFields) -> Result<MaintenanceLine, &'static str> {
        let ended = match (fields.status, fields.duration_seconds, fields.output) {
            (None, None, None) => None,
            (Some(status), Some(duration_seconds), Some(output)) => Some(MaintenanceEnded {
                status,
                duration_seconds,
                output,
            }),
            _ => {
                return Err(
                    "a maintenance line must have a status, a duration and an output, or none of them",
                );
            }
        };
        Ok(MaintenanceLine {
            command: fields.command,
            ended,
        })
    }
}

/// A tool call's line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ToolFields", into = "ToolFields")]
pub(crate) struct ToolLine {
    pub(crate) tool_use_id: String,
    pub(crate) tool: String,
    pub(crate) argument: String,
    pub(crate) account: String,
    pub(crate) shown: ToolShown,
}

/// What a tool call's line shows beyond its account, by verbosity.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ToolShown {
    /// Whether it failed, once it has returned.
    Compact { error: Option<bool> },
    /// Its input, and what it returned once it has.
    Full {
        input: Value,
        returned: Option<CallResult>,
    },
}

impl ToolShown {
    /// Its parts as a transcript in full shows them, each with its label:
    /// none at compact verbosity.
    pub(crate) fn parts(&self) -> Vec<(&'static str, String)> {
        match self {
            ToolShown::Compact { .. } => vec![],
            ToolShown::Full { input, returned } => super::parts(input, returned.as_ref()),
        }
    }
}

impl ToolLine {
    /// Whether it has returned an error.
    pub(crate) fn failed(&self) -> bool {
        self.error() == Some(true)
    }

    fn error(&self) -> Option<bool> {
        match &self.shown {
            ToolShown::Compact { error } => *error,
            ToolShown::Full { returned, .. } => returned.as_ref().map(|returned| returned.error),
        }
    }
}

/// A tool line as JSON: `error`, and in full `result`, `content` and
/// `details`, are null while the call runs, and `content` and `details`
/// also when it returned none (`docs/specs/monitoring.md`, logs, Running in
/// JSON). A compact line has none of the keys only a line in full has.
#[derive(Serialize, Deserialize)]
struct ToolFields {
    tool_use_id: String,
    tool: String,
    argument: String,
    account: String,
    error: Option<bool>,
    #[serde(default, skip_serializing_if = "InFull::is_absent")]
    input: InFull<Value>,
    #[serde(default, skip_serializing_if = "InFull::is_absent")]
    result: InFull<String>,
    #[serde(default, skip_serializing_if = "InFull::is_absent")]
    content: InFull<Vec<Value>>,
    #[serde(default, skip_serializing_if = "InFull::is_absent")]
    details: InFull<Value>,
}

/// A key only a tool line in full has: there in full, even when null.
#[derive(Default)]
enum InFull<T> {
    #[default]
    Absent,
    Null,
    Is(T),
}

impl<T> InFull<T> {
    fn is_absent(&self) -> bool {
        matches!(self, InFull::Absent)
    }

    fn or_null(value: Option<T>) -> InFull<T> {
        value.map_or(InFull::Null, InFull::Is)
    }

    /// Its value, or `T`'s default when it is null or not there.
    fn unwrap_or_default(self) -> T
    where
        T: Default,
    {
        match self {
            InFull::Is(value) => value,
            InFull::Absent | InFull::Null => T::default(),
        }
    }
}

impl<T: Serialize> Serialize for InFull<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            InFull::Is(value) => value.serialize(serializer),
            InFull::Absent | InFull::Null => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for InFull<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<InFull<T>, D::Error> {
        Option::deserialize(deserializer).map(InFull::or_null)
    }
}

impl From<ToolLine> for ToolFields {
    fn from(line: ToolLine) -> ToolFields {
        let (error, input, content, details, result) = match line.shown {
            ToolShown::Compact { error } => (
                error,
                InFull::Absent,
                InFull::Absent,
                InFull::Absent,
                InFull::Absent,
            ),
            ToolShown::Full {
                input,
                returned: None,
            } => (
                None,
                InFull::Is(input),
                InFull::Null,
                InFull::Null,
                InFull::Null,
            ),
            ToolShown::Full {
                input,
                returned: Some(returned),
            } => (
                Some(returned.error),
                InFull::Is(input),
                InFull::or_null(returned.content().map(<[Value]>::to_vec)),
                InFull::or_null(returned.details().cloned()),
                InFull::Is(returned.text),
            ),
        };
        ToolFields {
            tool_use_id: line.tool_use_id,
            tool: line.tool,
            argument: line.argument,
            account: line.account,
            error,
            input,
            result,
            content,
            details,
        }
    }
}

impl TryFrom<ToolFields> for ToolLine {
    type Error = &'static str;

    fn try_from(fields: ToolFields) -> Result<ToolLine, &'static str> {
        use InFull::{Absent, Is, Null};
        let shown = match (
            fields.error,
            fields.input,
            fields.result,
            fields.content,
            fields.details,
        ) {
            (error, Absent, Absent, Absent, Absent) => ToolShown::Compact { error },
            (None, called @ (Is(_) | Null), Null, Null, Null) => ToolShown::Full {
                input: called.unwrap_or_default(),
                returned: None,
            },
            (
                Some(error),
                called @ (Is(_) | Null),
                Is(result),
                content @ (Is(_) | Null),
                details @ (Is(_) | Null),
            ) => ToolShown::Full {
                input: called.unwrap_or_default(),
                returned: Some(CallResult {
                    error,
                    text: result,
                    other: content.unwrap_or_default(),
                    details: details.unwrap_or_default(),
                }),
            },
            _ => {
                return Err(
                    "a tool line in full must have an input, a result, content and details, \
                     null until the call returns and only then with an error",
                );
            }
        };
        Ok(ToolLine {
            tool_use_id: fields.tool_use_id,
            tool: fields.tool,
            argument: fields.argument,
            account: fields.account,
            shown,
        })
    }
}

fn lines(transcript: &Transcript, verbosity: Verbosity) -> Vec<Line> {
    let mut lines = vec![Line::Header(Header::Run {
        run: transcript.run.clone(),
        sessions: transcript.sessions.len(),
    })];
    for (i, session) in transcript.sessions.iter().enumerate() {
        let current = transcript.in_flight.is_some() && i + 1 == transcript.sessions.len();
        let nothing_read =
            session.clone.is_none() && session.maintenance.is_none() && session.turns.is_empty();
        if current && nothing_read {
            continue;
        }
        let mut stamper = Stamper::new(i + 1, session.started_at);
        lines.push(stamper.header());
        if let Some(clone) = &session.clone {
            lines.push(stamper.clone_line(clone, clone.at));
        }
        if let Some(maintenance) = &session.maintenance {
            lines.push(stamper.maintenance(maintenance, maintenance.at()));
        }
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
    clock: Clock,
}

impl Stamper {
    fn new(number: usize, started_at: Option<Timestamp>) -> Stamper {
        Stamper {
            number,
            clock: Clock::new(started_at),
        }
    }

    /// That the session started at `at`, unless when is already known.
    fn started(&mut self, at: Option<Timestamp>) {
        self.clock.started(at);
    }

    fn header(&self) -> Line {
        Line::Header(Header::Session {
            session: self.number,
            started_at: self.clock.started_at(),
        })
    }

    fn line(&mut self, at: Option<Timestamp>, under: Under, what: EntryLine) -> Line {
        let at = self.clock.tick(at);
        self.stamped(at, under, what)
    }

    fn stamped(&self, at: Option<Timestamp>, under: Under, what: EntryLine) -> Line {
        let stamp = Stamp {
            session: self.number,
            at,
            elapsed_seconds: self.clock.since_started(at),
            under,
        };
        Line::Entry(Box::new(Stamped { stamp, what }))
    }

    /// The line of the making of `clone` at `at`.
    fn clone_line(&mut self, clone: &SessionClone, at: Timestamp) -> Line {
        let what = EntryLine::Clone(CloneLine {
            ended: clone.ended.clone(),
        });
        self.line(Some(at), Under::Session, what)
    }

    /// The line of `maintenance` at `at`, or with no time for one recorded
    /// before its start was.
    fn maintenance(&mut self, maintenance: &Maintenance, at: Option<Timestamp>) -> Line {
        let what = EntryLine::Maintenance(MaintenanceLine {
            command: maintenance.command.clone(),
            ended: maintenance.ended().cloned(),
        });
        match maintenance.ran {
            MaintenanceRan::Started { .. } => self.line(at, Under::Session, what),
            MaintenanceRan::Recorded(_) => self.stamped(None, Under::Session, what),
        }
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
        What::Tool(call) => EntryLine::Tool(ToolLine {
            tool_use_id: call.id.clone(),
            tool: call.tool.name().to_string(),
            argument: call.argument(session.dir.as_deref()),
            account: call.account(),
            shown: if full {
                ToolShown::Full {
                    input: call.input.clone(),
                    returned: call.result.clone(),
                }
            } else {
                ToolShown::Compact {
                    error: call.result.as_ref().map(|result| result.error),
                }
            },
        }),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowedLine {
    pub made: Made,
    pub line: Line,
}

/// A run's transcript followed as lines. The lines shown only ever grow, the
/// same whatever pieces the records are read in.
pub struct Follower {
    run: Started,
    verbosity: Verbosity,
    feed: Feed,
    numbering: Numbering,
    /// Each opened session's, in order.
    stampers: Vec<Stamper>,
}

impl Follower {
    pub fn new(run: Started, verbosity: Verbosity, after: Option<After>) -> Follower {
        Follower {
            run,
            verbosity,
            feed: Feed::default(),
            numbering: Numbering::new(after),
            stampers: vec![],
        }
    }

    /// A line still being written waits.
    pub fn update(&mut self, sessions: &[SessionRecord]) -> Vec<FollowedLine> {
        self.read(sessions, false)
    }

    pub fn end(&mut self, sessions: &[SessionRecord]) -> Vec<FollowedLine> {
        self.read(sessions, true)
    }

    fn read(&mut self, sessions: &[SessionRecord], ended: bool) -> Vec<FollowedLine> {
        let mut layout = LineLayout {
            run: &self.run,
            verbosity: self.verbosity,
            numbering: &mut self.numbering,
            stampers: &mut self.stampers,
            lines: vec![],
        };
        self.feed.read(sessions, ended, &mut layout);
        layout.lines
    }
}

/// The lines a [`Follower`] makes of one read.
struct LineLayout<'a> {
    run: &'a Started,
    verbosity: Verbosity,
    numbering: &'a mut Numbering,
    stampers: &'a mut Vec<Stamper>,
    lines: Vec<FollowedLine>,
}

impl LineLayout<'_> {
    fn made(&mut self, read: ReadTo, line: Line) {
        if let Some(made) = self.numbering.next(read) {
            self.lines.push(FollowedLine { made, line });
        }
    }
}

impl Layout for LineLayout<'_> {
    fn began(&mut self, sessions: usize) {
        let header = Header::Run {
            run: self.run.clone(),
            sessions,
        };
        self.made(START, Line::Header(header));
    }

    fn changed(&mut self, Changed { read, change }: Changed, session: &Session) {
        if change == Change::Opened {
            self.stampers
                .push(Stamper::new(read.session, session.started_at));
        }
        let stamper = &mut self.stampers[read.session - 1];
        if let Some(line) = changed_line(change, session, self.verbosity, stamper) {
            self.made(read, line);
        }
    }
}

/// The line `change` makes of `session`, if any.
fn changed_line(
    change: Change,
    session: &Session,
    verbosity: Verbosity,
    stamper: &mut Stamper,
) -> Option<Line> {
    stamper.started(session.started_at);
    let (place, ended_at) = match change {
        Change::Opened => return Some(stamper.header()),
        Change::Turn(turn) => {
            let turn = &session.turns[turn];
            let what = prompt_line(turn, verbosity)?;
            return Some(stamper.line(turn.started_at, Under::Session, what));
        }
        Change::Added(place) => (place, None),
        Change::Ended(place, ended_at) => (place, Some(ended_at)),
    };
    let at = match place {
        Place::Clone => {
            let clone = session.clone.as_ref().expect("a changed clone is there");
            return Some(stamper.clone_line(clone, ended_at.flatten().unwrap_or(clone.at)));
        }
        Place::Maintenance => {
            let maintenance = session
                .maintenance
                .as_ref()
                .expect("a changed maintenance command is there");
            return Some(stamper.maintenance(maintenance, ended_at.unwrap_or(maintenance.at())));
        }
        Place::Entry(at) => at,
    };
    let entry = session.entry(&at);
    let what = entry_line(entry, session, verbosity)?;
    Some(stamper.line(ended_at.unwrap_or(entry.at), under(session, &at), what))
}

/// What the entry `at` is under: its session, or the calls that started the
/// sub-agents whose work it is.
fn under(session: &Session, at: &EntryAt) -> Under {
    let Some(parent) = at.parent() else {
        return Under::Session;
    };
    let What::Tool(call) = &session.entry(&parent).what else {
        unreachable!("an entry is under calls only")
    };
    under(session, &parent).sub_agent(&call.id)
}
pub fn render(transcript: &Transcript, verbosity: Verbosity, colour: bool) -> String {
    lines(transcript, verbosity)
        .iter()
        .map(|line| render_line(line, verbosity, colour))
        .collect()
}

/// `line` as text, coloured if `colour`, ending in a newline. `verbosity`
/// is that of the transcript `line` is of.
pub fn render_line(line: &Line, verbosity: Verbosity, colour: bool) -> String {
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
        EntryLine::Tool(call) => {
            let ToolLine {
                tool,
                argument,
                account,
                ..
            } = call;
            let account = if call.failed() {
                paint.red(account)
            } else {
                account.clone()
            };
            (
                format!("{}({argument}) → {account}", paint.bold(tool)),
                call.shown
                    .parts()
                    .into_iter()
                    .flat_map(|(label, text)| indented(&format!("{label}: "), &text))
                    .collect(),
            )
        }
        EntryLine::Clone(CloneLine { ended }) => match ended {
            None => ("clone → running".to_string(), vec![]),
            Some(ended) => (
                format!("clone → {}", ended.account()),
                ended.error.as_deref().map_or(vec![], |error| {
                    let red: Vec<String> = error.lines().map(|part| paint.red(part)).collect();
                    indented("", &red.join("\n"))
                }),
            ),
        },
        EntryLine::Maintenance(MaintenanceLine { command, ended }) => {
            let command = shown_command(command);
            let (account, output) = match ended {
                None => ("running".to_string(), vec![]),
                Some(ended) => (
                    if ended.status.failed() {
                        paint.red(ended.account())
                    } else {
                        ended.account()
                    },
                    if ended.output_shown(verbosity == Verbosity::Full)
                        && !ended.output.trim().is_empty()
                    {
                        indented("", &ended.output)
                    } else {
                        vec![]
                    },
                ),
            };
            (format!("maintenance: {command} → {account}"), output)
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
