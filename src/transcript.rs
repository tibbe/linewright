//! A run's transcripts (`docs/specs/design_system.md`, Transcripts): its
//! sessions' records, read into what they say and followed as they grow,
//! for each place to lay out its own way.

pub mod lines;

use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::Timestamp;
use crate::runs::{Ran, Started};

pub use lines::{
    FollowedLine, Follower, Line, Verbosity, json_line, json_lines, render, render_line, wire_line,
};

pub const OUTPUT: &str = "output.jsonl";

pub const BRIEF: &str = "brief.md";

/// The file in a session's record that holds its `n`th follow-up, from 1.
pub fn followup_file(n: usize) -> String {
    format!("followup-{n}.md")
}

/// The file in a session's record that holds its maintenance command's output,
/// redacted, as it ran.
pub const MAINTENANCE_OUTPUT: &str = "setup.log";

/// The file in a session's record that held [`MaintenanceRecord`], written when
/// the maintenance command ended, before its start and end were events in
/// [`OUTPUT`].
const MAINTENANCE_RECORD: &str = "setup.json";

/// How a session's maintenance command ran (`docs/specs/runs.md`, Maintenance), as a
/// session recorded before its start and end were events kept it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaintenanceRecord {
    pub command: String,
    pub status: MaintenanceStatus,
    pub duration_seconds: f64,
}

/// How a maintenance command ended.
pub use crate::bash_command::Status as MaintenanceStatus;

impl MaintenanceStatus {
    /// Whether the command failed (`docs/specs/design_system.md`,
    /// Transcripts, Maintenance): a cancelled one did not.
    pub fn failed(self) -> bool {
        !matches!(
            self,
            MaintenanceStatus::Exited(0) | MaintenanceStatus::Cancelled
        )
    }

    pub(crate) fn account(self) -> String {
        match self {
            MaintenanceStatus::Exited(code) => format!("exit {code}"),
            MaintenanceStatus::Signalled(signal) => format!("signal {signal}"),
            MaintenanceStatus::TimedOut => "timed out".into(),
            MaintenanceStatus::Cancelled => "cancelled".into(),
        }
    }
}

/// A session's maintenance command, its first entry
/// (`docs/specs/design_system.md`, Transcripts, Maintenance).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Maintenance {
    /// Redacted.
    pub command: String,
    pub ran: MaintenanceRan,
}

/// How far a maintenance command has run, as its record says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaintenanceRan {
    /// It started `at`, and has run as far as `course` says.
    Started {
        at: Timestamp,
        course: Course<MaintenanceEnded>,
    },
    /// It ended, recorded before its start was, so when is not known.
    Recorded(MaintenanceEnded),
}

impl Maintenance {
    /// When it started, if that is known.
    pub(crate) fn at(&self) -> Option<Timestamp> {
        match &self.ran {
            MaintenanceRan::Started { at, .. } => Some(*at),
            MaintenanceRan::Recorded(_) => None,
        }
    }

    /// How far it has run.
    pub(crate) fn course(&self) -> Course<&MaintenanceEnded> {
        match &self.ran {
            MaintenanceRan::Started { course, .. } => course.as_ref(),
            MaintenanceRan::Recorded(ended) => Course::Ended(ended),
        }
    }
}

/// How far an entry that runs has run (`docs/specs/design_system.md`,
/// Transcripts, Running).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Course<T> {
    Running,
    /// It ended, as `T` says.
    Ended(T),
    /// Its session ended first.
    Interrupted,
}

impl<T> Course<T> {
    pub fn as_ref(&self) -> Course<&T> {
        match self {
            Course::Running => Course::Running,
            Course::Ended(ended) => Course::Ended(ended),
            Course::Interrupted => Course::Interrupted,
        }
    }

    /// How it ended, once it has, unless its session ended first.
    pub fn ended(&self) -> Option<&T> {
        match self {
            Course::Ended(ended) => Some(ended),
            Course::Running | Course::Interrupted => None,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Course<U> {
        match self {
            Course::Running => Course::Running,
            Course::Ended(ended) => Course::Ended(f(ended)),
            Course::Interrupted => Course::Interrupted,
        }
    }

    /// How it ended, or else the account of an entry that has not:
    /// `running` or `interrupted`.
    pub(crate) fn ended_or_account(&self) -> Result<&T, &'static str> {
        match self {
            Course::Ended(ended) => Ok(ended),
            Course::Running => Err(RUNNING),
            Course::Interrupted => Err(INTERRUPTED),
        }
    }

    fn running() -> Course<T> {
        Course::Running
    }

    /// Ends it as interrupted, unless it has ended: whether it was running.
    fn interrupt(&mut self) -> bool {
        let running = matches!(self, Course::Running);
        if running {
            *self = Course::Interrupted;
        }
        running
    }
}

/// The account of an entry that runs still.
pub(crate) const RUNNING: &str = "running";

/// The account of an entry its session ended first.
pub(crate) const INTERRUPTED: &str = "interrupted";

/// How long an entry that runs ran (`docs/specs/design_system.md`,
/// Transcripts, Running), each where the times it is told from are known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lasted {
    /// It still runs, since `since`.
    Running {
        since: Option<Timestamp>,
    },
    Took {
        seconds: Option<f64>,
    },
    /// Its session ended first, `seconds` after it started.
    Interrupted {
        seconds: Option<f64>,
    },
}

impl Lasted {
    /// How long something that started `start` and ended `end` lasted,
    /// where both are known, and whether it was `cut_short`.
    fn ended(start: Option<Timestamp>, end: Option<Timestamp>, cut_short: bool) -> Lasted {
        let seconds = start
            .zip(end)
            .map(|(start, end)| seconds_between(start, end));
        if cut_short {
            Lasted::Interrupted { seconds }
        } else {
            Lasted::Took { seconds }
        }
    }
}

/// What an entry's line says of how it went, leaving out how long it ran,
/// with the duration the agent reported, if any, such as a sub-agent's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub said: String,
    pub reported_seconds: Option<u64>,
}

impl Account {
    fn said(said: impl Into<String>) -> Account {
        Account {
            said: said.into(),
            reported_seconds: None,
        }
    }
}

/// How a maintenance command ended, with its output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaintenanceEnded {
    pub status: MaintenanceStatus,
    pub duration_seconds: f64,
    /// Redacted, as [`MAINTENANCE_OUTPUT`] holds it.
    pub output: String,
}

