//! The run page (`docs/specs/design_system.md`, The run page): a run's
//! transcripts (Transcripts), in full with Verbose, each tool call folded to
//! its line, and followed while the run is in flight.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Write as _;

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::error;

use super::html::{self, Html};
use super::names::Names;
use super::runs_table::ItemCell;
use super::{
    ErrorPage, Frame, Page, Sent, frame_regions, render, run_path, samples, unanswered, with_query,
};
use crate::api::{self, EventId, Sources};
use crate::domain::Timestamp;
use crate::logs::{LogsError, RunRecords, Which};
use crate::phrase::Tone;
use crate::runs::{self, Ran, RunId};
use crate::transcript::{
    self, After, Call, Change, Changed, Clock, EntryAt, Feed, Layout, Made, Maintenance,
    MaintenanceEnded, MaintenanceRan, Numbering, Place, Prompt, Session, SessionClone, What,
};

/// A change to the page's transcript that a change to the run's makes.
enum Patch {
    /// `html` added at the end of the element `into`.
    Append { into: String, html: String },
    /// `html` in place of the element `id`.
    Replace { id: String, html: String },
}

impl Patch {
    /// As a partial that the page's script swaps in.
    fn partial(&self) -> String {
        match self {
            Patch::Append { into, html } => {
                format!(
                    r##"<hx-partial hx-target="#{into}" hx-swap="beforeend">{html}</hx-partial>"##
                )
            }
            Patch::Replace { id, html } => {
                format!(
                    r##"<hx-partial hx-target="#{id}" hx-swap="outerHTML">{html}</hx-partial>"##
                )
            }
        }
    }
}

/// A run's transcript as the page shows it, built up from its changes.
struct RunView {
    run: RunId,
    /// What the run is of, as the Item column of a list of runs names it
    /// (`docs/specs/design_system.md`, The run page, Heading).
    item: Html,
    verbose: bool,
    /// The number of sessions the run had when its transcript started, once
    /// it has.
    sessions_started: Option<usize>,
    /// Whether it ran by hand, once its transcript has started.
    by_hand: bool,
    sessions: Vec<SessionView>,
}

struct SessionView {
    number: usize,
    started_at: Option<Timestamp>,
    /// `None` for a session recorded before Linewright kept it.
    ran: Option<Ran>,
    /// In the order they were added: the `k`th's anchor is
    /// `s{number}-e{k}`.
    entries: Vec<EntryView>,
    /// The session's own entries, as against its sub-agents'.
    own: Vec<usize>,
    /// Its clone's entry, once it has been added.
    clone: Option<usize>,
    /// Its maintenance command's entry, once it has been added.
    maintenance: Option<usize>,
    /// The entries of its turns, by where each is.
    at: HashMap<EntryAt, usize>,
    clock: Clock,
    verbose: bool,
}

struct EntryView {
    stamp: StampView,
    of: Of,
    /// A sub-agent's entries, for the call that started it.
    children: Vec<usize>,
}

/// What of a session's transcript an entry shows.
enum Of {
    Clone,
    Maintenance,
    /// The brief or follow-up that started its `n`th turn.
    Prompt(usize),
    Entry(EntryAt),
}

