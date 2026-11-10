//! The CLI's layout of a transcript (`docs/specs/monitoring.md`, logs): its
//! lines, as text and as JSON.

use std::fmt::Write as _;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Account, After, CallResult, Change, Changed, Clock, CloneEnded, Course, Entry, EntryAt, Feed,
    Held, INTERRUPTED, Lasted, Layout, Made, Maintenance, MaintenanceEnded, MaintenanceRan,
    MaintenanceStatus, Numbering, Place, Prompt, ReadTo, START, Session, SessionClone,
    SessionRecord, Transcript, Turn, What, shown_command, summary, whole_seconds,
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
    Thought(ThoughtLine),
    WorkedFor(Span),
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
    Maintenance(SessionMaintenanceLine),
}

/// A thought's line: how long it went on and, in full, what its record
/// holds of it, if anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ThoughtLine {
    #[serde(flatten)]
    pub(crate) span: Span,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) in_full: Option<ThoughtInFull>,
}

/// What a thought's line in full shows of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ThoughtInFull {
    Text(String),
    /// The agent's block that holds it encrypted.
    Other(Value),
}

/// How long a thought or a turn went on, where that is known, and whether
/// what it is of was cut short (`docs/specs/design_system.md`, Transcripts,
/// Cut short).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Span {
    pub(crate) duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) interrupted: bool,
}

impl Span {
    /// How long something that has ended `lasted`, unless it runs still.
    fn of(lasted: Lasted) -> Option<Span> {
        let (duration_seconds, interrupted) = match lasted {
            Lasted::Running { .. } => return None,
            Lasted::Took { seconds } => (seconds, false),
            Lasted::Interrupted { seconds } => (seconds, true),
        };
        Some(Span {
            duration_seconds,
            interrupted,
        })
    }

    /// A thought's line as text: `thought`, `interrupted` if it was cut
    /// short, then how long it went on.
    fn thought(self) -> String {
        let said = cut_short(THOUGHT, self.interrupted);
        match self.duration_seconds {
            Some(seconds) => with_duration(&said, whole_seconds(seconds)),
            None => said,
        }
    }

    /// A turn's last line as text: `worked for` and how long, then
    /// `interrupted` if its session ended first.
    fn worked_for(self) -> String {
        let worked = match self.duration_seconds {
            Some(seconds) => Phrase::from(format!("{WORKED_FOR} "))
                .duration(whole_seconds(seconds), None)
                .to_string(),
            None => WORKED_FOR.to_string(),
        };
        cut_short(&worked, self.interrupted)
    }
}

/// `said`, then `interrupted` if what it says is of was `interrupted`.
pub(crate) fn cut_short(said: &str, interrupted: bool) -> String {
    if interrupted {
        format!("{said}, {INTERRUPTED}")
    } else {
        said.to_string()
    }
}

/// What a thought's line says it is.
pub(crate) const THOUGHT: &str = "thought";

/// What a turn's last line says it is.
pub(crate) const WORKED_FOR: &str = "worked for";

/// The `account` key of a line of an entry whose fields as JSON hold how far
/// it ran, `course`: there only for one that was interrupted.
fn interrupted<T>(course: &Course<T>) -> Option<String> {
    matches!(course, Course::Interrupted).then(|| INTERRUPTED.to_string())
}

/// `ended` as far as an entry's line as JSON says it ran: running when it
/// says neither, unless its `account` says it was interrupted.
fn course_of<T>(ended: Option<T>, account: Option<String>) -> Result<Course<T>, &'static str> {
    match (ended, account) {
        (None, None) => Ok(Course::Running),
        (Some(ended), None) => Ok(Course::Ended(ended)),
        (None, Some(account)) if account == INTERRUPTED => Ok(Course::Interrupted),
        (None, Some(_)) => Err("an entry's line may have an account only once interrupted"),
        (Some(_), Some(_)) => Err("an interrupted entry's line must say nothing of its end"),
    }
}