impl MaintenanceEnded {
    /// Whether its output shows from the start: in `full`, or when it
    /// failed (`docs/specs/design_system.md`, Transcripts, Maintenance).
    pub(crate) fn output_shown(&self, full: bool) -> bool {
        full || self.status.failed()
    }

    /// How it ended, leaving out how long it ran.
    pub(crate) fn account(&self) -> Account {
        Account::said(self.status.account())
    }
}

/// The lines Linewright writes to [`OUTPUT`] as a session's maintenance command
/// starts and ends, before the harness writes any.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum MaintenanceEvent {
    #[serde(rename = "linewright-setup")]
    Started { at: Timestamp, command: String },
    #[serde(rename = "linewright-setup-ended")]
    Ended {
        at: Timestamp,
        status: MaintenanceStatus,
        duration_seconds: f64,
    },
}

/// The line that says the maintenance `command`, redacted, started `at`.
pub fn maintenance_started(at: Timestamp, command: &str) -> String {
    serde_json::to_string(&MaintenanceEvent::Started {
        at,
        command: command.to_string(),
    })
    .expect("a maintenance event serializes")
}

/// The line that says the maintenance command ended `at`, as `status`, having run
/// for `duration_seconds`: written once [`MAINTENANCE_OUTPUT`] holds all its
/// output.
pub fn maintenance_ended(
    at: Timestamp,
    status: MaintenanceStatus,
    duration_seconds: f64,
) -> String {
    serde_json::to_string(&MaintenanceEvent::Ended {
        at,
        status,
        duration_seconds,
    })
    .expect("a maintenance event serializes")
}

/// Whether `line` of [`OUTPUT`] says the maintenance command ended.
fn says_maintenance_ended(line: &str) -> bool {
    matches!(
        serde_json::from_str(line),
        Ok(MaintenanceEvent::Ended { .. })
    )
}

/// The making of a session's clone, its first entry
/// (`docs/specs/design_system.md`, Transcripts, The clone).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionClone {
    /// When it started being made.
    pub at: Timestamp,
    pub course: Course<CloneEnded>,
}

/// How the making of a session's clone ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloneEnded {
    pub duration_seconds: f64,
    /// Why it could not be made, redacted, or `None` when it was made.
    pub error: Option<String>,
}

/// `seconds` as the whole seconds a line shows: none for a time that is not
/// one.
pub(crate) fn whole_seconds(seconds: f64) -> u64 {
    std::time::Duration::try_from_secs_f64(seconds).map_or(0, |duration| duration.as_secs())
}

/// The seconds from `start` to `end`: none when `end` comes first, as it can
/// on clocks that disagree.
fn seconds_between(start: Timestamp, end: Timestamp) -> f64 {
    (end - start)
        .to_std()
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// The lines Linewright writes to [`OUTPUT`] as a session's clone starts
/// being made and ends, before any other.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum CloneEvent {
    #[serde(rename = "linewright-clone")]
    Started { at: Timestamp },
    #[serde(rename = "linewright-clone-ended")]
    Ended {
        at: Timestamp,
        duration_seconds: f64,
        error: Option<String>,
    },
}

/// The line that says the session's clone started being made `at`.
pub fn clone_started(at: Timestamp) -> String {
    serde_json::to_string(&CloneEvent::Started { at }).expect("a clone event serializes")
}

/// The line that says the session's clone ended `at`, having taken
/// `duration_seconds`, made unless there is an `error`, which the caller has
/// redacted and scrubbed of the clone's path.
pub fn clone_ended(at: Timestamp, duration_seconds: f64, error: Option<&str>) -> String {
    serde_json::to_string(&CloneEvent::Ended {
        at,
        duration_seconds,
        error: error.map(str::to_string),
    })
    .expect("a clone event serializes")
}

/// The lines Linewright writes to [`OUTPUT`] as the session waits out the
/// usage limit and as its work stops.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "type")]
enum SessionEvent {
    #[serde(rename = "linewright-usage-limit")]
    Waiting { at: Timestamp, until: Timestamp },
    #[serde(rename = "linewright-usage-limit-ended")]
    Waited { at: Timestamp },
    #[serde(rename = "linewright-session-ended")]
    Ended { at: Timestamp },
}

/// The line that says the session ended `at`, whatever its outcome.
pub fn session_ended(at: Timestamp) -> String {
    serde_json::to_string(&SessionEvent::Ended { at }).expect("a session event serializes")
}

/// The line that says the session started `at` to wait out the usage limit
/// `until` then.
pub fn usage_limit_waiting(at: Timestamp, until: Timestamp) -> String {
    serde_json::to_string(&SessionEvent::Waiting { at, until }).expect("a session event serializes")
}

