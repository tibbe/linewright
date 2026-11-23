//! The run page (`docs/specs/runs.md`, Run): a run's
//! transcripts (Transcripts), as the agent's own default or verbose view
//! shows them, each entry on a line that opens to what it leaves out, and
//! followed while the run is in flight.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Write as _;

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use tracing::error;

use super::buttons::{ScreenButton, Verb};
use super::caller::Reader;
use super::capture::{self, Offer};
use super::html::{self, Html};
use super::icons::{Icon, IconSize};
use super::names::Names;
use super::runs_table::ItemCell;
use super::shell::{self, Frame, Heading, Missing, Navigation, Thing};
use super::{
    ErrorPage, Page, Sent, nothing_there, render, run_path, samples, unanswered, with_query,
};
use crate::access::{Permit, Read};
use crate::actions;
use crate::actions::Action;
use crate::api::{EventId, Sources};
use crate::domain::Timestamp;
use crate::logs::{KeptRuns, LogsError, RunRecords, Which};
use crate::phrase::Status;
use crate::runs::{self, Ran, Recorded, RunId};
use crate::transcript::lines::{THOUGHT, WORKED_FOR, cut_short};
use crate::transcript::{
    self, After, Call, Change, Changed, CloneEnded, Course, End, Entry, EntryAt, Feed, INTERRUPTED,
    Lasted, Layout, Made, Maintenance, MaintenanceEnded, Numbering, Place, Prompt, RUNNING,
    Session, Thought, What, whole_seconds,
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
    verbose: bool,
    sessions: Vec<SessionView>,
}

struct SessionView {
    number: usize,
    started_at: Option<Timestamp>,
    /// `None` for a session recorded before Linewright kept it.
    ran: Option<Ran>,
    /// In the order they were added: the `k`th's ID is `s{number}-e{k}`.
    entries: Vec<EntryView>,
    /// The session's own entries, as against its sub-agents'.
    own: Vec<usize>,
    /// Its clone's entry, once it has been added.
    clone: Option<usize>,
    /// Its maintenance command's entry, once it has been added.
    maintenance: Option<usize>,
    /// The entries of its turns, by where each is.
    at: HashMap<EntryAt, usize>,
    verbose: bool,
    /// Its working step as the page shows it: the list that holds it, empty
    /// of any step while it shows none.
    working: String,
}

struct EntryView {
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
    /// How long its `n`th turn took, once it has ended.
    WorkedFor(usize),
}

impl Of {
    /// Where the entry is, for one that may run.
    fn place(&self) -> Option<Place> {
        match self {
            Of::Clone => Some(Place::Clone),
            Of::Maintenance => Some(Place::Maintenance),
            Of::Prompt(_) | Of::WorkedFor(_) => None,
            Of::Entry(at) => Some(Place::Entry(at.clone())),
        }
    }
}