/// What an entry shows, as its session's transcript holds it now.
enum Kind<'a> {
    Clone(&'a SessionClone),
    Maintenance(&'a Maintenance),
    Brief(&'a str),
    FollowUp(Option<&'a str>),
    Message(&'a str),
    Thinking(&'a str),
    Context(&'a str),
    Event(&'a Value),
    Other(&'a Value),
    Tool(&'a Call),
}

/// When an entry happened, as it was added.
#[derive(Clone, Copy)]
struct StampView {
    at: Option<Timestamp>,
    elapsed_seconds: Option<i64>,
}

impl RunView {
    fn new(run: RunId, item: Html, verbose: bool) -> RunView {
        RunView {
            run,
            item,
            verbose,
            sessions_started: None,
            by_hand: false,
            sessions: vec![],
        }
    }

    /// That the run's transcript started, with `sessions` sessions, giving
    /// what it changes on the page.
    fn start(&mut self, sessions: usize, by_hand: bool) -> Vec<Patch> {
        self.sessions_started = Some(sessions);
        self.by_hand = by_hand;
        vec![Patch::Replace {
            id: "run".into(),
            html: self.heading(),
        }]
    }

    /// Takes the next change to the transcript, of a session whose
    /// transcript the line it was read from left as `model`, and which ran
    /// with `ran`, giving what it changes on the page.
    fn apply(&mut self, changed: Changed, model: &Session, ran: Option<&Ran>) -> Vec<Patch> {
        let Changed { read, change } = changed;
        if change == Change::Opened {
            if read.session != self.sessions.len() + 1 {
                return vec![];
            }
            self.sessions.push(SessionView {
                number: read.session,
                started_at: model.started_at,
                ran: ran.cloned(),
                entries: vec![],
                own: vec![],
                clone: None,
                maintenance: None,
                at: HashMap::new(),
                clock: Clock::new(model.started_at),
                verbose: self.verbose,
            });
            let added = self.sessions.last().expect("just pushed");
            return vec![Patch::Append {
                into: "sessions".into(),
                html: added.html(model),
            }];
        }
        let Some(session) = read
            .session
            .checked_sub(1)
            .and_then(|i| self.sessions.get_mut(i))
        else {
            return vec![];
        };
        session.change(change, model)
    }

    fn heading(&self) -> String {
        let none = if self.sessions_started == Some(0) {
            " (it started no session)"
        } else {
            ""
        };
        let by_hand = if self.by_hand { ", by hand" } else { "" };
        format!(
            r#"<hgroup id="run"><h1>{}{by_hand}{none}</h1><p class="text-muted">{}</p></hgroup>"#,
            html::escape(&self.run.to_string()),
            self.item,
        )
    }

    /// The transcript as the page shows it whole, its sessions as `feed`
    /// has read them.
    fn html(&self, feed: &Feed) -> String {
        let sessions: String = self
            .sessions
            .iter()
            .map(|session| {
                let model = feed
                    .session(session.number)
                    .expect("a session shown has opened");
                session.html(model)
            })
            .collect();
        format!(r#"{}<div id="sessions">{sessions}</div>"#, self.heading())
    }
}

impl SessionView {
    /// The session as the page shows it, its transcript as `model` holds it.
    fn html(&self, model: &Session) -> String {
        let n = self.number;
        let heading = match self.started_at {
            Some(at) => format!("session {n}: started {}", html::time(at)),
            None => format!("session {n}"),
        };
        let drawn = self.drawn(model);
        let entries: String = self
            .own
            .iter()
            .filter(|&&i| drawn.shown(i))
            .map(|&i| drawn.entry_html(i))
            .collect();
        let ran = ran_html(self.ran.as_ref());
        format!(
            r#"<section class="session" id="s{n}"><h2 class="mt-4 mb-1 font-semibold">{heading}</h2>{ran}<ol class="entries font-mono text-sm" id="s{n}-entries">{entries}</ol></section>"#
        )
    }

    /// Takes the next change to its transcript, which the line it was read
    /// from left as `model`, giving what it changes on the page.
    fn change(&mut self, change: Change, model: &Session) -> Vec<Patch> {
        self.clock.started(model.started_at);
        match change {
            Change::Opened => vec![],
            Change::Turn(turn) => {
                let started = &model.turns[turn];
                if matches!(started.prompt, Prompt::Brief(None)) {
                    return vec![];
                }
                let stamp = self.stamp_tick(started.started_at);
                self.add(stamp, Of::Prompt(turn), None, model)
            }
            Change::Added(Place::Clone) => {
                let clone = model.clone.as_ref().expect("a changed clone is there");
                let stamp = self.stamp_tick(Some(clone.at));
                self.clone = Some(self.entries.len());
                self.add(stamp, Of::Clone, None, model)
            }
            Change::Added(Place::Maintenance) => {
                let maintenance = maintenance(model);
                let stamp = match maintenance.ran {
                    MaintenanceRan::Started { .. } => self.stamp_tick(maintenance.at()),
                    MaintenanceRan::Recorded(_) => StampView {
                        at: None,
                        elapsed_seconds: None,
                    },
                };
                self.maintenance = Some(self.entries.len());
                self.add(stamp, Of::Maintenance, None, model)
            }
            Change::Added(Place::Entry(at)) => {
                let stamp = self.stamp_tick(model.entry(&at).at);
                let parent = at.parent().and_then(|parent| self.at.get(&parent).copied());
                self.at.insert(at.clone(), self.entries.len());
                self.add(stamp, Of::Entry(at), parent, model)
            }
            Change::Ended(Place::Clone, at) => {
                let clone = model.clone.as_ref().expect("a changed clone is there");
                self.stamp_tick(Some(at.unwrap_or(clone.at)));
                self.again(self.clone, model)
            }
            Change::Ended(Place::Maintenance, at) => {
                if let MaintenanceRan::Started { .. } = maintenance(model).ran {
                    self.stamp_tick(at);
                }
                self.again(self.maintenance, model)
            }
            Change::Ended(Place::Entry(at), ended_at) => {
                self.stamp_tick(ended_at);
                let Some(&i) = self.at.get(&at) else {
                    return vec![];
                };
                let What::Tool(call) = &model.entry(&at).what else {
                    return vec![];
                };
                self.drawn(model).account_patches(i, call)
            }
        }
    }

    /// When something shown that happened `at` happened.
    fn stamp_tick(&mut self, at: Option<Timestamp>) -> StampView {
        let at = self.clock.tick(at);
        StampView {
            at,
            elapsed_seconds: self.clock.since_started(at),
        }
    }

    /// Its entries, drawn from its transcript as `model` holds it.
    fn drawn<'a>(&'a self, model: &'a Session) -> Drawn<'a> {
        Drawn {
            session: self,
            model,
            verbose: self.verbose,
        }
    }

    /// Adds an entry that shows `of`, under the call `parent` if it is of a
    /// sub-agent's work, giving what it changes on the page.
    fn add(
        &mut self,
        stamp: StampView,
        of: Of,
        parent: Option<usize>,
        model: &Session,
    ) -> Vec<Patch> {
        let i = self.entries.len();
        self.entries.push(EntryView {
            stamp,
            of,
            children: vec![],
        });
        match parent {
            Some(parent) => self.entries[parent].children.push(i),
            None => self.own.push(i),
        }
        let drawn = self.drawn(model);
        if !drawn.shown(i) {
            return vec![];
        }
        let Some(parent) = parent else {
            return vec![Patch::Append {
                into: format!("s{}-entries", self.number),
                html: drawn.entry_html(i),
            }];
        };
        let id = drawn.entry_id(parent);
        if drawn.shown_children(parent).count() == 1 {
            vec![
                Patch::Append {
                    into: format!("{id}-summary"),
                    html: drawn.count_html(parent),
                },
                Patch::Append {
                    into: format!("{id}-call"),
                    html: drawn.sub_agent_html(parent),
                },
            ]
        } else {
            vec![
                Patch::Append {
                    into: format!("{id}-entries"),
                    html: drawn.entry_html(i),
                },
                Patch::Replace {
                    id: format!("{id}-count"),
                    html: drawn.count_html(parent),
                },
            ]
        }
    }

    /// The patch that shows the entry `entry` that runs again once it has
    /// ended.
    fn again(&self, entry: Option<usize>, model: &Session) -> Vec<Patch> {
        let Some(i) = entry else {
            return vec![];
        };
        let drawn = self.drawn(model);
        vec![Patch::Replace {
            id: drawn.entry_id(i),
            html: drawn.entry_html(i),
        }]
    }
}

fn maintenance(model: &Session) -> &Maintenance {
    model
        .maintenance
        .as_ref()
        .expect("a changed maintenance command is there")
}

/// A session's model and effort, and where each of its settings and prompt
/// files came from.
fn ran_html(ran: Option<&Ran>) -> String {
    let said = format!(
        r#"<p class="ran text-sm text-muted">{}</p>"#,
        html::escape(&runs::describe_ran(ran))
    );
    let Some(ran) = ran else {
        return said;
    };
    let item = |class: &str, name: &str, whose: &str| {
        format!(
            r#"<li class="{class}">{}: {whose}</li>"#,
            html::escape(name)
        )
    };
    let settings: String = ran
        .layers
        .settings
        .iter()
        .map(|(setting, source)| item("setting", setting.name(), source.whose()))
        .collect();
    let prompts: String = ran
        .layers
        .prompts
        .iter()
        .map(|(path, layer)| item("prompt", path, layer.whose()))
        .collect();
    format!(
        r#"{said}<div class="layers text-sm text-muted"><ul class="ml-4">{settings}</ul><ul class="ml-4 font-mono">{prompts}</ul></div>"#
    )
}

/// A command's line, such as a maintenance command's, labeled `label`,
/// opening to its output once it has ended, open from the start when it
/// failed, and with `verbose`.
pub(super) fn command_html(
    stamp: &str,
    label: &str,
    command: &str,
    ended: Option<&MaintenanceEnded>,
    verbose: bool,
) -> String {
    let Some(ended) = ended else {
        let running = status_html(Tone::InProgress, "running");
        return command_line_html(stamp, label, command, &running);
    };
    let command = html::escape(&transcript::shown_command(command));
    let account = if ended.status.failed() {
        status_html(Tone::Failure, &ended.account())
    } else {
        html::escape(&ended.account()).to_string()
    };
    let open = if ended.output_shown(verbose) {
        " open"
    } else {
        ""
    };
    let output = output_html(&ended.output);
    format!(
        r#"<details{open}><summary class="cursor-pointer wrap-anywhere"><span class="line">{stamp} {label}: {command} → {account}</span></summary>{output}</details>"#
    )
}

/// The clone's line, opening to its error once it has ended with one, open
/// from the start (`docs/specs/design_system.md`, The run page, Folding).
fn clone_html(stamp: &str, ended: Option<&transcript::CloneEnded>) -> String {
    let Some(ended) = ended else {
        let running = status_html(Tone::InProgress, "running");
        return format!(r#"<div class="line">{stamp} clone → {running}</div>"#);
    };
    let Some(error) = &ended.error else {
        let account = html::escape(&ended.account());
        return format!(r#"<div class="line">{stamp} clone → {account}</div>"#);
    };
    let account = status_html(Tone::Failure, &ended.account());
    let error = output_html(error);
    format!(
        r#"<details open><summary class="cursor-pointer wrap-anywhere"><span class="line">{stamp} clone → {account}</span></summary>{error}</details>"#
    )
}

/// A command's line that does not open, `account` after its arrow.
pub(super) fn command_line_html(stamp: &str, label: &str, command: &str, account: &str) -> String {
    let command = html::escape(&transcript::shown_command(command));
    format!(r#"<div class="line">{stamp} {label}: {command} → {account}</div>"#)
}

/// A command's output under its line; nothing for an empty one.
pub(super) fn output_html(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }
    format!(
        r#"<pre class="output my-1 ml-[4.5em] max-h-[30rem] overflow-auto rounded-box bg-base-200 px-2.5 py-1.5 whitespace-pre-wrap wrap-anywhere">{}</pre>"#,
        html::escape(output.trim_end())
    )
}

/// A session's entries, drawn from its transcript as `model` holds it.
struct Drawn<'a> {
    session: &'a SessionView,
    model: &'a Session,
    verbose: bool,
}

impl Drawn<'_> {
    fn kind(&self, i: usize) -> Kind<'_> {
        match &self.session.entries[i].of {
            Of::Clone => Kind::Clone(self.model.clone.as_ref().expect("an added clone is there")),
            Of::Maintenance => Kind::Maintenance(maintenance(self.model)),
            Of::Prompt(turn) => match &self.model.turns[*turn].prompt {
                Prompt::Brief(text) => Kind::Brief(text.as_deref().unwrap_or_default()),
                Prompt::FollowUp(text) => Kind::FollowUp(text.as_deref()),
            },
            Of::Entry(at) => match &self.model.entry(at).what {
                What::Message(text) => Kind::Message(text),
                What::Thinking(text) => Kind::Thinking(text),
                What::Context(text) => Kind::Context(text),
                What::Tool(call) => Kind::Tool(call),
                What::Event(event) => Kind::Event(event),
                What::Other(other) => Kind::Other(other),
            },
        }
    }

    /// What a call's account changes: its line and its result, leaving its
    /// fold as the reader left it, unless it failed, which opens it.
    fn account_patches(&self, i: usize, call: &Call) -> Vec<Patch> {
        let id = self.entry_id(i);
        if call.failed() {
            return vec![Patch::Replace {
                id: format!("{id}-call"),
                html: self.call_html(i, call),
            }];
        }
        vec![
            Patch::Replace {
                id: format!("{id}-line"),
                html: self.call_line_html(i, call),
            },
            Patch::Replace {
                id: format!("{id}-io"),
                html: io_html(&id, call),
            },
        ]
    }

    /// Whether the page shows an entry: as a transcript shows it, or with
    /// `verbose` as a transcript in full does.
    fn shown(&self, i: usize) -> bool {
        match self.kind(i) {
            Kind::Message(_)
            | Kind::FollowUp(_)
            | Kind::Tool(_)
            | Kind::Clone(_)
            | Kind::Maintenance(_) => true,
            Kind::Brief(_)
            | Kind::Thinking(_)
            | Kind::Context(_)
            | Kind::Event(_)
            | Kind::Other(_) => self.verbose,
        }
    }

    fn shown_children(&self, call: usize) -> impl Iterator<Item = usize> + '_ {
        self.session.entries[call]
            .children
            .iter()
            .copied()
            .filter(move |&i| self.shown(i))
    }

    /// The entry's anchor: its number in the order entries were added to its
    /// session.
    fn entry_id(&self, i: usize) -> String {
        format!("s{}-e{}", self.session.number, i + 1)
    }

    fn entry_html(&self, i: usize) -> String {
        let id = self.entry_id(i);
        let stamp = stamp_html(&id, self.session.entries[i].stamp);
        let (kind, body) = match self.kind(i) {
            Kind::Tool(call) => ("tool", self.call_html(i, call)),
            Kind::Clone(clone) => ("clone", clone_html(&stamp, clone.ended.as_ref())),
            Kind::Maintenance(maintenance) => (
                "maintenance",
                command_html(
                    &stamp,
                    "maintenance",
                    &maintenance.command,
                    maintenance.ended(),
                    self.verbose,
                ),
            ),
            Kind::Message(text) => ("message", line_html(&stamp, "", text)),
            Kind::Thinking(text) => ("thinking", line_html(&stamp, "thinking: ", text)),
            Kind::Context(text) => ("context", line_html(&stamp, "context: ", text)),
            Kind::Brief(text) => ("brief", line_html(&stamp, "brief: ", text)),
            Kind::FollowUp(text) => {
                let text = match text {
                    Some(text) if self.verbose => text.to_string(),
                    _ => text.map_or(String::new(), transcript::summary),
                };
                ("follow-up", line_html(&stamp, "follow-up: ", &text))
            }
            Kind::Event(event) => ("event", line_html(&stamp, "event: ", &event.to_string())),
            Kind::Other(other) => ("other", line_html(&stamp, "other: ", &other.to_string())),
        };
        format!(r#"<li class="entry {kind} py-0.5" id="{id}">{body}</li>"#)
    }

    /// A call's line, opening to its full input and result and to the work
    /// of the sub-agent it started, with the count of its entries; open from
    /// the start when it failed, and with `verbose`.
    fn call_html(&self, i: usize, call: &Call) -> String {
        let id = self.entry_id(i);
        let sub_agent = self.shown_children(i).next().is_some();
        let (count, entries) = if sub_agent {
            (self.count_html(i), self.sub_agent_html(i))
        } else {
            (String::new(), String::new())
        };
        let open = if call.failed() || self.verbose {
            " open"
        } else {
            ""
        };
        format!(
            r#"<details class="call" id="{id}-call"{open}><summary class="cursor-pointer wrap-anywhere" id="{id}-summary">{line}{count}</summary>{io}{entries}</details>"#,
            line = self.call_line_html(i, call),
            io = io_html(&id, call),
        )
    }

    fn call_line_html(&self, i: usize, call: &Call) -> String {
        let id = self.entry_id(i);
        let stamp = stamp_html(&id, self.session.entries[i].stamp);
        let account = call.account();
        let account = if call.failed() {
            status_html(Tone::Failure, &account)
        } else if call.result.is_none() {
            status_html(Tone::InProgress, &account)
        } else {
            html::escape(&account).to_string()
        };
        format!(
            r#"<span class="line" id="{id}-line">{stamp} <b>{tool}</b>({argument}) → {account}</span>"#,
            tool = html::escape(call.tool.name()),
            argument = html::escape(&call.argument(self.model.dir.as_deref())),
        )
    }

    /// The work of the sub-agent a call started.
    fn sub_agent_html(&self, call: usize) -> String {
        let entries: String = self
            .shown_children(call)
            .map(|i| self.entry_html(i))
            .collect();
        format!(
            r#"<ol class="entries border-l-2 border-base-300 pl-6" id="{}-entries">{entries}</ol>"#,
            self.entry_id(call)
        )
    }

    fn count_html(&self, call: usize) -> String {
        let n = self.shown_children(call).count();
        let entries = if n == 1 { "entry" } else { "entries" };
        format!(
            r#" <span class="count badge badge-sm badge-ghost ml-2" id="{}-count">{n} {entries}</span>"#,
            self.entry_id(call)
        )
    }
}

/// The time since the session started, linking to the entry, with when it
/// happened in the page's tooltip.
fn stamp_html(id: &str, stamp: StampView) -> String {
    let elapsed = stamp
        .elapsed_seconds
        .map_or(String::new(), transcript::lines::elapsed);
    let tip = stamp.at.map_or(String::new(), |at| {
        format!(r#" data-tip="{}""#, html::utc(at))
    });
    format!(
        r##"<a class="at inline-block min-w-[4em] text-right text-muted" href="#{id}"{tip}>{elapsed}</a>"##
    )
}

fn line_html(stamp: &str, label: &str, text: &str) -> String {
    format!(
        r#"<div class="line">{stamp} {}<span class="text whitespace-pre-wrap wrap-anywhere">{}</span></div>"#,
        html::escape(label),
        html::escape(text.trim_end()),
    )
}

/// A call's full input and result, as a transcript in full shows them.
fn io_html(id: &str, call: &Call) -> String {
    let mut io = String::new();
    for (label, text) in call.parts() {
        let _ = writeln!(
            io,
            r#"<div>{label}: <pre class="mt-0.5 mb-1.5 max-h-[30rem] overflow-auto rounded-box bg-base-200 px-2.5 py-1.5 whitespace-pre-wrap wrap-anywhere">{}</pre></div>"#,
            html::escape(text.trim_end())
        );
    }
    format!(r#"<div class="io my-1 ml-[4.5em]" id="{id}-io">{io}</div>"#)
}

#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::status(tone, word) %}{% endcall %}"#,
    ext = "html"
)]
struct Status<'a> {
    tone: Tone,
    word: &'a str,
}

pub(super) fn status_html(tone: Tone, word: &str) -> String {
    render(&Status { tone, word })
}

/// Where the page's stream follows a run in flight from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
enum Follow {
    /// Its first line: the page showed none.
    Start,
    /// The line after the last the page showed.
    After(EventId),
}

const START: &str = "start";

impl TryFrom<String> for Follow {
    type Error = String;

    fn try_from(follow: String) -> Result<Follow, String> {
        match follow.as_str() {
            START => Ok(Follow::Start),
            id => id.parse().map(Follow::After),
        }
    }
}

impl std::fmt::Display for Follow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Follow::Start => f.write_str(START),
            Follow::After(id) => id.fmt(f),
        }
    }
}

#[derive(Deserialize)]
pub(super) struct RunQuery {
    /// Set for the page: the transcripts in full, everything open.
    verbose: Option<String>,
    /// For the page's stream, while the run is in flight.
    follow: Option<Follow>,
}

impl RunQuery {
    fn verbose(&self) -> bool {
        self.verbose.is_some()
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "run.html")]
struct RunPage {
    page: Page,
    frame: Frame,
    verbose: bool,
    /// The page with the switch the other way.
    switched: String,
    transcript: String,
}

/// Why a run cannot be shown, and the status the page answers with.
struct RunError {
    status: StatusCode,
    error: String,
}

impl From<LogsError> for RunError {
    fn from(e: LogsError) -> RunError {
        let (status, error) = unanswered(e);
        RunError { status, error }
    }
}

impl IntoResponse for RunError {
    fn into_response(self) -> Response {
        (self.status, self.error).into_response()
    }
}

fn run_id(id: &str) -> Result<RunId, RunError> {
    id.parse().map_err(|error| RunError {
        status: StatusCode::NOT_FOUND,
        error,
    })
}

/// A run's transcript followed for its page.
struct FollowedRun {
    records: RunRecords,
    page: PageFollower,
}

/// What the page makes of a run's records as they grow.
struct PageFollower {
    feed: Feed,
    view: RunView,
    numbering: Numbering,
}

impl FollowedRun {
    /// Following the run `id` from its start, giving what comes after the
    /// change made at `after`, if one is given.
    fn start(
        sources: &Sources,
        id: &RunId,
        verbose: bool,
        after: Option<Made>,
    ) -> Result<FollowedRun, LogsError> {
        let which = Which::Run {
            id: id.clone(),
            item: None,
        };
        Ok(FollowedRun {
            records: RunRecords::start(&sources.work_dir, &which)?,
            page: PageFollower {
                feed: Feed::default(),
                view: RunView::new(id.clone(), item_line(sources, id), verbose),
                numbering: Numbering::new(after.map(After::Line)),
            },
        })
    }

    /// One event per change the page has yet to show that changes it, once
    /// there are changes or the run has ended, each with its change's event
    /// ID, so that a reader resumes after the last it took; `None` once the
    /// run has ended and every change has been given.
    async fn next(&mut self) -> anyhow::Result<Option<Vec<Event>>> {
        let FollowedRun { records, page } = self;
        let made = records.next(|records| page.made(records)).await?;
        Ok(made.map(|made| events(records.run(), made)))
    }
}

impl PageFollower {
    /// What the page has yet to show of `records` as last read: each
    /// change's patches, with where it was made.
    fn made(&mut self, records: &RunRecords) -> Vec<(Made, Vec<Patch>)> {
        let mut layout = PageLayout {
            view: &mut self.view,
            numbering: &mut self.numbering,
            records,
            made: vec![],
        };
        self.feed
            .read(records.sessions(), records.ended(), &mut layout);
        layout.made
    }
}

/// The patches a [`PageFollower`] makes of one read.
struct PageLayout<'a> {
    view: &'a mut RunView,
    numbering: &'a mut Numbering,
    records: &'a RunRecords,
    made: Vec<(Made, Vec<Patch>)>,
}

impl Layout for PageLayout<'_> {
    fn began(&mut self, sessions: usize) {
        let patches = self.view.start(sessions, self.records.started().by_hand());
        if let Some(made) = self.numbering.next(transcript::START) {
            self.made.push((made, patches));
        }
    }

    fn changed(&mut self, changed: Changed, session: &Session) {
        let read = changed.read;
        let ran = self
            .records
            .sessions()
            .get(read.session - 1)
            .and_then(|record| record.ran.as_ref());
        let patches = self.view.apply(changed, session, ran);
        if let Some(made) = self.numbering.next(read) {
            self.made.push((made, patches));
        }
    }
}

/// An event of `run`'s for each change made that changes the page.
fn events(run: &RunId, made: Vec<(Made, Vec<Patch>)>) -> Vec<Event> {
    made.into_iter()
        .filter(|(_, patches)| !patches.is_empty())
        .map(|(made, patches)| {
            let id = EventId {
                run: run.clone(),
                made,
            };
            let data: String = patches.iter().map(Patch::partial).collect();
            Event::default().id(id.to_string()).data(data)
        })
        .collect()
}

/// What `id` is of, as the Item column of a list of runs names it, its item
/// as Linewright holds it now (`docs/specs/design_system.md`, The run page,
/// Heading).
fn item_line(sources: &Sources, id: &RunId) -> Html {
    let view = api::view(sources);
    let names = Names::of(
        &sources.work_dir.repo,
        view.as_ref().map(|view| &*view.snapshot),
    );
    ItemCell::of(&id.of, &names).html()
}

/// The transcript as its records hold it now, and while the run is in
/// flight where its page's stream follows it from.
fn read(sources: &Sources, id: RunId, verbose: bool) -> Result<(String, Option<Follow>), RunError> {
    let FollowedRun {
        mut records,
        mut page,
    } = FollowedRun::start(sources, &id, verbose, None)?;
    records.read().map_err(LogsError::Other)?;
    let made = page.made(&records);
    let follow = (!records.ended()).then(|| match made.last() {
        Some((last, _)) => Follow::After(EventId {
            run: id,
            made: *last,
        }),
        None => Follow::Start,
    });
    Ok((page.view.html(&page.feed), follow))
}

fn page_path(id: &RunId, prefix: &str, verbose: bool, follow: Option<&Follow>) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    if verbose {
        query.append_pair("verbose", "");
    }
    if let Some(follow) = follow {
        query.append_pair("follow", &follow.to_string());
    }
    with_query(&format!("{prefix}{}", run_path(id)), &query.finish())
}

pub(super) async fn run_page(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    Query(query): Query<RunQuery>,
) -> Response {
    let verbose = query.verbose();
    let title = format!("{id} · {}", sources.work_dir.repo);
    let frame = Frame::of(&sources);
    let shown = run_id(&id).and_then(|run| {
        let (transcript, follow) = read(&sources, run.clone(), verbose)?;
        Ok((run, transcript, follow))
    });
    match shown {
        Ok((run, transcript, follow)) => RunPage {
            page: Page::new(title, page_path(&run, "/events", verbose, follow.as_ref())),
            frame,
            verbose,
            switched: page_path(&run, "", !verbose, None),
            transcript,
        }
        .into_response(),
        Err(RunError { status, error }) => {
            let page = ErrorPage {
                page: Page::new(title, format!("/events/runs/{id}")),
                frame,
                region: "transcript",
                error,
            };
            (status, page).into_response()
        }
    }
}

const LAST_EVENT_ID: &str = "last-event-id";

/// The run page's regions: the frame's and, while the run is in flight, the
/// transcript's lines after those the page showed, or after the
/// `Last-Event-ID` a reconnecting reader gives.
pub(super) async fn run_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(id): Path<String>,
    Query(query): Query<RunQuery>,
    headers: HeaderMap,
) -> Response {
    let resumed = headers
        .get(LAST_EVENT_ID)
        .and_then(|last| last.to_str().ok())
        .and_then(|last| Follow::try_from(last.to_string()).ok());
    let follow = query
        .follow
        .as_ref()
        .map(|follow| resumed.unwrap_or_else(|| follow.clone()));
    let transcript = match (run_id(&id), follow) {
        (Ok(run), Some(follow)) => {
            let after = match follow {
                Follow::Start => None,
                Follow::After(last) if last.run == run => Some(last.made),
                Follow::After(last) => {
                    return RunError {
                        status: StatusCode::BAD_REQUEST,
                        error: format!("{last} is not an event of {run}"),
                    }
                    .into_response();
                }
            };
            match FollowedRun::start(&sources, &run, query.verbose(), after) {
                Ok(followed) => Some(followed),
                Err(e) => {
                    let e = RunError::from(e);
                    error!(run = %run, error = e.error, "following a transcript for its page failed");
                    return e.into_response();
                }
            }
        }
        _ => None,
    };
    let stream = RunStream {
        sent: Sent::default(),
        samples: samples(),
        transcript,
        sources,
        stopping,
    };
    let events = futures_util::stream::unfold(stream, |mut stream| async move {
        let events = stream.next().await?.into_iter().map(Ok::<_, Infallible>);
        Some((futures_util::stream::iter(events), stream))
    });
    crate::event_stream::respond(futures_util::StreamExt::flatten(events))
}

struct RunStream {
    sent: Sent,
    samples: tokio::time::Interval,
    /// `None` once the run has ended, or with the page not following it.
    transcript: Option<FollowedRun>,
    sources: Sources,
    stopping: CancellationToken,
}

/// What a stream woke for.
enum Wake {
    Transcript(anyhow::Result<Option<Vec<Event>>>),
    Sample,
}

impl RunStream {
    /// The next events to send, once there are some; `None` once `serve`
    /// stops.
    async fn next(&mut self) -> Option<Vec<Event>> {
        loop {
            let wake = tokio::select! {
                () = self.stopping.cancelled() => return None,
                events = next_events(self.transcript.as_mut()) => Wake::Transcript(events),
                _ = self.samples.tick() => Wake::Sample,
            };
            let events = match wake {
                Wake::Transcript(Ok(Some(events))) => events,
                Wake::Transcript(Ok(None)) => {
                    self.transcript = None;
                    vec![]
                }
                Wake::Transcript(Err(e)) => {
                    error!(error = format!("{e:#}"), "following a transcript failed");
                    self.transcript = None;
                    vec![]
                }
                Wake::Sample => {
                    let changed = self.sent.changed(frame_regions(&Frame::of(&self.sources)));
                    changed
                        .map(|message| Event::default().data(message))
                        .into_iter()
                        .collect()
                }
            };
            if !events.is_empty() {
                return Some(events);
            }
        }
    }
}

/// The next events of the transcript being followed; never, with none.
async fn next_events(transcript: Option<&mut FollowedRun>) -> anyhow::Result<Option<Vec<Event>>> {
    match transcript {
        Some(transcript) => transcript.next().await,
        None => std::future::pending().await,
    }
}