/// The line that says the session's wait for the usage limit ended `at`.
pub fn usage_limit_waited(at: Timestamp) -> String {
    serde_json::to_string(&SessionEvent::Waited { at }).expect("a session event serializes")
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

/// The line the harness writes to [`OUTPUT`] once a turn's output has ended.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename = "linewright-turn-ended")]
struct TurnEndedMarker {
    at: Timestamp,
}

/// The line that says the agent's output for its turn ended `at`.
pub fn turn_ended_marker(at: Timestamp) -> String {
    serde_json::to_string(&TurnEndedMarker { at }).expect("a marker serializes")
}

#[derive(Clone, Debug, Default)]
pub struct SessionRecord {
    pub output: String,
    pub brief: Option<String>,
    pub followups: Vec<String>,
    /// What the session ran with, from its run's record: `None` for a
    /// session recorded before Linewright kept it.
    pub ran: Option<Ran>,
    /// Its maintenance command's output, once the command has ended.
    pub maintenance_output: Option<String>,
    /// Its maintenance command, of a session recorded before the command's start
    /// and end were events in [`OUTPUT`], which held it once it ended.
    pub maintenance: Option<MaintenanceRecord>,
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
        if self.maintenance.is_none()
            && let Some(record) = read(MAINTENANCE_RECORD)?
        {
            self.maintenance =
                Some(serde_json::from_str(&record).with_context(|| {
                    format!("reading {}", dir.join(MAINTENANCE_RECORD).display())
                })?);
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
        drop_character_being_written(&mut added);
        let partial_line_start = self.output.rfind('\n').map_or(0, |end| end + 1);
        self.output.push_str(&String::from_utf8_lossy(&added));
        let maintenance_ended = self.maintenance.is_some()
            || whole_lines(&self.output[partial_line_start..])
                .lines()
                .any(says_maintenance_ended);
        if self.maintenance_output.is_none() && maintenance_ended {
            self.maintenance_output = Some(read(MAINTENANCE_OUTPUT)?.unwrap_or_default());
        }
        Ok(())
    }
}

/// `bytes` of UTF-8 without the character they end part-way through, if
/// any.
fn drop_character_being_written(bytes: &mut Vec<u8>) {
    if let Err(e) = std::str::from_utf8(bytes)
        && e.error_len().is_none()
    {
        bytes.truncate(e.valid_up_to());
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
    /// When its clone started being made, if it says so, or else when its
    /// maintenance command started, if it ran one that says so, or else when
    /// its first turn did.
    pub started_at: Option<Timestamp>,
    pub dir: Option<String>,
    /// Its first entry, unless it was recorded before the clone was one.
    #[serde(default)]
    pub clone: Option<SessionClone>,
    /// Its entry before its turns, if it ran a maintenance command.
    pub maintenance: Option<Maintenance>,
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub end: End,
    /// Its waits for the usage limit to reset, in order.
    #[serde(default)]
    pub waits: Vec<Wait>,
}

/// Whether a session or a wait in it has ended, and when.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum End {
    #[default]
    Going,
    At(Timestamp),
    /// It ended, but when is not known, as when a restart interrupted its
    /// session's run.
    NotKnown,
}

impl End {
    fn at(self) -> Option<Timestamp> {
        match self {
            End::At(at) => Some(at),
            End::Going | End::NotKnown => None,
        }
    }
}

/// A session's wait for the usage limit to reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    pub at: Timestamp,
    /// When it was to end.
    pub until: Timestamp,
    pub end: End,
}

impl Session {
    /// The entry `at`.
    ///
    /// # Panics
    ///
    /// When there is no entry `at`, as there is for each place a change
    /// names.
    pub fn entry(&self, at: &EntryAt) -> &Entry {
        let mut entries = &self.turns[at.turn].entries;
        let (last, holders) = at.path.split_last().expect("an entry's path is not empty");
        for &i in holders {
            let What::Tool(call) = &entries[i].what else {
                unreachable!("a path leads through calls only")
            };
            entries = &call.entries;
        }
        &entries[*last]
    }

    /// How long the entry at `place` ran, unless it is not one that runs.
    pub fn lasted(&self, place: &Place) -> Option<Lasted> {
        let (since, course) = match place {
            Place::Clone => {
                let clone = self.clone.as_ref()?;
                let took = clone
                    .course
                    .as_ref()
                    .map(|ended| Some(ended.duration_seconds));
                (Some(clone.at), took)
            }
            Place::Maintenance => {
                let maintenance = self.maintenance.as_ref()?;
                let took = maintenance
                    .course()
                    .map(|ended| Some(ended.duration_seconds));
                (maintenance.at(), took)
            }
            Place::Entry(at) => {
                let entry = self.entry(at);
                let call = match &entry.what {
                    What::Tool(call) => call,
                    What::Thought(thought) => return Some(thought.lasted(entry.at)),
                    What::Message(_) | What::Context(_) | What::Event(_) | What::Other(_) => {
                        return None;
                    }
                };
                let took = call.end.map(|end| {
                    entry
                        .at
                        .zip(end)
                        .map(|(start, end)| seconds_between(start, end))
                });
                (entry.at, took)
            }
        };
        Some(match course {
            Course::Running => Lasted::Running { since },
            Course::Ended(seconds) => Lasted::Took { seconds },
            Course::Interrupted => Lasted::Interrupted {
                seconds: since
                    .zip(self.end.at())
                    .map(|(start, end)| seconds_between(start, end)),
            },
        })
    }
    /// How long its `turn`th turn, from 0, took, once it has ended: from
    /// its start to the latest time of its entries or, where its session
    /// ended first, to that end (`docs/specs/design_system.md`,
    /// Transcripts, A turn's length).
    pub fn worked_for(&self, n: usize) -> Option<Lasted> {
        let turn = &self.turns[n];
        let to = match turn.end {
            Course::Running => return None,
            Course::Ended(_) => turn.entries.iter().filter_map(Entry::last_at).max(),
            Course::Interrupted => self.turn_ended_at(n),
        };
        let cut_short = turn.end == Course::Interrupted;
        Some(Lasted::ended(turn.started_at, to, cut_short))
    }

    /// When its `turn`th turn, from 0, ended, if it has and that is known:
    /// where its session ended first, when the session did.
    pub(crate) fn turn_ended_at(&self, turn: usize) -> Option<Timestamp> {
        match self.turns[turn].end {
            Course::Running => None,
            Course::Ended(at) => at,
            Course::Interrupted => self.end.at(),
        }
    }

    /// How long its last turn has gone on so far, leaving out its waits for
    /// the usage limit to reset, while it goes on and when it started is
    /// known (`docs/specs/design_system.md`, The run page, Working).
    pub fn turn_so_far(&self) -> Option<SessionLasted> {
        let turn = self
            .turns
            .last()
            .filter(|turn| turn.end == Course::Running)?;
        turn.started_at.map(|start| self.lasted_from(start))
    }

    /// How long it has run so far, leaving out its waits for the usage
    /// limit to reset, unless when it started is not known
    /// (`docs/specs/design_system.md`, The run page, A session's heading).
    pub fn lasted_so_far(&self) -> Option<SessionLasted> {
        self.started_at.map(|start| self.lasted_from(start))
    }

    /// How long it has run from `from`, leaving out its waits for the usage
    /// limit to reset: up to its end or, where that is not known, to its last
    /// entry; while it goes on, to now, holding still while it waits.
    pub fn lasted_from(&self, from: Timestamp) -> SessionLasted {
        let to = match self.end {
            End::At(at) => at,
            End::NotKnown => self.last_at().map_or(from, |last| last.max(from)),
            End::Going => match self.waiting() {
                Some(wait) => wait.at.max(from),
                None => {
                    return SessionLasted::Going {
                        since: from + self.waited(from, None),
                    };
                }
            },
        };
        let lasted = (to - from - self.waited(from, Some(to))).max(TimeDelta::zero());
        SessionLasted::Held {
            seconds: lasted.as_seconds_f64(),
        }
    }

