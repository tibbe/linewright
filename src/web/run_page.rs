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
use crate::logs::{Following, LogsError, Which};
use crate::phrase::Tone;
use crate::runs::{self, Ran, RunId};
use crate::transcript::{
    self, EntryLine, FollowedLine, Header, Line, Made, Setup, SetupLine, Stamped, ToolLine, Under,
    Verbosity,
};

/// A change to the page's transcript that a line of it makes.
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

/// A run's transcript as the page shows it, built up from its lines as
/// following it makes them, so that the page read whole and the page
/// followed in pieces are one and the same.
struct RunView {
    run: RunId,
    /// What the run is of, as the Item column of a list of runs names it
    /// (`docs/specs/design_system.md`, The run page, Heading).
    item: Html,
    verbose: bool,
    /// The number of sessions its header gave, once it has come.
    sessions_started: Option<usize>,
    /// Whether its header said it ran by hand.
    by_hand: bool,
    sessions: Vec<SessionView>,
}

struct SessionView {
    number: usize,
    started_at: Option<Timestamp>,
    /// `None` for a session recorded before Linewright kept it.
    ran: Option<Ran>,
    /// Its setup command, once its line has come.
    setup: Option<Setup>,
    /// In the order they first appeared, so that each keeps its anchor
    /// however the transcript is read: the `k`th is `s{number}-e{k}`.
    entries: Vec<EntryView>,
    /// The session's own entries, as against its sub-agents'.
    own: Vec<usize>,
    /// Each call's entry, by the ID the agent gives it.
    calls: HashMap<String, usize>,
}

struct EntryView {
    stamp: StampView,
    what: EntryLine,
    /// A sub-agent's entries, for the call that started it.
    children: Vec<usize>,
}