/// The line of the making of a session's clone: how it ended, once it has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CloneFields", into = "CloneFields")]
pub(crate) struct CloneLine {
    pub(crate) course: Course<CloneEnded>,
}

/// A clone line as JSON: both null while the clone is being made or once it
/// was interrupted, and the error null when it was made
/// (`docs/specs/monitoring.md`, logs, The clone).
#[derive(Serialize, Deserialize)]
struct CloneFields {
    duration_seconds: Option<f64>,
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
}

impl From<CloneLine> for CloneFields {
    fn from(line: CloneLine) -> CloneFields {
        let account = interrupted(&line.course);
        match line.course {
            Course::Running | Course::Interrupted => CloneFields {
                duration_seconds: None,
                error: None,
                account,
            },
            Course::Ended(ended) => CloneFields {
                duration_seconds: Some(ended.duration_seconds),
                error: ended.error,
                account,
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
        Ok(CloneLine {
            course: course_of(ended, fields.account)?,
        })
    }
}

/// A build's line, as a maintenance command's: the command, and how it
/// ended once it has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MaintenanceFields", into = "MaintenanceFields")]
pub(crate) struct BuildMaintenanceLine {
    pub(crate) command: String,
    pub(crate) ended: Option<MaintenanceEnded>,
}

/// A session's maintenance command's line: the command, and how far it ran.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MaintenanceFields", into = "MaintenanceFields")]
pub(crate) struct SessionMaintenanceLine {
    pub(crate) command: String,
    pub(crate) course: Course<MaintenanceEnded>,
}

/// A maintenance line as JSON: what it holds once the command has ended is null
/// until then, and once it was interrupted (`docs/specs/monitoring.md`, logs,
/// Running in JSON).
#[derive(Serialize, Deserialize)]
struct MaintenanceFields {
    command: String,
    status: Option<MaintenanceStatus>,
    duration_seconds: Option<f64>,
    output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
}

impl MaintenanceFields {
    fn of(command: String, ended: Option<MaintenanceEnded>, account: Option<String>) -> Self {
        MaintenanceFields {
            command,
            status: ended.as_ref().map(|ended| ended.status),
            duration_seconds: ended.as_ref().map(|ended| ended.duration_seconds),
            output: ended.map(|ended| ended.output),
            account,
        }
    }

    /// How the command ended, as the fields say, if they do.
    fn ended(&mut self) -> Result<Option<MaintenanceEnded>, &'static str> {
        match (
            self.status.take(),
            self.duration_seconds.take(),
            self.output.take(),
        ) {
            (None, None, None) => Ok(None),
            (Some(status), Some(duration_seconds), Some(output)) => Ok(Some(MaintenanceEnded {
                status,
                duration_seconds,
                output,
            })),
            _ => Err(
                "a maintenance line must have a status, a duration and an output, or none of them",
            ),
        }
    }
}

impl From<BuildMaintenanceLine> for MaintenanceFields {
    fn from(line: BuildMaintenanceLine) -> MaintenanceFields {
        MaintenanceFields::of(line.command, line.ended, None)
    }
}

impl TryFrom<MaintenanceFields> for BuildMaintenanceLine {
    type Error = &'static str;

    fn try_from(mut fields: MaintenanceFields) -> Result<BuildMaintenanceLine, &'static str> {
        if fields.account.is_some() {
            return Err("a build's maintenance line is never interrupted");
        }
        Ok(BuildMaintenanceLine {
            ended: fields.ended()?,
            command: fields.command,
        })
    }
}

impl From<SessionMaintenanceLine> for MaintenanceFields {
    fn from(line: SessionMaintenanceLine) -> MaintenanceFields {
        let account = interrupted(&line.course);
        let ended = match line.course {
            Course::Ended(ended) => Some(ended),
            Course::Running | Course::Interrupted => None,
        };
        MaintenanceFields::of(line.command, ended, account)
    }
}