    /// Its wait for the usage limit under way, if any.
    pub fn waiting(&self) -> Option<&Wait> {
        self.waits.last().filter(|wait| wait.end == End::Going)
    }

    /// How long it waited for the usage limit between `from` and `to`, or
    /// since `from` where `to` is not given: a wait whose end is not known
    /// lasting to `to`.
    fn waited(&self, from: Timestamp, to: Option<Timestamp>) -> TimeDelta {
        self.waits
            .iter()
            .filter_map(|wait| {
                let end = match (wait.end.at(), to) {
                    (Some(end), Some(to)) => end.min(to),
                    (end, to) => end.or(to)?,
                };
                Some((end - wait.at.max(from)).max(TimeDelta::zero()))
            })
            .sum()
    }

    /// When the latest of its entries happened, as it began or ended, or
    /// else when its latest turn began, if any of those is known.
    pub fn last_at(&self) -> Option<Timestamp> {
        let clone = self.clone.as_ref().map(|clone| {
            let took = clone.course.ended().map(|ended| ended.duration_seconds);
            last_of(clone.at, took)
        });
        let maintenance = self.maintenance.as_ref().and_then(|maintenance| {
            let took = maintenance
                .course()
                .ended()
                .map(|ended| ended.duration_seconds);
            maintenance.at().map(|at| last_of(at, took))
        });
        let turns = self.turns.iter().flat_map(|turn| {
            std::iter::once(turn.started_at).chain(turn.entries.iter().map(Entry::last_at))
        });
        [clone, maintenance]
            .into_iter()
            .chain(turns)
            .flatten()
            .max()
    }
}

/// How long a session has run, leaving out its waits for the usage limit to
/// reset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SessionLasted {
    /// It goes on: as long as since `since`, its start moved on by how long
    /// it waited.
    Going { since: Timestamp },
    /// It has ended, or waits for the usage limit, having run `seconds`.
    Held { seconds: f64 },
}

impl SessionLasted {
    /// How long it has run `now`, in seconds.
    pub fn seconds(self, now: Timestamp) -> f64 {
        match self {
            SessionLasted::Going { since } => seconds_between(since, now),
            SessionLasted::Held { seconds } => seconds,
        }
    }
}

impl From<SessionLasted> for Lasted {
    /// As an entry that runs from where the session's duration is told
    /// from: running while the session goes on, else having taken as long.
    fn from(lasted: SessionLasted) -> Lasted {
        match lasted {
            SessionLasted::Going { since } => Lasted::Running { since: Some(since) },
            SessionLasted::Held { seconds } => Lasted::Took {
                seconds: Some(seconds),
            },
        }
    }
}

/// The latest time known of an entry that started `at` and, once it ended,
/// `took` that many seconds: when it ended, or else when it started.
fn last_of(at: Timestamp, took: Option<f64>) -> Timestamp {
    let took = std::time::Duration::try_from_secs_f64(took.unwrap_or_default())
        .ok()
        .and_then(|duration| TimeDelta::from_std(duration).ok())
        .unwrap_or_default();
    at + took
}