/// When an entry happened, kept from its first line: a call's later line
/// gives its account, not its time, as a transcript shows it.
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

    /// Takes the next line of the transcript, giving what it changes on the
    /// page, from `following`, which made it.
    fn apply_line(&mut self, line: &Line, following: &Following) -> Vec<Patch> {
        match line {
            Line::Header(Header::Run { run, sessions }) => {
                self.sessions_started = Some(*sessions);
                self.by_hand = run.by_hand();
                vec![Patch::Replace {
                    id: "run".into(),
                    html: self.heading(),
                }]
            }
            Line::Header(Header::Session {
                session,
                started_at,
            }) => {
                if *session != self.sessions.len() + 1 {
                    return vec![];
                }
                self.sessions.push(SessionView {
                    number: *session,
                    started_at: *started_at,
                    ran: following.ran(*session).cloned(),
                    setup: None,
                    entries: vec![],
                    own: vec![],
                    calls: HashMap::new(),
                });
                let added = self.sessions.last().expect("just pushed");
                vec![Patch::Append {
                    into: "sessions".into(),
                    html: self.session_html(added),
                }]
            }
            Line::Setup(line) => self.setup(line),
            Line::Entry(stamped) => self.entry(stamped),
        }
    }

    fn setup(&mut self, line: &SetupLine) -> Vec<Patch> {
        let Some(session) = line
            .session
            .checked_sub(1)
            .and_then(|i| self.sessions.get_mut(i))
        else {
            return vec![];
        };
        session.setup = Some(line.setup.clone());
        vec![Patch::Replace {
            id: format!("s{}-setup", session.number),
            html: setup_html(session, self.verbose),
        }]
    }

    fn entry(&mut self, stamped: &Stamped) -> Vec<Patch> {
        let verbose = self.verbose;
        let Some(session) = stamped
            .stamp
            .session
            .checked_sub(1)
            .and_then(|i| self.sessions.get_mut(i))
        else {
            return vec![];
        };
        if let EntryLine::Tool(call) = &stamped.what
            && let Some(&i) = session.calls.get(&call.tool_use_id)
        {
            session.entries[i].what = stamped.what.clone();
            return account_patches(session, i, call, verbose);
        }
        let i = session.entries.len();
        session.entries.push(EntryView {
            stamp: StampView {
                at: stamped.stamp.at,
                elapsed_seconds: stamped.stamp.elapsed_seconds,
            },
            what: stamped.what.clone(),
            children: vec![],
        });
        if let EntryLine::Tool(call) = &stamped.what {
            session.calls.insert(call.tool_use_id.clone(), i);
        }
        let parent = match &stamped.stamp.under {
            Under::Session => None,
            Under::SubAgent { parent, .. } => session.calls.get(parent).copied(),
        };
        match parent {
            Some(parent) => session.entries[parent].children.push(i),
            None => session.own.push(i),
        }
        let session = &*session;
        if !shown(&session.entries[i].what, verbose) {
            return vec![];
        }
        let Some(parent) = parent else {
            return vec![Patch::Append {
                into: format!("s{}-entries", session.number),
                html: entry_html(session, i, verbose),
            }];
        };
        let id = entry_id(session, parent);
        if shown_children(session, parent, verbose).count() == 1 {
            vec![
                Patch::Append {
                    into: format!("{id}-summary"),
                    html: count_html(session, parent, verbose),
                },
                Patch::Append {
                    into: format!("{id}-call"),
                    html: sub_agent_html(session, parent, verbose),
                },
            ]
        } else {
            vec![
                Patch::Append {
                    into: format!("{id}-entries"),
                    html: entry_html(session, i, verbose),
                },
                Patch::Replace {
                    id: format!("{id}-count"),
                    html: count_html(session, parent, verbose),
                },
            ]
        }
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

    fn session_html(&self, session: &SessionView) -> String {
        let n = session.number;
        let heading = match session.started_at {
            Some(at) => format!("session {n}: started {}", html::time(at)),
            None => format!("session {n}"),
        };
        let entries: String = session
            .own
            .iter()
            .filter(|&&i| shown(&session.entries[i].what, self.verbose))
            .map(|&i| entry_html(session, i, self.verbose))
            .collect();
        let ran = ran_html(session.ran.as_ref());
        let setup = setup_html(session, self.verbose);
        format!(
            r#"<section class="session" id="s{n}"><h2 class="mt-4 mb-1 font-semibold">{heading}</h2>{ran}{setup}<ol class="entries font-mono text-sm" id="s{n}-entries">{entries}</ol></section>"#
        )
    }

    /// The transcript as the page shows it whole.
    fn html(&self) -> String {
        let sessions: String = self
            .sessions
            .iter()
            .map(|session| self.session_html(session))
            .collect();
        format!(r#"{}<div id="sessions">{sessions}</div>"#, self.heading())
    }
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

/// The session's setup command, in a section of its own: its line, opening
/// to its output, open from the start when it failed, and with `verbose`;
/// empty until its line has come.
fn setup_html(session: &SessionView, verbose: bool) -> String {
    let n = session.number;
    let Some(setup) = &session.setup else {
        return format!(r#"<div class="setup" id="s{n}-setup"></div>"#);
    };
    let Setup { ran, output } = setup;
    let account = if ran.status.failed() {
        status_html(Tone::Failure, &ran.account())
    } else {
        html::escape(&ran.account()).to_string()
    };
    let open = if setup.output_shown(verbose) {
        " open"
    } else {
        ""
    };
    let output = if output.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<pre class="output my-1 ml-[4.5em] max-h-[30rem] overflow-auto rounded-box bg-base-200 px-2.5 py-1.5 whitespace-pre-wrap wrap-anywhere">{}</pre>"#,
            html::escape(output.trim_end())
        )
    };
    format!(
        r#"<div class="setup font-mono text-sm" id="s{n}-setup"><details{open}><summary class="cursor-pointer wrap-anywhere"><span class="line"><span class="inline-block min-w-[4em]"></span> setup: {command} → {account}</span></summary>{output}</details></div>"#,
        command = html::escape(&ran.shown_command()),
    )
}

/// What a call's account changes: its line and its result, leaving its fold
/// as the reader left it, unless it failed, which opens it.
fn account_patches(session: &SessionView, i: usize, call: &ToolLine, verbose: bool) -> Vec<Patch> {
    let id = entry_id(session, i);
    if call.error {
        return vec![Patch::Replace {
            id: format!("{id}-call"),
            html: call_html(session, i, call, verbose),
        }];
    }
    vec![
        Patch::Replace {
            id: format!("{id}-line"),
            html: call_line_html(session, i, call),
        },
        Patch::Replace {
            id: format!("{id}-io"),
            html: io_html(&id, call),
        },
    ]
}

/// Whether the page shows an entry: as a transcript shows it, or with
/// `verbose` as a transcript in full does.
fn shown(what: &EntryLine, verbose: bool) -> bool {
    match what {
        EntryLine::Message { .. } | EntryLine::FollowUp { .. } | EntryLine::Tool(_) => true,
        EntryLine::Brief { .. }
        | EntryLine::Thinking { .. }
        | EntryLine::Context { .. }
        | EntryLine::Event { .. }
        | EntryLine::Other { .. } => verbose,
    }
}

fn shown_children(
    session: &SessionView,
    call: usize,
    verbose: bool,
) -> impl Iterator<Item = usize> + '_ {
    session.entries[call]
        .children
        .iter()
        .copied()
        .filter(move |&i| shown(&session.entries[i].what, verbose))
}