impl TryFrom<MaintenanceFields> for SessionMaintenanceLine {
    type Error = &'static str;

    fn try_from(mut fields: MaintenanceFields) -> Result<SessionMaintenanceLine, &'static str> {
        Ok(SessionMaintenanceLine {
            course: course_of(fields.ended()?, fields.account)?,
            command: fields.command,
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
        for (n, turn) in session.turns.iter().enumerate() {
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
            lines.extend(worked_for_line(session, n, &mut stamper));
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
            lines.push(stamper.line(shown_at(entry), under.clone(), what));
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
            course: clone.course.clone(),
        });
        self.line(Some(at), Under::Session, what)
    }

    /// The line of `maintenance` at `at`, or with no time for one recorded
    /// before its start was.
    fn maintenance(&mut self, maintenance: &Maintenance, at: Option<Timestamp>) -> Line {
        let what = EntryLine::Maintenance(SessionMaintenanceLine {
            command: maintenance.command.clone(),
            course: maintenance.course().map(Clone::clone),
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

/// The line that ends the `turn`th turn, from 0, of `session`, once it has
/// ended.
fn worked_for_line(session: &Session, turn: usize, stamper: &mut Stamper) -> Option<Line> {
    let span = Span::of(session.worked_for(turn)?)?;
    let at = session.turn_ended_at(turn);
    Some(stamper.line(at, Under::Session, EntryLine::WorkedFor(span)))
}

/// When `entry` happened as its line shows it: a thought once it ended,
/// where that is known.
fn shown_at(entry: &Entry) -> Option<Timestamp> {
    match &entry.what {
        What::Thought(thought) => thought.ended_at().or(entry.at),
        What::Message(_) | What::Context(_) | What::Tool(_) | What::Event(_) | What::Other(_) => {
            entry.at
        }
    }
}

fn entry_line(entry: &Entry, session: &Session, verbosity: Verbosity) -> Option<EntryLine> {
    let full = verbosity == Verbosity::Full;
    Some(match &entry.what {
        What::Message(text) => EntryLine::Message { text: text.clone() },
        What::Thought(thought) => EntryLine::Thought(ThoughtLine {
            span: Span::of(thought.lasted(entry.at))?,
            in_full: match &thought.held {
                _ if !full => None,
                Held::Text(text) if text.trim().is_empty() => None,
                Held::Text(text) => Some(ThoughtInFull::Text(text.clone())),
                Held::Redacted(block) => Some(ThoughtInFull::Other(block.clone())),
            },
        }),
        What::Context(text) if full => EntryLine::Context { text: text.clone() },
        What::Event(event) if full => EntryLine::Event {
            event: event.clone(),
        },
        What::Other(other) if full => EntryLine::Other {
            other: other.clone(),
        },
        What::Context(_) | What::Event(_) | What::Other(_) => return None,
        What::Tool(call) => EntryLine::Tool(ToolLine {
            tool_use_id: call.id.clone(),
            tool: call.tool.name().to_string(),
            argument: call.argument(session.dir.as_deref()),
            account: composed(call.account()),
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
        Change::TurnEnded(turn) => return worked_for_line(session, turn, stamper),
        Change::Added(place) => (place, None),
        Change::Ended(place, ended_at) => (place, Some(ended_at)),
        Change::WaitBegan(_) | Change::WaitEnded(_) | Change::SessionEnded => return None,
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
    Some(stamper.line(
        ended_at.unwrap_or(shown_at(entry)),
        under(session, &at),
        what,
    ))
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
        EntryLine::Thought(ThoughtLine { span, in_full }) => (
            paint.yellow(span.thought()),
            match in_full {
                None => vec![],
                Some(ThoughtInFull::Text(text)) => indented("", text),
                Some(ThoughtInFull::Other(other)) => vec![format!("  other: {other}")],
            },
        ),
        EntryLine::WorkedFor(span) => (span.worked_for(), vec![]),
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
        EntryLine::Clone(CloneLine { course }) => match course.ended_or_account() {
            Err(account) => (format!("clone → {account}"), vec![]),
            Ok(ended) => (
                format!("clone → {}", clone_account(ended)),
                ended.error.as_deref().map_or(vec![], |error| {
                    let red: Vec<String> = error.lines().map(|part| paint.red(part)).collect();
                    indented("", &red.join("\n"))
                }),
            ),
        },
        EntryLine::Maintenance(SessionMaintenanceLine { command, course }) => {
            let command = shown_command(command);
            let (account, output) = match course.ended_or_account() {
                Err(account) => (account.to_string(), vec![]),
                Ok(ended) => (
                    if ended.status.failed() {
                        paint.red(maintenance_account(ended))
                    } else {
                        maintenance_account(ended)
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

/// `account` as a line says it, with the duration the agent reported.
pub(crate) fn composed(account: Account) -> String {
    match account.reported_seconds {
        Some(seconds) => with_duration(&account.said, seconds),
        None => account.said,
    }
}

/// How a maintenance command or a build ended and how long it ran, as its
/// line says them.
pub(crate) fn maintenance_account(ended: &MaintenanceEnded) -> String {
    with_duration(&ended.account().said, whole_seconds(ended.duration_seconds))
}

/// How long the making of a clone took, as its line says it: its line says
/// nothing else, its error showing under it.
pub(crate) fn clone_account(ended: &CloneEnded) -> String {
    with_duration("", whole_seconds(ended.duration_seconds))
}

/// `said`, then `seconds` as a duration.
fn with_duration(said: &str, seconds: u64) -> String {
    let said = if said.is_empty() {
        Phrase::default()
    } else {
        Phrase::from(format!("{said}, "))
    };
    said.duration(seconds, None).to_string()
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
    lines(transcript, verbosity)
        .iter()
        .filter_map(json_line)
        .collect()
}

/// `line` as `logs --json` gives it, ending in a newline, following the
/// agent's own JSON output: none of a turn's "worked for" line, and of a
/// thought only its text, in full, where its record holds any
/// (`docs/specs/monitoring.md`, logs, The transcripts).
pub fn json_line(line: &Line) -> Option<String> {
    let json = match line {
        Line::Entry(stamped) => match &stamped.what {
            EntryLine::Thought(ThoughtLine {
                in_full: Some(ThoughtInFull::Text(text)),
                ..
            }) => serde_json::to_string(&Thinking {
                stamp: &stamped.stamp,
                text,
            }),
            EntryLine::Thought(ThoughtLine {
                in_full: Some(ThoughtInFull::Other(other)),
                ..
            }) => serde_json::to_string(&Line::Entry(Box::new(Stamped {
                stamp: stamped.stamp.clone(),
                what: EntryLine::Other {
                    other: other.clone(),
                },
            }))),
            EntryLine::Thought(_) | EntryLine::WorkedFor(_) => return None,
            EntryLine::Brief { .. }
            | EntryLine::FollowUp { .. }
            | EntryLine::Message { .. }
            | EntryLine::Context { .. }
            | EntryLine::Event { .. }
            | EntryLine::Other { .. }
            | EntryLine::Tool(_)
            | EntryLine::Clone(_)
            | EntryLine::Maintenance(_) => serde_json::to_string(line),
        },
        Line::Header(_) => serde_json::to_string(line),
    };
    Some(json.expect("the lines serialize") + "\n")
}

/// `line` whole as JSON, as the API's events carry it for the CLI to show as
/// text or as JSON.
pub fn wire_line(line: &Line) -> String {
    serde_json::to_string(line).expect("the lines serialize")
}

/// A thought as the agent's own JSON output gives it.
#[derive(Serialize)]
#[serde(tag = "type", rename = "thinking")]
struct Thinking<'a> {
    #[serde(flatten)]
    stamp: &'a Stamp,
    text: &'a str,
}