/// What an entry the page shows is, as its session's transcript holds it
/// now: what the agent's own views show, and never what it was told
/// besides its prompts, events or blocks Linewright does not know
/// (`docs/specs/runs.md`, Run, Views).
enum Kind<'a> {
    Clone(Course<&'a CloneEnded>),
    Maintenance(&'a Maintenance),
    Brief(&'a str),
    FollowUp(Option<&'a str>),
    Message(&'a str),
    Thought(&'a Thought),
    Tool(&'a Call),
    /// The end of its `n`th turn.
    WorkedFor(usize),
}

impl RunView {
    fn new(verbose: bool) -> RunView {
        RunView {
            verbose,
            sessions: vec![],
        }
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
                verbose: self.verbose,
                working: working_html(read.session, model),
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
        format!(r#"<div id="sessions">{sessions}</div>"#)
    }
}

impl SessionView {
    /// The session as the page shows it, its transcript as `model` holds it.
    fn html(&self, model: &Session) -> String {
        let n = self.number;
        let heading = self.heading_html(model);
        let working = working_html(n, model);
        let drawn = self.drawn(model);
        let entries: String = self
            .own
            .iter()
            .filter_map(|&i| drawn.entry_html(i))
            .collect();
        let settings = settings_html(self.ran.as_ref(), self.verbose);
        format!(
            r#"<section class="session" id="s{n}">{heading}{settings}<ol class="entries font-mono text-sm" id="s{n}-entries">{entries}</ol>{working}</section>"#
        )
    }

    /// Its number and when it started, with how long it has run at the
    /// right (`docs/specs/runs.md`, Run, A session's
    /// heading).
    fn heading_html(&self, model: &Session) -> String {
        let n = self.number;
        let Some(started_at) = self.started_at else {
            return format!(
                r#"<h2 class="mt-4 mb-1 in-[.session:first-child]:mt-0 font-semibold" id="s{n}-heading"><span class="text">session {n}</span></h2>"#
            );
        };
        let lasted = duration_html(model.lasted_so_far().map(Lasted::from));
        format!(
            r#"<h2 class="mt-4 mb-1 in-[.session:first-child]:mt-0 flex items-baseline font-semibold" id="s{n}-heading"><span class="text grow">session {n}: started {}</span>{lasted}</h2>"#,
            html::time(started_at),
        )
    }

    /// Takes the next change to its transcript, which the line it was read
    /// from left as `model`, giving what it changes on the page: the
    /// entries it changes, then its heading and its working step where
    /// those change.
    fn change(&mut self, change: Change, model: &Session) -> Vec<Patch> {
        let heading_changes = matches!(
            change,
            Change::WaitBegan(_) | Change::WaitEnded(_) | Change::SessionEnded
        );
        let mut patches = self.entries_change(change, model);
        if heading_changes {
            patches.push(Patch::Replace {
                id: format!("s{}-heading", self.number),
                html: self.heading_html(model),
            });
        }
        let working = working_html(self.number, model);
        if working != self.working {
            patches.push(Patch::Replace {
                id: format!("s{}-working", self.number),
                html: working.clone(),
            });
            self.working = working;
        }
        patches
    }

    /// What a change to its transcript, which the line it was read from left
    /// as `model`, changes of its entries.
    fn entries_change(&mut self, change: Change, model: &Session) -> Vec<Patch> {
        match change {
            Change::Opened | Change::WaitBegan(_) | Change::WaitEnded(_) | Change::SessionEnded => {
                vec![]
            }
            Change::Turn(turn) => {
                if matches!(model.turns[turn].prompt, Prompt::Brief(None)) {
                    return vec![];
                }
                self.add(Of::Prompt(turn), None, model)
            }
            Change::TurnEnded(turn) => self.add(Of::WorkedFor(turn), None, model),
            Change::Added(Place::Clone) => {
                self.clone = Some(self.entries.len());
                self.add(Of::Clone, None, model)
            }
            Change::Added(Place::Maintenance) => {
                self.maintenance = Some(self.entries.len());
                self.add(Of::Maintenance, None, model)
            }
            Change::Added(Place::Entry(at)) => {
                let parent = at.parent().and_then(|parent| self.at.get(&parent).copied());
                self.at.insert(at.clone(), self.entries.len());
                self.add(Of::Entry(at), parent, model)
            }
            Change::Ended(Place::Clone, _) => self.again(self.clone, model),
            Change::Ended(Place::Maintenance, _) => self.again(self.maintenance, model),
            Change::Ended(Place::Entry(at), _) => {
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
    fn add(&mut self, of: Of, parent: Option<usize>, model: &Session) -> Vec<Patch> {
        let i = self.entries.len();
        self.entries.push(EntryView {
            of,
            children: vec![],
        });
        match parent {
            Some(parent) => self.entries[parent].children.push(i),
            None => self.own.push(i),
        }
        let drawn = self.drawn(model);
        let Some(html) = drawn.entry_html(i) else {
            return vec![];
        };
        let Some(parent) = parent else {
            return vec![Patch::Append {
                into: format!("s{}-entries", self.number),
                html,
            }];
        };
        let id = drawn.entry_id(parent);
        let Kind::Tool(call) = drawn.kind(parent).expect("a call is shown") else {
            unreachable!("a sub-agent's work is under a call")
        };
        let line = Patch::Replace {
            id: format!("{id}-line"),
            html: drawn.call_line_html(parent, call),
        };
        if drawn.shown_children(parent).count() == 1 {
            vec![
                line,
                Patch::Append {
                    into: format!("{id}-body"),
                    html: drawn.sub_agent_html(parent),
                },
            ]
        } else {
            vec![
                Patch::Append {
                    into: format!("{id}-entries"),
                    html,
                },
                line,
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
        drawn
            .entry_html(i)
            .map(|html| Patch::Replace {
                id: drawn.entry_id(i),
                html,
            })
            .into_iter()
            .collect()
    }
}

/// The session `n`'s working step, as `model` holds its transcript: while it
/// goes on with no entry running, while it waits for the usage limit, a
/// step that says so and until when, or else while a turn goes on, one that
/// says "working" and how long the turn has gone on so far
/// (`docs/specs/runs.md`, Run, Working).
fn working_html(n: usize, model: &Session) -> String {
    let step = (model.end == End::Going && !runs_an_entry(model))
        .then(|| {
            let (text, lasted) = match model.waiting() {
                Some(wait) => (
                    format!(
                        "waiting for the usage limit, until {}",
                        html::time(wait.until)
                    ),
                    model
                        .last_at()
                        .or(model.started_at)
                        .map(|from| Lasted::from(model.lasted_from(from))),
                ),
                None => (WORKING.to_string(), Some(model.turn_so_far()?.into())),
            };
            let row = entry_row(&Row {
                id: Some(&format!("s{n}-step")),
                text,
                lasted,
                fold: None,
            });
            Some(format!(
                r#"<li class="entry working py-px" id="s{n}-step">{row}</li>"#
            ))
        })
        .flatten();
    format!(
        r#"<ol class="entries font-mono text-sm" id="s{n}-working">{}</ol>"#,
        step.unwrap_or_default()
    )
}

/// Whether any of a session's entries that run runs still.
fn runs_an_entry(model: &Session) -> bool {
    fn running(entries: &[Entry]) -> bool {
        entries.iter().any(|entry| match &entry.what {
            What::Tool(call) => call.end == Course::Running || running(&call.entries),
            What::Message(_)
            | What::Thought(_)
            | What::Context(_)
            | What::Event(_)
            | What::Other(_) => false,
        })
    }
    let clone = model
        .clone
        .as_ref()
        .is_some_and(|clone| clone.course == Course::Running);
    let maintenance = model
        .maintenance
        .as_ref()
        .is_some_and(|maintenance| matches!(maintenance.course(), Course::Running));
    clone || maintenance || model.turns.iter().any(|turn| running(&turn.entries))
}

fn maintenance(model: &Session) -> &Maintenance {
    model
        .maintenance
        .as_ref()
        .expect("a changed maintenance command is there")
}

/// A session's model and effort, opening to where each of its settings and
/// brief files came from; open from the start with `verbose`
/// (`docs/specs/runs.md`, Run, A session's settings).
fn settings_html(ran: Option<&Ran>, verbose: bool) -> String {
    let said = format!(
        r#"<span class="ran text-base-content/60">{}</span>"#,
        html::escape(&runs::describe_ran(ran))
    );
    let fold = ran.map(|ran| {
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
        let briefs: String = ran
            .layers
            .briefs
            .iter()
            .map(|(path, layer)| item("brief", path, layer.whose()))
            .collect();
        Fold {
            open: verbose,
            body: format!(
                r#"<div class="layers text-base-content/60"><ul>{settings}</ul><ul class="font-mono">{briefs}</ul></div>"#
            ),
        }
    });
    let row = entry_row(&Row {
        id: None,
        text: said,
        lasted: None,
        fold,
    });
    format!(r#"<div class="settings mb-1 text-sm">{row}</div>"#)
}

/// An entry's line, and what it opens to, if anything
/// (`docs/specs/web/components.md`, Transcript).
pub(super) struct Row<'a> {
    /// The start of the IDs of its line, `{id}-line`, of what holds it and
    /// what it opens to, `{id}-fold`, and of what it opens to,
    /// `{id}-body`.
    pub(super) id: Option<&'a str>,
    /// Its text, as markup.
    pub(super) text: String,
    /// How long it ran, for an entry that runs.
    pub(super) lasted: Option<Lasted>,
    pub(super) fold: Option<Fold>,
}

/// What an entry's line opens to.
pub(super) struct Fold {
    /// Whether it starts open.
    pub(super) open: bool,
    /// As markup.
    pub(super) body: String,
}

/// `row` as markup: its line starts with a column for a chevron, which a
/// row that opens fills, then its text, then its duration at the right
/// edge. What it opens to starts where its text does.
pub(super) fn entry_row(row: &Row) -> String {
    let Some(fold) = &row.fold else {
        return line_html(row);
    };
    let id = |suffix: &str| {
        row.id
            .map_or(String::new(), |id| format!(r#" id="{id}-{suffix}""#))
    };
    let open = if fold.open { " open" } else { "" };
    format!(
        r#"<details{fold_id}{open}>{line}<div class="ml-6"{body_id}>{body}</div></details>"#,
        fold_id = id("fold"),
        line = line_html(row),
        body_id = id("body"),
        body = fold.body,
    )
}

/// A row's line: the summary of its fold, if it opens, in
/// `bg-base-content/10` on hover and on keyboard focus
/// (`docs/specs/web/components.md`, Transcript, Hover).
/// The chevron of a fold points right while it is closed and down while it
/// is open (`docs/specs/web/components.md`, Transcript, Lines).
fn line_html(row: &Row) -> String {
    let (tag, chevron) = if row.fold.is_some() {
        (
            "summary",
            Icon::ChevronRight
                .svg(IconSize::Inline)
                .then(&Icon::ChevronDown.svg(IconSize::Inline)),
        )
    } else {
        ("div", Html::default())
    };
    let id = row
        .id
        .map_or(String::new(), |id| format!(r#" id="{id}-line""#));
    format!(
        r#"<{tag} class="line grid grid-cols-[1.5rem_minmax(0,1fr)_auto] items-baseline rounded-sm hover:bg-base-content/10 focus-visible:bg-base-content/10"{id}><span class="chevron flex h-lh items-center justify-center self-start text-base-content/60" aria-hidden="true">{chevron}</span><span class="text whitespace-pre-wrap wrap-anywhere">{text}</span>{duration}</{tag}>"#,
        text = row.text,
        duration = duration_html(row.lasted),
    )
}

/// The class of a block an entry opens to
/// (`docs/specs/web/components.md`, Transcript, What an entry opens to).
const OPENS_TO: &str = "max-h-[30rem] overflow-auto rounded-box bg-base-200 px-3 py-2 whitespace-pre-wrap wrap-anywhere";

/// The text of the working step while a turn goes on
/// (`docs/specs/web/components.md`, Transcript, "working").
const WORKING: &str = r#"<span class="text-base-content/60"><span class="loading loading-dots loading-xs mr-1 align-middle"></span>working</span>"#;

/// The class of a duration at the right of its line.
const DURATION: &str =
    "duration pl-4 text-right whitespace-nowrap text-base-content/60 font-normal";

/// How long an entry that runs ran, ticking while it runs, and nothing
/// where that is not known (`docs/specs/runs.md`, Run,
/// Durations).
fn duration_html(lasted: Option<Lasted>) -> String {
    let Some(lasted) = lasted else {
        return String::new();
    };
    let (seconds, since) = match lasted {
        Lasted::Running { since: Some(since) } => (0, Some(since)),
        Lasted::Took {
            seconds: Some(seconds),
        }
        | Lasted::Interrupted {
            seconds: Some(seconds),
        } => (whole_seconds(seconds), None),
        Lasted::Running { since: None }
        | Lasted::Took { seconds: None }
        | Lasted::Interrupted { seconds: None } => return String::new(),
    };
    html::duration(Some(DURATION), seconds, since).to_string()
}

/// A command's text on its line, such as a maintenance command's, labeled
/// `label`, with `account`, as markup, after its arrow.
pub(super) fn command_text(label: &str, command: &str, account: &str) -> String {
    format!(
        "{label}: {} → {account}",
        html::escape(&transcript::shown_command(command))
    )
}

/// A command's row, such as a maintenance command's, labeled `label`, as
/// far as it ran by `course`, and how long it `lasted`. It opens to its
/// output, open from the start when it failed, and with `verbose`.
pub(super) fn command_row(
    id: Option<&str>,
    label: &str,
    command: &str,
    course: Course<&MaintenanceEnded>,
    lasted: Option<Lasted>,
    verbose: bool,
) -> String {
    let fold = course.ended().and_then(|ended| {
        let output = output_html(&ended.output);
        (!output.is_empty()).then(|| Fold {
            open: ended.output_shown(verbose),
            body: output,
        })
    });
    let account = account_html(course.map(|ended| Said {
        said: ended.account().said,
        failed: ended.status.failed(),
    }));
    entry_row(&Row {
        id,
        text: command_text(label, command, &account),
        lasted,
        fold,
    })
}

/// How an entry that runs went, as its line says after its arrow.
struct Said {
    said: String,
    failed: bool,
}

/// The account of an entry that runs as far as `course` says, as markup:
/// `running`, `interrupted`, and how it ended when it failed, each as its
/// status (`docs/specs/runs.md`, Transcripts, Running).
fn account_html(course: Course<Said>) -> String {
    match course {
        Course::Running => status_html(Status::InFlight, RUNNING),
        Course::Interrupted => status_html(Status::Cancelled, INTERRUPTED),
        Course::Ended(Said { said, failed: true }) => status_html(Status::Failed, &said),
        Course::Ended(Said {
            said,
            failed: false,
        }) => html::escape(&said).to_string(),
    }
}

/// A command's output where its row opens to it; nothing for an empty one.
pub(super) fn output_html(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }
    format!(
        r#"<pre class="output my-1 {OPENS_TO}">{}</pre>"#,
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
    /// What the entry `i` is, if the page shows it: as the agent's default
    /// view does, or with `verbose` as its verbose view does.
    fn kind(&self, i: usize) -> Option<Kind<'_>> {
        let kind = match &self.session.entries[i].of {
            Of::Clone => Kind::Clone(
                self.model
                    .clone
                    .as_ref()
                    .expect("an added clone is there")
                    .course
                    .as_ref(),
            ),
            Of::Maintenance => Kind::Maintenance(maintenance(self.model)),
            Of::Prompt(turn) => match &self.model.turns[*turn].prompt {
                Prompt::Brief(text) => Kind::Brief(text.as_deref().unwrap_or_default()),
                Prompt::FollowUp(text) => Kind::FollowUp(text.as_deref()),
            },
            Of::Entry(at) => match &self.model.entry(at).what {
                What::Message(text) => Kind::Message(text),
                What::Thought(thought) => Kind::Thought(thought),
                What::Tool(call) => Kind::Tool(call),
                What::Context(_) | What::Event(_) | What::Other(_) => {
                    return None;
                }
            },
            Of::WorkedFor(turn) => Kind::WorkedFor(*turn),
        };
        Some(kind)
    }

    /// How long the entry `i` ran, for one that runs, a thought or a turn's
    /// end.
    fn lasted(&self, i: usize) -> Option<Lasted> {
        match &self.session.entries[i].of {
            Of::WorkedFor(turn) => self.model.worked_for(*turn),
            of @ (Of::Clone | Of::Maintenance | Of::Prompt(_) | Of::Entry(_)) => {
                of.place().and_then(|place| self.model.lasted(&place))
            }
        }
    }

    /// What a call's account changes: its line and its result, leaving its
    /// fold as the reader left it, unless it failed, which opens it.
    fn account_patches(&self, i: usize, call: &Call) -> Vec<Patch> {
        let id = self.entry_id(i);
        if call.failed() {
            return vec![Patch::Replace {
                id: format!("{id}-fold"),
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

    fn shown_children(&self, call: usize) -> impl Iterator<Item = usize> + '_ {
        self.session.entries[call]
            .children
            .iter()
            .copied()
            .filter(move |&i| self.kind(i).is_some())
    }

    /// The entry's ID: its number in the order entries were added to its
    /// session.
    fn entry_id(&self, i: usize) -> String {
        format!("s{}-e{}", self.session.number, i + 1)
    }

    /// The entry `i`, if the page shows it.
    fn entry_html(&self, i: usize) -> Option<String> {
        let id = self.entry_id(i);
        let row = |text: String, fold: Option<Fold>| {
            entry_row(&Row {
                id: Some(&id),
                text,
                lasted: self.lasted(i),
                fold,
            })
        };
        let (kind, body) = match self.kind(i)? {
            Kind::Tool(call) => ("tool", self.call_html(i, call)),
            Kind::Clone(course) => ("clone", row(clone_text(course), clone_fold(course))),
            Kind::Maintenance(maintenance) => (
                "maintenance",
                command_row(
                    Some(&id),
                    "maintenance",
                    &maintenance.command,
                    maintenance.course(),
                    self.lasted(i),
                    self.verbose,
                ),
            ),
            Kind::Message(text) => (
                "message",
                row(html::escape(text.trim_end()).to_string(), None),
            ),
            Kind::Thought(thought) => {
                let text = thought.text().trim();
                let fold = (!text.is_empty()).then(|| Fold {
                    open: self.verbose,
                    body: format!(
                        r#"<div class="thinking my-1 {OPENS_TO}">{}</div>"#,
                        html::escape(text)
                    ),
                });
                (
                    "thought text-base-content/60",
                    row(cut_short(THOUGHT, thought.cut_short()), fold),
                )
            }
            Kind::WorkedFor(turn) => {
                let interrupted = self.model.turns[turn].end == Course::Interrupted;
                ("worked-for", row(cut_short(WORKED_FOR, interrupted), None))
            }
            Kind::Brief(text) => ("brief", self.prompt_row(&id, "brief", text)),
            Kind::FollowUp(text) => (
                "follow-up",
                self.prompt_row(&id, "follow-up", text.unwrap_or_default()),
            ),
        };
        Some(format!(
            r#"<li class="entry {kind} py-px" id="{id}">{body}</li>"#
        ))
    }

    /// The brief or a follow-up, labeled `label`: its first line, opening
    /// to its full `text` where that holds more, and open with `verbose`
    /// (`docs/specs/runs.md`, Run, Prompts).
    fn prompt_row(&self, id: &str, label: &str, text: &str) -> String {
        let text = text.trim();
        let (first, more) = match text.split_once('\n') {
            Some((first, _)) => (first.trim_end(), true),
            None => (text, false),
        };
        let fold = more.then(|| Fold {
            open: self.verbose,
            body: format!(
                r#"<div class="prompt my-1 {OPENS_TO}">{}</div>"#,
                html::escape(text)
            ),
        });
        entry_row(&Row {
            id: Some(id),
            text: format!("{label}: {}", html::escape(first)),
            lasted: None,
            fold,
        })
    }

    /// A call's row, opening to its full input and result and to the work
    /// of the sub-agent it started; open from the start when it failed, and
    /// with `verbose`.
    fn call_html(&self, i: usize, call: &Call) -> String {
        let id = self.entry_id(i);
        entry_row(&self.call_row(&id, i, call))
    }

    /// A call's line, as the summary of its row.
    fn call_line_html(&self, i: usize, call: &Call) -> String {
        let id = self.entry_id(i);
        line_html(&self.call_row(&id, i, call))
    }

    /// The row of the call `i`, whose ID is `id`.
    fn call_row<'a>(&self, id: &'a str, i: usize, call: &Call) -> Row<'a> {
        let entries = if self.shown_children(i).next().is_some() {
            self.sub_agent_html(i)
        } else {
            String::new()
        };
        Row {
            id: Some(id),
            text: self.call_text(i, call),
            lasted: self.lasted(i),
            fold: Some(Fold {
                open: call.failed() || self.verbose,
                body: format!("{}{entries}", io_html(id, call)),
            }),
        }
    }

    /// A call's text on its line: the tool, its main argument and its
    /// account, leaving out how long it ran, with the count of its
    /// sub-agent's entries.
    fn call_text(&self, i: usize, call: &Call) -> String {
        let course = match (call.end, &call.result) {
            (Course::Interrupted, _) => Course::Interrupted,
            (_, None) => Course::Running,
            (_, Some(_)) => Course::Ended(Said {
                said: call.account().said,
                failed: call.failed(),
            }),
        };
        let account = account_html(course);
        format!(
            "<b>{tool}</b>({argument}) → {account}{count}",
            tool = html::escape(call.tool.name()),
            argument = html::escape(&call.argument(self.model.dir.as_deref())),
            count = self.count_html(i),
        )
    }

    /// The work of the sub-agent a call started.
    fn sub_agent_html(&self, call: usize) -> String {
        let entries: String = self
            .shown_children(call)
            .filter_map(|i| self.entry_html(i))
            .collect();
        format!(
            r#"<ol class="entries border-s-2 border-base-content/10" id="{}-entries">{entries}</ol>"#,
            self.entry_id(call)
        )
    }

    /// The count of the entries of the sub-agent a call started, if any.
    fn count_html(&self, call: usize) -> String {
        let n = self.shown_children(call).count();
        if n == 0 {
            return String::new();
        }
        let entries = if n == 1 { "entry" } else { "entries" };
        format!(r#" <span class="count badge badge-sm ml-2">{n} {entries}</span>"#)
    }
}

/// The clone's text on its line: how far it ran, then `done` or `failed`.
fn clone_text(course: Course<&CloneEnded>) -> String {
    let account = account_html(course.map(|ended| {
        let failed = ended.error.is_some();
        Said {
            said: if failed { "failed" } else { "done" }.to_string(),
            failed,
        }
    }));
    format!("clone → {account}")
}

/// What the clone opens to: its error, once it has ended with one, open
/// from the start (`docs/specs/runs.md`, Run, Folding).
fn clone_fold(course: Course<&CloneEnded>) -> Option<Fold> {
    let error = course.ended()?.error.as_ref()?;
    Some(Fold {
        open: true,
        body: output_html(error),
    })
}

/// A call's full input and result, as a transcript in full shows them.
fn io_html(id: &str, call: &Call) -> String {
    let mut io = String::new();
    for (label, text) in call.parts() {
        let _ = writeln!(
            io,
            r#"<div>{label}: <pre class="mt-0.5 mb-1.5 {OPENS_TO}">{}</pre></div>"#,
            html::escape(text.trim_end())
        );
    }
    format!(r#"<div class="io my-1" id="{id}-io">{io}</div>"#)
}

#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::status(status, word) %}{% endcall %}"#,
    ext = "html"
)]
struct Badge<'a> {
    status: Status,
    word: &'a str,
}

pub(super) fn status_html(status: Status, word: &str) -> String {
    render(&Badge { status, word })
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
    /// For the page's stream: set where the page draws the capture's dialog.
    capture: Option<String>,
}

impl RunQuery {
    pub(super) fn verbose(&self) -> bool {
        self.verbose.is_some()
    }

    /// Whether the stream's page draws the capture's dialog.
    fn captures(&self) -> bool {
        self.capture.is_some()
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
    /// The dialog of the capture, which "Add task" opens, empty for a run
    /// that cannot be captured.
    capture: Html,
    transcript: String,
}

/// A run whose transcript a page shows: where it is kept and which it is.
#[derive(Clone)]
pub(super) struct Transcribed {
    pub(super) kept: KeptRuns,
    pub(super) which: Which,
    /// What its page names as missing where its run does not exist, if it
    /// is that page's own run.
    pub(super) missing: Option<Missing>,
}

impl Transcribed {
    /// Run `id` of `serve`'s runs.
    fn run(permit: &Permit<Read>, sources: &Sources, id: &RunId) -> Transcribed {
        Transcribed {
            kept: actions::run_transcripts(permit, sources),
            which: Which::Run {
                id: id.clone(),
                item: None,
            },
            missing: Some(Missing::Run(id.to_string())),
        }
    }
}

/// Why a run cannot be shown.
enum RunError {
    /// There is no such run, as the error says.
    NoSuchRun(String),
    /// Any other reason, and the status the page answers with.
    Other { status: StatusCode, error: String },
}

impl RunError {
    /// The status the page answers with, and what it says.
    fn answer(self) -> (StatusCode, String) {
        match self {
            RunError::NoSuchRun(error) => (StatusCode::NOT_FOUND, error),
            RunError::Other { status, error } => (status, error),
        }
    }
}

impl From<LogsError> for RunError {
    fn from(e: LogsError) -> RunError {
        match e {
            LogsError::NoSuchRun(no_such) => RunError::NoSuchRun(no_such.to_string()),
            e @ LogsError::Other(_) => {
                let (status, error) = unanswered(e);
                RunError::Other { status, error }
            }
        }
    }
}

impl IntoResponse for RunError {
    fn into_response(self) -> Response {
        self.answer().into_response()
    }
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
    /// Following the run `transcribed` names from its start, giving what
    /// comes after the change made at `after`, if one is given.
    async fn start(
        transcribed: &Transcribed,
        verbose: bool,
        after: Option<Made>,
    ) -> Result<FollowedRun, LogsError> {
        let records = RunRecords::start(transcribed.kept.clone(), &transcribed.which).await?;
        let view = RunView::new(verbose);
        Ok(FollowedRun {
            records,
            page: PageFollower {
                feed: Feed::default(),
                view,
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
    fn began(&mut self, _sessions: usize) {
        if let Some(made) = self.numbering.next(transcript::START) {
            self.made.push((made, vec![]));
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

/// Run `recorded`'s heading (`docs/specs/runs.md`, Run, Heading): its ID,
/// then ", by hand" for an automation's run by hand, and "(it started no
/// session)" for a run that ended without a session, with its item under
/// them.
fn run_heading(permit: &Permit<Read>, sources: &Sources, recorded: &Recorded) -> Heading {
    let id = recorded.id.to_string();
    let by_hand = if recorded.started().by_hand() {
        ", by hand"
    } else {
        ""
    };
    let none = if recorded.state != runs::State::InFlight && recorded.sessions.is_empty() {
        " (it started no session)"
    } else {
        ""
    };
    Heading {
        name: html::escape(&format!("{id}{by_hand}{none}")),
        title: id,
        state: None,
        line: Some(item_line(permit, sources, &recorded.id)),
    }
}

/// What run `run`'s page shows of it: its heading, or that there is no
/// such run; `None` where its record does not read.
async fn run_thing(permit: &Permit<Read>, sources: &Sources, run: &RunId) -> Option<Thing> {
    match actions::get_run(permit, sources, run).await {
        Ok(Some(recorded)) => Some(Thing::Found(run_heading(permit, sources, &recorded))),
        Ok(None) => Some(Thing::Missing(Missing::Run(run.to_string()))),
        Err(_) => None,
    }
}

/// The page at `place`, its stream at `events`, with "Add task" among the
/// screen's buttons where it draws the capture's dialog
/// (`docs/specs/runs.md`, Run, Capturing).
fn transcript_page_at(
    place: shell::Place,
    events: String,
    captures: bool,
    navigation: Navigation,
) -> Page {
    let buttons = captures
        .then(|| ScreenButton::Act {
            verb: Verb::AddTask,
            dialog: capture::DIALOG.into(),
            action: Action::AddTask,
        })
        .into_iter()
        .collect();
    Page::new(place, events, navigation).buttons(buttons)
}

/// What `id` is of, as the Item column of a list of runs names it, its item
/// as Linewright holds it now (`docs/specs/runs.md`, Run,
/// Heading).
fn item_line(permit: &Permit<Read>, sources: &Sources, id: &RunId) -> Html {
    let snapshot = actions::view_snapshot(permit, sources);
    let names = Names::of(&sources.work_dir.repo, snapshot.as_deref());
    ItemCell::of(&id.of, &names).html()
}

/// The transcript as its records hold it now, and while the run is in
/// flight where its page's stream follows it from.
async fn read(
    transcribed: &Transcribed,
    verbose: bool,
) -> Result<(String, Option<Follow>), RunError> {
    let FollowedRun {
        mut records,
        mut page,
    } = FollowedRun::start(transcribed, verbose, None).await?;
    records.read().await.map_err(LogsError::Other)?;
    let made = page.made(&records);
    let follow = (!records.ended()).then(|| match made.last() {
        Some((last, _)) => Follow::After(EventId {
            run: records.run().clone(),
            made: *last,
        }),
        None => Follow::Start,
    });
    Ok((page.view.html(&page.feed), follow))
}

/// The page at `path`, its stream's with `prefix`, in the view `verbose`
/// says, following from `follow` if given, and for a stream, with
/// `captures` whether the page draws the capture's dialog.
fn page_path(
    path: &str,
    prefix: &str,
    verbose: bool,
    follow: Option<&Follow>,
    captures: bool,
) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    if verbose {
        query.append_pair("verbose", "");
    }
    if let Some(follow) = follow {
        query.append_pair("follow", &follow.to_string());
    }
    if captures {
        query.append_pair("capture", "");
    }
    with_query(&format!("{prefix}{path}"), &query.finish())
}

/// The page of the transcript `transcribed` names, at `path` and `place`,
/// in the shell's `frame`, with `capture` the dialog its "Add task" opens,
/// if it offers one; or, where it cannot be shown, why, standing in for it.
pub(super) async fn transcript_page(
    frame: Frame,
    path: &str,
    place: shell::Place,
    transcribed: Result<Transcribed, (StatusCode, String)>,
    verbose: bool,
    capture: Option<Html>,
    navigation: Navigation,
) -> Response {
    let shown = match transcribed {
        Ok(transcribed) => match (read(&transcribed, verbose).await, transcribed.missing) {
            (Err(RunError::NoSuchRun(_)), Some(missing)) => {
                let events = format!("/events{path}");
                return nothing_there(
                    |thing| Page::new(run_place(thing), events, navigation),
                    missing,
                    frame,
                    "transcript",
                );
            }
            (shown, _) => shown,
        },
        Err((status, error)) => Err(RunError::Other { status, error }),
    };
    match shown {
        Ok((transcript, follow)) => {
            let captures = capture.is_some();
            let events = page_path(path, "/events", verbose, follow.as_ref(), captures);
            RunPage {
                page: transcript_page_at(place, events, captures, navigation),
                frame,
                verbose,
                switched: page_path(path, "", !verbose, None, false),
                capture: capture.unwrap_or_default(),
                transcript,
            }
            .into_response()
        }
        Err(e) => {
            let (status, error) = e.answer();
            let page = ErrorPage {
                page: Page::new(place, format!("/events{path}"), navigation),
                frame,
                region: "transcript",
                error,
            };
            (status, page).into_response()
        }
    }
}

/// The place of a run's screen, or of a transcript's: alone, in Runs.
fn run_place(thing: Thing) -> shell::Place {
    shell::Place::Alone(shell::Area::Runs, thing)
}

pub(super) async fn run_page(
    Reader { permit, sources }: Reader,
    Path(id): Path<String>,
    Query(query): Query<RunQuery>,
    navigation: Navigation,
) -> Response {
    let events = format!("/events{}", super::run_path(&id));
    let Ok(run) = id.parse::<RunId>() else {
        let frame = Frame::of(&permit, &sources).await;
        let page = |thing| Page::new(run_place(thing), events, navigation);
        return nothing_there(page, Missing::Run(id), frame, "transcript");
    };
    let thing = match run_thing(&permit, &sources, &run).await {
        Some(Thing::Missing(missing)) => {
            let frame = Frame::of(&permit, &sources).await;
            let page = |thing| Page::new(run_place(thing), events, navigation);
            return nothing_there(page, missing, frame, "transcript");
        }
        Some(found) => found,
        None => Thing::Found(Heading::named(id)),
    };
    let capture = Offer::of(&permit, &sources, &run).await.map(Offer::html);
    transcript_page(
        Frame::of(&permit, &sources).await,
        &run_path(&run),
        run_place(thing),
        Ok(Transcribed::run(&permit, &sources, &run)),
        query.verbose(),
        capture,
        navigation,
    )
    .await
}

const LAST_EVENT_ID: &str = "last-event-id";

/// The run page's regions: the shell's, with "Add task" where the page
/// draws the capture's dialog, and while the run is in flight, the
/// transcript's lines after those the page showed, or after the
/// `Last-Event-ID` a reconnecting reader gives. A run missing as the stream
/// starts stays so, as its page shows it.
pub(super) async fn run_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Path(id): Path<String>,
    Query(query): Query<RunQuery>,
    headers: HeaderMap,
    navigation: Navigation,
) -> Response {
    let run = id.parse::<RunId>().ok();
    let transcribed = run
        .as_ref()
        .map(|run| Transcribed::run(&permit, &sources, run));
    let found = match &run {
        Some(run) => !matches!(
            run_thing(&permit, &sources, run).await,
            Some(Thing::Missing(_))
        ),
        None => false,
    };
    let captures = query.captures();
    let page: PageOf = {
        let (permit, sources) = (permit.clone(), sources.clone());
        Box::new(move || {
            let (permit, sources) = (permit.clone(), sources.clone());
            let (run, id) = (run.clone(), id.clone());
            let events = format!("/events{}", super::run_path(&id));
            Box::pin(async move {
                let thing = match &run {
                    Some(run) if found => run_thing(&permit, &sources, run).await?,
                    _ => Thing::Missing(Missing::Run(id)),
                };
                Some(transcript_page_at(
                    run_place(thing),
                    events,
                    captures,
                    navigation,
                ))
            })
        })
    };
    transcript_events(
        permit,
        sources,
        watch,
        transcribed.as_ref(),
        &query,
        &headers,
        page,
    )
    .await
}

/// The page of a transcript's stream, made again each time the stream
/// looks for what changed; `None` where what it shows did not read, which
/// leaves the shell as the stream last sent it.
pub(super) type PageOf = Box<dyn Fn() -> BoxFuture<'static, Option<Page>> + Send + Sync>;

/// The regions of the page of the transcript `transcribed` names: the
/// shell's of the page `page` makes, and while the run is in flight the
/// transcript's lines after those the page showed, or after the
/// `Last-Event-ID` a reconnecting reader gives.
pub(super) async fn transcript_events(
    permit: Permit<Read>,
    sources: Sources,
    watch: super::caller::Watch,
    transcribed: Option<&Transcribed>,
    query: &RunQuery,
    headers: &HeaderMap,
    page: PageOf,
) -> Response {
    let resumed = headers
        .get(LAST_EVENT_ID)
        .and_then(|last| last.to_str().ok())
        .and_then(|last| Follow::try_from(last.to_string()).ok());
    let follow = query
        .follow
        .as_ref()
        .map(|follow| resumed.unwrap_or_else(|| follow.clone()));
    let transcript = match (transcribed, follow) {
        (Some(transcribed), Some(follow)) => {
            let after = match &follow {
                Follow::Start => None,
                Follow::After(last) => Some(last.made),
            };
            match FollowedRun::start(transcribed, query.verbose(), after).await {
                Ok(followed) => match follow {
                    Follow::After(last) if &last.run != followed.records.run() => {
                        return RunError::Other {
                            status: StatusCode::BAD_REQUEST,
                            error: format!("{last} is not an event of {}", followed.records.run()),
                        }
                        .into_response();
                    }
                    Follow::Start | Follow::After(_) => Some(followed),
                },
                Err(e) => {
                    let (status, error) = RunError::from(e).answer();
                    error!(error, "following a transcript for its page failed");
                    return (status, error).into_response();
                }
            }
        }
        _ => None,
    };
    let stream = RunStream {
        sent: Sent::default(),
        samples: samples(),
        transcript,
        page,
        permit,
        sources,
        watch,
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
    page: PageOf,
    permit: Permit<Read>,
    sources: Sources,
    watch: super::caller::Watch,
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
                () = self.watch.stopping().cancelled() => return None,
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
                    if !self.watch.signs_someone_in().await {
                        return None;
                    }
                    let frame = Frame::of(&self.permit, &self.sources).await;
                    let regions = match (self.page)().await {
                        Some(page) => shell::regions(&frame, &page),
                        None => vec![shell::holds(&frame)],
                    };
                    let changed = self.sent.changed(regions);
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