/// The entry's anchor: its number in the order entries first appeared in
/// its session.
fn entry_id(session: &SessionView, i: usize) -> String {
    format!("s{}-e{}", session.number, i + 1)
}

fn entry_html(session: &SessionView, i: usize, verbose: bool) -> String {
    let entry = &session.entries[i];
    let id = entry_id(session, i);
    let stamp = stamp_html(&id, entry.stamp);
    let (kind, body) = match &entry.what {
        EntryLine::Tool(call) => ("tool", call_html(session, i, call, verbose)),
        EntryLine::Message { text } => ("message", line_html(&stamp, "", text)),
        EntryLine::Thinking { text } => ("thinking", line_html(&stamp, "thinking: ", text)),
        EntryLine::Context { text } => ("context", line_html(&stamp, "context: ", text)),
        EntryLine::Brief { text } => ("brief", line_html(&stamp, "brief: ", text)),
        EntryLine::FollowUp { summary, text } => {
            let text = text.as_ref().filter(|_| verbose).unwrap_or(summary);
            ("follow-up", line_html(&stamp, "follow-up: ", text))
        }
        EntryLine::Event { event } => ("event", line_html(&stamp, "event: ", &event.to_string())),
        EntryLine::Other { other } => ("other", line_html(&stamp, "other: ", &other.to_string())),
    };
    format!(r#"<li class="entry {kind} py-0.5" id="{id}">{body}</li>"#)
}

/// The time since the session started, linking to the entry, with when it
/// happened in the page's tooltip.
fn stamp_html(id: &str, stamp: StampView) -> String {
    let elapsed = stamp
        .elapsed_seconds
        .map_or(String::new(), transcript::elapsed);
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

/// A call's line, opening to its full input and result and to the work of
/// the sub-agent it started, with the count of its entries; open from the
/// start when it failed, and with `verbose`.
fn call_html(session: &SessionView, i: usize, call: &ToolLine, verbose: bool) -> String {
    let id = entry_id(session, i);
    let sub_agent = shown_children(session, i, verbose).next().is_some();
    let (count, entries) = if sub_agent {
        (
            count_html(session, i, verbose),
            sub_agent_html(session, i, verbose),
        )
    } else {
        (String::new(), String::new())
    };
    let open = if call.error || verbose { " open" } else { "" };
    format!(
        r#"<details class="call" id="{id}-call"{open}><summary class="cursor-pointer wrap-anywhere" id="{id}-summary">{line}{count}</summary>{io}{entries}</details>"#,
        line = call_line_html(session, i, call),
        io = io_html(&id, call),
    )
}

fn call_line_html(session: &SessionView, i: usize, call: &ToolLine) -> String {
    let id = entry_id(session, i);
    let stamp = stamp_html(&id, session.entries[i].stamp);
    let account = if call.error {
        status_html(Tone::Failure, &call.account)
    } else if call.result.is_none() {
        // Read at full verbosity, a call has a result once it returns.
        status_html(Tone::InProgress, &call.account)
    } else {
        html::escape(&call.account).to_string()
    };
    format!(
        r#"<span class="line" id="{id}-line">{stamp} <b>{tool}</b>({argument}) → {account}</span>"#,
        tool = html::escape(&call.tool),
        argument = html::escape(&call.argument),
    )
}

/// A call's full input and result, as a transcript in full shows them.
fn io_html(id: &str, call: &ToolLine) -> String {
    let pretty = |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default();
    let mut io = String::new();
    let mut part = |label: &str, text: &str| {
        let _ = writeln!(
            io,
            r#"<div>{label}: <pre class="mt-0.5 mb-1.5 max-h-[30rem] overflow-auto rounded-box bg-base-200 px-2.5 py-1.5 whitespace-pre-wrap wrap-anywhere">{}</pre></div>"#,
            html::escape(text.trim_end())
        );
    };
    if let Some(input) = &call.input {
        part("input", &pretty(input));
    }
    if let Some(result) = &call.result {
        part("result", result);
    }
    if let Some(content) = &call.content {
        let content: Vec<String> = content.iter().map(Value::to_string).collect();
        part("content", &content.join("\n"));
    }
    if let Some(details) = &call.details {
        part("details", &pretty(details));
    }
    format!(r#"<div class="io my-1 ml-[4.5em]" id="{id}-io">{io}</div>"#)
}

/// The work of the sub-agent a call started.
fn sub_agent_html(session: &SessionView, call: usize, verbose: bool) -> String {
    let entries: String = shown_children(session, call, verbose)
        .map(|i| entry_html(session, i, verbose))
        .collect();
    format!(
        r#"<ol class="entries border-l-2 border-base-300 pl-6" id="{}-entries">{entries}</ol>"#,
        entry_id(session, call)
    )
}

fn count_html(session: &SessionView, call: usize, verbose: bool) -> String {
    let n = shown_children(session, call, verbose).count();
    let entries = if n == 1 { "entry" } else { "entries" };
    format!(
        r#" <span class="count badge badge-sm badge-ghost ml-2" id="{}-count">{n} {entries}</span>"#,
        entry_id(session, call)
    )
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

fn status_html(tone: Tone, word: &str) -> String {
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

/// Following the run `id` from its first line, at full verbosity, which
/// gives every call's input and result to fold.
fn following(sources: &Sources, id: RunId) -> Result<Following, LogsError> {
    let which = Which::Run { id, item: None };
    Following::start(&sources.work_dir, &which, Verbosity::Full, None)
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
    let mut following = following(sources, id.clone())?;
    let lines = following.read().map_err(LogsError::Other)?;
    let mut view = RunView::new(id.clone(), item_line(sources, &id), verbose);
    for FollowedLine { line, .. } in &lines {
        view.apply_line(line, &following);
    }
    let follow = (!following.ended()).then(|| match lines.last() {
        Some(last) => Follow::After(EventId {
            run: id,
            made: last.made,
        }),
        None => Follow::Start,
    });
    Ok((view.html(), follow))
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

/// A run's transcript being followed for its page.
struct FollowedTranscript {
    following: Following,
    view: RunView,
    /// The last line the page has shown, if any.
    after: Option<Made>,
}

impl FollowedTranscript {
    /// One event per line the page has yet to show, with that line's event
    /// ID, so that a reader resumes after the last it took.
    fn events(&mut self, lines: &[FollowedLine]) -> Vec<Event> {
        let mut events = vec![];
        for FollowedLine { made, line } in lines {
            let patches = self.view.apply_line(line, &self.following);
            if self.after.is_some_and(|after| *made <= after) || patches.is_empty() {
                continue;
            }
            let id = EventId {
                run: self.following.run().clone(),
                made: *made,
            };
            let data: String = patches.iter().map(Patch::partial).collect();
            events.push(Event::default().id(id.to_string()).data(data));
        }
        events
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
            match following(&sources, run.clone()) {
                Ok(following) => Some(FollowedTranscript {
                    following,
                    view: RunView::new(run.clone(), item_line(&sources, &run), query.verbose()),
                    after,
                }),
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
    transcript: Option<FollowedTranscript>,
    sources: Sources,
    stopping: CancellationToken,
}

/// What a stream woke for.
enum Wake {
    Lines(anyhow::Result<Option<Vec<FollowedLine>>>),
    Sample,
}

impl RunStream {
    /// The next events to send, once there are some; `None` once `serve`
    /// stops.
    async fn next(&mut self) -> Option<Vec<Event>> {
        loop {
            let wake = tokio::select! {
                () = self.stopping.cancelled() => return None,
                lines = next_lines(self.transcript.as_mut()) => Wake::Lines(lines),
                _ = self.samples.tick() => Wake::Sample,
            };
            let events = match wake {
                Wake::Lines(Ok(Some(lines))) => self
                    .transcript
                    .as_mut()
                    .map(|transcript| transcript.events(&lines))
                    .unwrap_or_default(),
                Wake::Lines(Ok(None)) => {
                    self.transcript = None;
                    vec![]
                }
                Wake::Lines(Err(e)) => {
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

/// The next lines of the transcript being followed; never, with none.
async fn next_lines(
    transcript: Option<&mut FollowedTranscript>,
) -> anyhow::Result<Option<Vec<FollowedLine>>> {
    match transcript {
        Some(transcript) => transcript.following.next().await,
        None => std::future::pending().await,
    }
}