impl Entry {
    /// When the latest of it and the work of a sub-agent it started
    /// happened, as each began or ended, if that is known.
    fn last_at(&self) -> Option<Timestamp> {
        let call = match &self.what {
            What::Tool(call) => call,
            What::Thought(thought) => return self.at.max(thought.ended_at()),
            What::Message(_) | What::Context(_) | What::Event(_) | What::Other(_) => {
                return self.at;
            }
        };
        let ended = call.end.ended().copied().flatten();
        [self.at, ended]
            .into_iter()
            .chain(call.entries.iter().map(Entry::last_at))
            .flatten()
            .max()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub started_at: Option<Timestamp>,
    pub prompt: Prompt,
    pub entries: Vec<Entry>,
    /// It ends when the agent's output for it ends, at the time its record
    /// gives, if any, or is interrupted when its session ends first.
    #[serde(default = "Course::running")]
    pub end: Course<Option<Timestamp>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Prompt {
    Brief(Option<String>),
    FollowUp(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// When it happened or, for a thought, when it started: when the last
    /// thing before it in its agent's work happened.
    pub at: Option<Timestamp>,
    pub what: What,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum What {
    Message(String),
    Thought(Thought),
    Context(String),
    Tool(Call),
    Event(Value),
    Other(Value),
}

/// Where the agent thought before what it did next
/// (`docs/specs/design_system.md`, Transcripts, Thinking).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thought {
    pub held: Held,
    pub end: ThoughtEnd,
}

/// What of a thought its record holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Held {
    /// Its text, empty where its record holds none of it.
    Text(String),
    /// The agent's block that holds it encrypted, as the agent gave it.
    Redacted(Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThoughtEnd {
    /// Its thinking ended, at the time its record gives, if any.
    Ended(Option<Timestamp>),
    /// Its turn ended first, at the time given, if that is known.
    CutShort(Option<Timestamp>),
}

impl Thought {
    /// When it ended, if that is known.
    pub(crate) fn ended_at(&self) -> Option<Timestamp> {
        match self.end {
            ThoughtEnd::Ended(at) | ThoughtEnd::CutShort(at) => at,
        }
    }

    /// Whether its turn ended first.
    pub(crate) fn cut_short(&self) -> bool {
        matches!(self.end, ThoughtEnd::CutShort(_))
    }

    /// Its text, empty where its record holds none of it.
    pub(crate) fn text(&self) -> &str {
        match &self.held {
            Held::Text(text) => text,
            Held::Redacted(_) => "",
        }
    }

    /// How long it went on, from `at`, its start.
    fn lasted(&self, at: Option<Timestamp>) -> Lasted {
        Lasted::ended(at, self.ended_at(), self.cut_short())
    }
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
    /// It ends when it returns or, for one that went on in the background,
    /// when it ends there, at the time its record gives, if any.
    #[serde(default = "Course::running")]
    pub end: Course<Option<Timestamp>>,
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

impl CallResult {
    /// What it returned besides its text, unless it returned nothing else.
    pub(crate) fn content(&self) -> Option<&[Value]> {
        Some(self.other.as_slice()).filter(|other| !other.is_empty())
    }

    /// Its details, unless it returned none.
    pub(crate) fn details(&self) -> Option<&Value> {
        Some(&self.details).filter(|details| !details.is_null())
    }
}

impl Call {
    /// Its main argument, as its line shows it, with the session's `dir`
    /// left out of paths.
    pub(crate) fn argument(&self, dir: Option<&str>) -> String {
        let value = self
            .tool
            .main_argument()
            .and_then(|key| self.input[key].as_str())
            .or_else(|| {
                self.input
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

    /// The status its sub-agent returned, for a call that started one and
    /// returned no error.
    fn sub_agent_status(&self) -> Option<&str> {
        let result = self.result.as_ref().filter(|result| !result.error)?;
        matches!(self.tool, Tool::Agent | Tool::Task)
            .then(|| result.details["status"].as_str())
            .flatten()
    }

    /// Whether it went on in the background as it returned.
    fn goes_on_in_background(&self) -> bool {
        let bash = self.tool == Tool::Bash
            && self.result.as_ref().is_some_and(|result| {
                !result.error && result.details["backgroundTaskId"].is_string()
            });
        bash || self.sub_agent_status() == Some("async_launched")
    }

    /// How it went: `running` until it returns, and `interrupted` once its
    /// session ended first.
    pub fn account(&self) -> Account {
        if self.end == Course::Interrupted {
            return Account::said(INTERRUPTED);
        }
        let Some(result) = &self.result else {
            return Account::said(RUNNING);
        };
        let sub_agent_done = self.sub_agent_status() == Some("completed");
        Account {
            said: self.returned(result),
            reported_seconds: sub_agent_done
                .then(|| result.details["totalDurationMs"].as_u64().unwrap_or(0) / 1000),
        }
    }

    fn returned(&self, result: &CallResult) -> String {
        let details = &result.details;
        let lines = |n: usize| format!("{n} {}", if n == 1 { "line" } else { "lines" });
        let in_background = |started: &str| match &self.background {
            Some(status) => format!("{status} in the background"),
            None => started.to_string(),
        };
        if result.error {
            let text = result.text.trim();
            if self.tool == Tool::Bash
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
        match self.tool {
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
                    self.input["content"]
                        .as_str()
                        .unwrap_or_default()
                        .lines()
                        .count()
                )
            ),
            Tool::Edit | Tool::MultiEdit | Tool::NotebookEdit => "edited".into(),
            Tool::Skill => "loaded".into(),
            Tool::Agent | Tool::Task => match self.sub_agent_status() {
                Some("completed") => {
                    let uses = details["totalToolUseCount"].as_u64().unwrap_or(0);
                    let uses_word = if uses == 1 { "use" } else { "uses" };
                    format!("done, {uses} tool {uses_word}")
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

    /// Whether it has returned an error.
    pub(crate) fn failed(&self) -> bool {
        self.result.as_ref().is_some_and(|result| result.error)
    }

    /// Its parts as a transcript in full shows them, each with its label.
    pub(crate) fn parts(&self) -> Vec<(&'static str, String)> {
        parts(&self.input, self.result.as_ref())
    }
}

/// A call's parts as a transcript in full shows them, each with its label:
/// its `input`, and once it has `returned`, its result, and its content and
/// details unless it returned none.
pub(crate) fn parts(input: &Value, returned: Option<&CallResult>) -> Vec<(&'static str, String)> {
    let pretty = |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default();
    let mut parts = vec![("input", pretty(input))];
    if let Some(returned) = returned {
        parts.push(("result", returned.text.clone()));
        if let Some(content) = returned.content() {
            let content: Vec<String> = content.iter().map(Value::to_string).collect();
            parts.push(("content", content.join("\n")));
        }
        if let Some(details) = returned.details() {
            parts.push(("details", pretty(details)));
        }
    }
    parts
}

/// How much of a session's record has been written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    /// It may yet grow.
    SoFar,
    /// It is whole, and the session has ended, when its record says or else
    /// at a time not known.
    Whole,
}

impl Written {
    /// How much has been written of the record of the `session`th of a run's
    /// `sessions` sessions, from 1: all of it once the run has `ended` or a
    /// later session has started.
    pub fn of(session: usize, sessions: usize, ended: bool) -> Written {
        if ended || session < sessions {
            Written::Whole
        } else {
            Written::SoFar
        }
    }
}

/// Lines that do not read as events, such as one being written, are skipped.
pub fn parse(record: &SessionRecord, written: Written) -> Session {
    let mut parser = Parser::default();
    parser.recorded_maintenance(record);
    for line in record.output.lines() {
        parser.read(line, record);
    }
    if written == Written::Whole {
        parser.end(None);
    }
    parser.session
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
    changed: Vec<Change>,
    /// When the latest event that said so happened, or else its turn
    /// started.
    last_at: Option<Timestamp>,
    /// When the last thing in each agent's work this turn happened: its turn
    /// starting or, for a sub-agent, the call that started it, then each of
    /// the agent's events that says when it happened.
    threads: HashMap<Thread, Timestamp>,
    /// Whether the main agent has been thinking since the last thing in its
    /// work this turn.
    thinking: bool,
    /// Whether the agent has given its result for the latest turn.
    resulted: bool,
}

/// Whose work an event is of.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Thread {
    Main,
    /// The sub-agent the call with this ID started.
    SubAgent(String),
}

/// Where an entry is: its turn, and its path through the calls that hold
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EntryAt {
    pub turn: usize,
    pub path: Vec<usize>,
}

impl EntryAt {
    /// Where the call that holds it is, for an entry of a sub-agent's work.
    pub fn parent(&self) -> Option<EntryAt> {
        let (_, holders) = self.path.split_last()?;
        (!holders.is_empty()).then(|| EntryAt {
            turn: self.turn,
            path: holders.to_vec(),
        })
    }
}

/// A change to a session's transcript, as reading its records makes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// The session's transcript opened: its first line was read, or the
    /// session was whole with none.
    Opened,
    /// Its `n`th turn, from 0, began.
    Turn(usize),
    /// Its `n`th turn, from 0, ended, after the thought it cut short, if
    /// any, and before its session's end.
    TurnEnded(usize),
    Added(Place),
    /// An entry that runs ended, or was interrupted as its session ended,
    /// at the time its record gives, if any.
    Ended(Place, Option<Timestamp>),
    /// Its `n`th wait for the usage limit, from 0, began.
    WaitBegan(usize),
    /// Its `n`th wait for the usage limit, from 0, ended.
    WaitEnded(usize),
    /// The session ended, after each entry it interrupted.
    SessionEnded,
}

/// An entry that a change is of.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Clone,
    Maintenance,
    Entry(EntryAt),
}

impl Parser {
    /// Reads `line` of `record`'s output into the session.
    fn read(&mut self, line: &str, record: &SessionRecord) {
        self.line(line, record);
        self.session.started_at = started_at(&self.session);
    }

    fn line(&mut self, line: &str, record: &SessionRecord) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if let Ok(clone) = CloneEvent::deserialize(&event) {
            self.session_clone(clone);
            return;
        }
        if let Ok(maintenance) = MaintenanceEvent::deserialize(&event) {
            self.maintenance(maintenance, record);
            return;
        }
        if let Ok(session) = SessionEvent::deserialize(&event) {
            self.session_event(session);
            return;
        }
        if let Ok(TurnEndedMarker { at }) = TurnEndedMarker::deserialize(&event) {
            self.end_turn(Course::Ended(Some(at)));
            return;
        }
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
        let recorded_before_turns_were_marked = self.session.turns.is_empty();
        if recorded_before_turns_were_marked {
            self.begin_turn(None, Prompt::Brief(record.brief.clone()));
        }
        self.event(event);
    }

    /// Takes the maintenance command of a session recorded before its start and
    /// end were events, as having started with the session, unless it has
    /// been taken.
    fn recorded_maintenance(&mut self, record: &SessionRecord) {
        if self.session.maintenance.is_some() {
            return;
        }
        let Some(ran) = &record.maintenance else {
            return;
        };
        self.session.maintenance = Some(Maintenance {
            command: ran.command.clone(),
            ran: MaintenanceRan::Recorded(MaintenanceEnded {
                status: ran.status,
                duration_seconds: ran.duration_seconds,
                output: record.maintenance_output.clone().unwrap_or_default(),
            }),
        });
        self.changed.push(Change::Added(Place::Maintenance));
    }

    fn session_clone(&mut self, event: CloneEvent) {
        match event {
            CloneEvent::Started { at } => {
                self.session.clone = Some(SessionClone {
                    at,
                    course: Course::Running,
                });
                self.changed.push(Change::Added(Place::Clone));
                self.last_at = Some(at);
            }
            CloneEvent::Ended {
                at,
                duration_seconds,
                error,
            } => {
                let Some(clone) = &mut self.session.clone else {
                    return;
                };
                clone.course = Course::Ended(CloneEnded {
                    duration_seconds,
                    error,
                });
                self.changed.push(Change::Ended(Place::Clone, Some(at)));
                self.last_at = Some(at);
            }
        }
    }

    fn maintenance(&mut self, event: MaintenanceEvent, record: &SessionRecord) {
        match event {
            MaintenanceEvent::Started { at, command } => {
                self.session.maintenance = Some(Maintenance {
                    command,
                    ran: MaintenanceRan::Started {
                        at,
                        course: Course::Running,
                    },
                });
                self.changed.push(Change::Added(Place::Maintenance));
                self.last_at = Some(at);
            }
            MaintenanceEvent::Ended {
                at,
                status,
                duration_seconds,
            } => {
                let Some(Maintenance {
                    ran: MaintenanceRan::Started { course, .. },
                    ..
                }) = &mut self.session.maintenance
                else {
                    return;
                };
                *course = Course::Ended(MaintenanceEnded {
                    status,
                    duration_seconds,
                    output: record.maintenance_output.clone().unwrap_or_default(),
                });
                self.changed
                    .push(Change::Ended(Place::Maintenance, Some(at)));
                self.last_at = Some(at);
            }
        }
    }

    fn session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Waiting { at, until } => {
                self.end_turn(Course::Ended(None));
                self.session.waits.push(Wait {
                    at,
                    until,
                    end: End::Going,
                });
                self.changed
                    .push(Change::WaitBegan(self.session.waits.len() - 1));
            }
            SessionEvent::Waited { at } => self.end_wait(End::At(at)),
            SessionEvent::Ended { at } => self.end(Some(at)),
        }
    }

    /// Ends the session's wait under way, if any, as `end` says.
    fn end_wait(&mut self, end: End) {
        let n = self.session.waits.len().checked_sub(1);
        if let Some(n) = n
            && self.session.waits[n].end == End::Going
        {
            self.session.waits[n].end = end;
            self.changed.push(Change::WaitEnded(n));
        }
    }

    /// Ends the session `at`, or at a time not known, unless it has ended,
    /// interrupting each entry still running.
    fn end(&mut self, at: Option<Timestamp>) {
        if self.session.end != End::Going {
            return;
        }
        self.session.end = at.map_or(End::NotKnown, End::At);
        self.end_wait(self.session.end);
        if let Some(clone) = &mut self.session.clone
            && clone.course.interrupt()
        {
            self.changed.push(Change::Ended(Place::Clone, at));
        }
        if let Some(Maintenance {
            ran: MaintenanceRan::Started { course, .. },
            ..
        }) = &mut self.session.maintenance
            && course.interrupt()
        {
            self.changed.push(Change::Ended(Place::Maintenance, at));
        }
        let turn_end = if self.resulted {
            Course::Ended(None)
        } else {
            Course::Interrupted
        };
        let ended_turn = self.cut_turn(turn_end);
        for (turn, Turn { entries, .. }) in self.session.turns.iter_mut().enumerate() {
            interrupt(entries, turn, &mut vec![], at, &mut self.changed);
        }
        self.changed.extend(ended_turn.map(Change::TurnEnded));
        self.changed.push(Change::SessionEnded);
    }

    /// Ends the session's latest turn as `end` says, unless it has ended.
    fn end_turn(&mut self, end: Course<Option<Timestamp>>) {
        if let Some(n) = self.cut_turn(end) {
            self.changed.push(Change::TurnEnded(n));
        }
    }

    /// Ends the session's latest turn as `end` says, unless it has ended,
    /// first adding the main agent's thought under way, if any, as cut short
    /// then, but not noting the turn's end: giving the turn, if it ended.
    fn cut_turn(&mut self, end: Course<Option<Timestamp>>) -> Option<usize> {
        let n = self.session.turns.len().checked_sub(1)?;
        if self.session.turns[n].end != Course::Running {
            return None;
        }
        self.session.turns[n].end = end;
        if std::mem::take(&mut self.thinking) {
            let what = What::Thought(Thought {
                held: Held::Text(String::new()),
                end: ThoughtEnd::CutShort(self.session.turn_ended_at(n)),
            });
            let at = self.threads.get(&Thread::Main).copied();
            self.push(n, None, Entry { at, what });
        }
        Some(n)
    }

    fn begin_turn(&mut self, started_at: Option<Timestamp>, prompt: Prompt) {
        self.end_turn(Course::Ended(None));
        self.last_at = started_at.or(self.last_at);
        self.threads.clear();
        if let Some(at) = self.last_at {
            self.threads.insert(Thread::Main, at);
        }
        self.resulted = false;
        self.session.turns.push(Turn {
            started_at,
            prompt,
            entries: vec![],
            end: Course::Running,
        });
        self.changed
            .push(Change::Turn(self.session.turns.len() - 1));
    }

    fn event(&mut self, event: Value) {
        let turn = self.session.turns.len() - 1;
        let at = timestamp(&event["timestamp"]).or(self.last_at);
        self.last_at = at;
        let parent = event["parent_tool_use_id"]
            .as_str()
            .and_then(|id| self.calls.get(id))
            .filter(|call| call.turn == turn)
            .map(|call| call.path.clone());
        let parent = parent.as_deref();
        let thread = match parent {
            Some(_) => Thread::SubAgent(text(&event["parent_tool_use_id"])),
            None => Thread::Main,
        };
        let since = self.threads.get(&thread).copied();
        if !is_progress(&event)
            && let Some(said) = timestamp(&event["timestamp"])
        {
            let last = self.threads.entry(thread.clone()).or_insert(said);
            *last = said.max(*last);
            if thread == Thread::Main {
                self.thinking = false;
            }
        }
        let blocks = event["message"]["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        match event["type"].as_str() {
            Some("assistant") => {
                let thought_ended = timestamp(&event["timestamp"]);
                for block in blocks {
                    let thought = |held| {
                        What::Thought(Thought {
                            held,
                            end: ThoughtEnd::Ended(thought_ended),
                        })
                    };
                    let what = match block["type"].as_str() {
                        Some("text") => What::Message(text(&block["text"])),
                        // A thinking block can hold only a signature:
                        // <https://platform.claude.com/docs/en/build-with-claude/thinking#controlling-thinking-display>.
                        Some("thinking") => thought(Held::Text(text(&block["thinking"]))),
                        Some("redacted_thinking") => thought(Held::Redacted(block)),
                        Some("tool_use") => What::Tool(Call {
                            id: text(&block["id"]),
                            tool: Tool::from(text(&block["name"])),
                            input: block["input"].clone(),
                            result: None,
                            background: None,
                            entries: vec![],
                            end: Course::Running,
                        }),
                        _ => What::Other(block),
                    };
                    let at = if matches!(what, What::Thought(_)) {
                        since
                    } else {
                        at
                    };
                    let blank = matches!(&what, What::Message(t) if t.trim().is_empty());
                    if !blank {
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
                                if call.background.is_some() || !call.goes_on_in_background() {
                                    call.end = Course::Ended(at);
                                }
                                self.changed.push(Change::Ended(Place::Entry(returned), at));
                            }
                            continue;
                        }
                        Some("text") => What::Context(text(&block["text"])),
                        _ => What::Other(block),
                    };
                    self.push(turn, parent, Entry { at, what });
                }
                let several_results = results > 1;
                if several_results && !event["tool_use_result"].is_null() {
                    let details = serde_json::json!({"tool_use_result": event["tool_use_result"]});
                    let what = What::Other(details);
                    self.push(turn, parent, Entry { at, what });
                }
            }
            _ if is_progress(&event) => {
                let of_main_agent = event["parent_tool_use_id"].is_null();
                if of_main_agent && event["subtype"] == "thinking_tokens" {
                    self.thinking = true;
                }
            }
            _ => {
                if event["type"] == "result" {
                    self.resulted = true;
                }
                match event["subtype"].as_str() {
                    Some("init") => {
                        self.session.dir = event["cwd"].as_str().map(str::to_string);
                    }
                    Some("task_notification") => {
                        if let Some((ended, call)) = self.call(&event["tool_use_id"]) {
                            call.background = event["status"].as_str().map(str::to_string);
                            if call.result.is_some() {
                                call.end = Course::Ended(at);
                                self.changed.push(Change::Ended(Place::Entry(ended), at));
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
            if let Some(started) = entries.last().and_then(|call| call.at) {
                self.threads.insert(Thread::SubAgent(id.clone()), started);
            }
            self.calls.insert(id, at.clone());
        }
        self.changed.push(Change::Added(Place::Entry(at)));
    }

    fn call(&mut self, id: &Value) -> Option<(EntryAt, &mut Call)> {
        let at = self.calls.get(id.as_str()?)?.clone();
        let call = call_at(&mut self.session.turns[at.turn].entries, &at.path);
        Some((at, call))
    }
}

/// Interrupts each call still running among `entries`, at `path` in `turn`,
/// and among the work of their sub-agents, noting each change in `changed`
/// as ended `at`.
fn interrupt(
    entries: &mut [Entry],
    turn: usize,
    path: &mut Vec<usize>,
    at: Option<Timestamp>,
    changed: &mut Vec<Change>,
) {
    for (i, entry) in entries.iter_mut().enumerate() {
        let What::Tool(call) = &mut entry.what else {
            continue;
        };
        path.push(i);
        if call.end.interrupt() {
            let at_path = EntryAt {
                turn,
                path: path.clone(),
            };
            changed.push(Change::Ended(Place::Entry(at_path), at));
        }
        interrupt(&mut call.entries, turn, path, at, changed);
        path.pop();
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

/// Where something shown of a followed transcript, such as a line, was made:
/// the `nth`, from 1, of those made once the transcript was read to `read`.
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
    /// Whether what was made at `made` comes after `self`.
    fn precedes(self, made: Made) -> bool {
        match self {
            After::Line(line) => made > line,
            After::Read(read) => made.read > read,
        }
    }
}

/// Numbers what a layout makes of a followed transcript by where it was
/// made, giving only what comes after where it follows from.
pub struct Numbering {
    after: Option<After>,
    /// Where the last was made, so that what is made at the same point by a
    /// later read numbers on from it.
    last: Option<Made>,
}

impl Numbering {
    pub fn new(after: Option<After>) -> Numbering {
        Numbering { after, last: None }
    }

    /// Where the next thing made once the transcript was read to `read`
    /// was made, unless it comes no later than where this follows from.
    pub fn next(&mut self, read: ReadTo) -> Option<Made> {
        let nth = self
            .last
            .filter(|last| last.read == read)
            .map_or(0, |last| last.nth);
        let made = Made { read, nth: nth + 1 };
        self.last = Some(made);
        self.after
            .is_none_or(|after| after.precedes(made))
            .then_some(made)
    }
}

/// When a session's entries happened, as a place shows the time beside
/// each: something with no time of its own happened when what was shown
/// before it did.
pub(crate) struct Clock {
    started_at: Option<Timestamp>,
    last: Option<Timestamp>,
}

impl Clock {
    pub(crate) fn new(started_at: Option<Timestamp>) -> Clock {
        Clock {
            started_at,
            last: started_at,
        }
    }

    /// When the session started, if that is known.
    pub(crate) fn started_at(&self) -> Option<Timestamp> {
        self.started_at
    }

    /// That the session started at `at`, unless when is already known.
    pub(crate) fn started(&mut self, at: Option<Timestamp>) {
        if self.started_at.is_none() {
            self.started_at = at;
            self.last = self.last.or(at);
        }
    }

    /// When something shown that happened `at` happened, if that is known.
    pub(crate) fn tick(&mut self, at: Option<Timestamp>) -> Option<Timestamp> {
        let at = at.or(self.last);
        self.last = at;
        at
    }

    /// The time from the session's start to `at`, in seconds, if both are
    /// known.
    pub(crate) fn since_started(&self, at: Option<Timestamp>) -> Option<i64> {
        at.zip(self.started_at)
            .map(|(at, started)| (at - started).num_seconds())
    }
}

/// Where a run's transcript starts: before the first line of its first
/// session.
pub const START: ReadTo = ReadTo {
    session: 1,
    line: 0,
};

/// A change to a followed run's transcript, with where it was read: a change
/// of the session `read.session`.
#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub read: ReadTo,
    pub change: Change,
}

/// A run's sessions followed as their records grow, read into the model,
/// each change given as it is read. The changes are the same whatever pieces
/// the records are read in.
#[derive(Default)]
pub struct Feed {
    began: bool,
    sessions: Vec<Fed>,
}

/// What a place makes of a followed transcript as [`Feed`] reads it.
pub trait Layout {
    /// The run's transcript began, at [`START`], with the `sessions` sessions
    /// the run had then: once its first session's record came, or the run
    /// ended with none.
    fn began(&mut self, sessions: usize);

    /// `changed` was read, leaving its session's model as `session`.
    fn changed(&mut self, changed: Changed, session: &Session);
}

#[derive(Default)]
struct Fed {
    parser: Parser,
    bytes_read: usize,
    lines_read: usize,
    opened: bool,
}

impl Feed {
    /// Reads what `records`, a run's sessions in order, hold beyond what was
    /// read before, giving `layout` that the run's transcript began, then each
    /// change with its session's model as the line it was read from left it.
    /// A line still being written waits, unless the run has `ended` or a
    /// later session has started, which makes the session whole: it then
    /// ends, if its record has not said so, at a time not known.
    pub fn read(&mut self, records: &[SessionRecord], ended: bool, layout: &mut impl Layout) {
        if !self.began && (!records.is_empty() || ended) {
            self.began = true;
            layout.began(records.len());
        }
        let mut changed = |changed, session: &Session| layout.changed(changed, session);
        for (i, record) in records.iter().enumerate() {
            let session = i + 1;
            if i == self.sessions.len() {
                self.sessions.push(Fed::default());
            }
            let fed = &mut self.sessions[i];
            let whole = Written::of(session, records.len(), ended) == Written::Whole;
            let unread = &record.output[fed.bytes_read.min(record.output.len())..];
            let complete = if whole { unread } else { whole_lines(unread) };
            fed.parser.recorded_maintenance(record);
            for line in complete.lines() {
                fed.parser.read(line, record);
                fed.open(session, &mut changed);
                fed.lines_read += 1;
                fed.give(session, &mut changed);
            }
            fed.bytes_read += complete.len();
            fed.give(session, &mut changed);
            if whole {
                fed.parser.end(None);
                fed.give(session, &mut changed);
                fed.open(session, &mut changed);
            }
        }
    }

    /// The model of session `n`, from 1, as far as it has been read: `None`
    /// before it opened.
    pub fn session(&self, n: usize) -> Option<&Session> {
        let fed = self.sessions.get(n.checked_sub(1)?)?;
        fed.opened.then_some(&fed.parser.session)
    }
}

impl Fed {
    fn read_to(&self, session: usize) -> ReadTo {
        ReadTo {
            session,
            line: self.lines_read,
        }
    }

    /// Gives that it opened, unless it has.
    fn open(&mut self, session: usize, changed: &mut impl FnMut(Changed, &Session)) {
        if !self.opened {
            self.opened = true;
            let read = self.read_to(session);
            changed(
                Changed {
                    read,
                    change: Change::Opened,
                },
                &self.parser.session,
            );
        }
    }

    /// Gives the changes read since it last gave, opening it first.
    fn give(&mut self, session: usize, changed: &mut impl FnMut(Changed, &Session)) {
        let changes = std::mem::take(&mut self.parser.changed);
        if changes.is_empty() {
            return;
        }
        self.open(session, changed);
        let read = self.read_to(session);
        for change in changes {
            changed(Changed { read, change }, &self.parser.session);
        }
    }
}

pub fn whole_lines(text: &str) -> &str {
    &text[..text.rfind('\n').map_or(0, |end| end + 1)]
}

fn started_at(session: &Session) -> Option<Timestamp> {
    let clone = session.clone.as_ref().map(|clone| clone.at);
    clone
        .or_else(|| session.maintenance.as_ref().and_then(Maintenance::at))
        .or_else(|| {
            session.turns.first().and_then(|turn| {
                turn.started_at
                    .or_else(|| turn.entries.iter().find_map(|entry| entry.at))
            })
        })
}

/// The longest a tool's argument shows, in characters.
const ARGUMENT_CHARS: usize = 80;

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

/// A prompt's first line, with an ellipsis when it has more.
pub(crate) fn summary(prompt: &str) -> String {
    let prompt = prompt.trim();
    match prompt.split_once('\n') {
        Some((first, _)) => format!("{} …", first.trim_end()),
        None => prompt.to_string(),
    }
}

/// A maintenance command, as its line shows it.
pub(crate) fn shown_command(command: &str) -> String {
    one_line(command, ARGUMENT_CHARS)
}
