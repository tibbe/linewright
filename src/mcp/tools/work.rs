//! The tools that show the work (`docs/specs/access.md`, The MCP server,
//! What a tool shows): each returns what its screen shows, as the model
//! its page renders.

use std::num::NonZeroUsize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Context, Tool, refused};
use crate::access::{Permit, Read};
use crate::actions::{
    self, Action, AutomationScreen, BoardScreen, EntryScreen, ItemScreen, Missing, RunHeading,
    ServerScreen, SessionScreen,
};
use crate::api::Unexplained;
use crate::config;
use crate::domain::{Number, Timestamp};
use crate::mcp::page::{Earlier, Paging, missing};
use crate::reconcile::WorkKind;
use crate::runs::{self, Run, RunId};
use crate::status::UnparsedAutomation;
use crate::transcript::lines::Verbosity;

/// See the board (`docs/specs/board.md`, Board).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetBoard {
    /// The login of the person to narrow the board to
    /// (`docs/specs/board.md`, Filtering by person).
    person: Option<String>,
}

impl Tool for GetBoard {
    const ACTION: Action = Action::GetBoard;
    type Level = Read;
    type Output = BoardScreen;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<BoardScreen, String> {
        let person = self.person.as_deref().and_then(actions::chosen_person);
        Ok(actions::get_board(permit, context.sources, person.as_ref()).await)
    }
}

/// See an item (`docs/specs/board.md`, Item).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetItem {
    /// The issue's or pull request's number.
    number: u64,
}

impl Tool for GetItem {
    const ACTION: Action = Action::GetItem;
    type Level = Read;
    type Output = ItemScreen;

    async fn run(self, permit: &Permit<Read>, context: &Context<'_>) -> Result<ItemScreen, String> {
        match actions::get_item(permit, context.sources, Number(self.number)).await {
            Ok((item, _)) => Ok(item),
            Err(Unexplained::NoSuchItem(number)) => Err(Missing::Item(number).to_string()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// See the runs, newest first, narrowed by the filters of Runs
/// (`docs/specs/runs.md`, Runs). It holds the fields of [`Paging`] rather
/// than flattening it, since serde reads no unknown field of a struct that
/// flattens another as refused
/// (<https://serde.rs/container-attrs.html#deny_unknown_fields>).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct ListRuns {
    /// The issue or pull request whose runs to list.
    item: Option<u64>,
    /// The stage whose runs to list.
    stage: Option<WorkKind>,
    /// Keeps the runs that started at or after a time
    /// (2026-10-14T12:00:00Z), a date (2026-10-14) or a duration ago (2h).
    since: Option<String>,
    /// How many to give at most: 100 without it.
    limit: Option<NonZeroUsize>,
    /// The `after` of the list that gave the ones before: gives those after
    /// it.
    after: Option<String>,
}

/// The runs, as [`ListRuns`] gives them: a page of them, by the names
/// `runs --json` gives them.
#[derive(Serialize, JsonSchema)]
pub(super) struct RunsPage {
    runs: Vec<Run>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

impl Tool for ListRuns {
    const ACTION: Action = Action::ListRuns;
    type Level = Read;
    type Output = RunsPage;

    async fn run(self, permit: &Permit<Read>, context: &Context<'_>) -> Result<RunsPage, String> {
        let now = context.sources.clock.now();
        let since = self
            .since
            .as_deref()
            .map(|since| config::parse_since(since.trim()).map_err(|e| refused("since", &e)))
            .transpose()?;
        let query = runs::Query {
            item: self.item.map(Number),
            stage: self.stage,
            since: since.map(|since| since.at(now)),
            limit: None,
        };
        let listed = actions::list_runs(permit, context.sources, &query, now)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let after = match &self.after {
            Some(after) => {
                let after = run_after(permit, context, after).await?;
                Some(newest_first(after.started_at, &after.id))
            }
            None => None,
        };
        let paging = Paging {
            limit: self.limit,
            after: self.after,
        };
        let page = paging.page_in_order(
            listed.runs,
            |run| run.id.to_string(),
            |run, _| {
                after
                    .as_ref()
                    .is_some_and(|after| newest_first(run.started_at, &run.id) < *after)
            },
        );
        Ok(RunsPage {
            runs: page.items,
            left_out: page.left_out,
            after: page.after,
        })
    }
}

/// The run `after` names, whether or not the list still holds it, or else
/// the refusal of an `after` that names no run.
async fn run_after(
    permit: &Permit<Read>,
    context: &Context<'_>,
    after: &str,
) -> Result<RunHeading, String> {
    let id: RunId = after.parse().map_err(|_| missing(after))?;
    actions::run_heading(permit, context.sources, &id)
        .await
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| missing(after))
}

/// Where the run `id`, started at `started_at`, is in a list of runs,
/// newest first: after each run whose place is greater
/// (`docs/specs/runs.md`, Runs).
fn newest_first(started_at: Timestamp, id: &RunId) -> (Timestamp, u32, String) {
    (started_at, id.n, id.to_string())
}

/// See a run: its heading, its sessions and the latest entries of its
/// transcripts (`docs/specs/runs.md`, Run).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetRun {
    /// The run's ID, such as `43/rework-2` or `weekly.md/3`.
    run_id: String,
    /// Whether to give the transcripts in full, as the verbose switch does.
    #[serde(default)]
    verbose: bool,
    /// How many of the latest entries to give at most: 100 without it.
    limit: Option<NonZeroUsize>,
    /// The `before` of the call that gave the entries after: gives those
    /// before them.
    before: Option<String>,
}

/// A run, as [`GetRun`] gives it: its heading, its sessions, and the
/// latest of its entries, with how many earlier ones it left out and the
/// `before` that gives those.
#[derive(Serialize, JsonSchema)]
pub(super) struct RunPage {
    #[serde(flatten)]
    heading: RunHeading,
    sessions: Vec<SessionScreen>,
    /// In order.
    entries: Vec<EntryScreen>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<String>,
}

impl Tool for GetRun {
    const ACTION: Action = Action::GetRun;
    type Level = Read;
    type Output = RunPage;

    async fn run(self, permit: &Permit<Read>, context: &Context<'_>) -> Result<RunPage, String> {
        let missing = || Missing::Run(self.run_id.clone()).to_string();
        let id: RunId = self.run_id.parse().map_err(|_| missing())?;
        let verbosity = Verbosity::of(self.verbose);
        let run = actions::get_run(permit, context.sources, &id, verbosity)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(missing)?;
        let earlier = Earlier {
            limit: self.limit,
            before: self.before,
        };
        let latest = earlier.latest(run.entries, |entry| entry.id.clone())?;
        Ok(RunPage {
            heading: run.heading,
            sessions: run.sessions,
            entries: latest.items,
            left_out: latest.left_out,
            before: latest.before,
        })
    }
}

/// See the automations, by name (`docs/specs/automations.md`, Automations).
#[derive(Deserialize, JsonSchema)]
#[serde(transparent)]
pub(in crate::mcp) struct ListAutomations {
    paging: Paging,
}

/// The automations, as [`ListAutomations`] gives them: each automation file
/// that does not parse, then a page of the automations, by the names
/// `status --json` gives them.
#[derive(Serialize, JsonSchema)]
pub(super) struct AutomationsPage {
    /// Each automation file that does not parse, by path.
    unparsed_automations: Vec<UnparsedAutomation>,
    automations: Vec<AutomationScreen>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

impl Tool for ListAutomations {
    const ACTION: Action = Action::ListAutomations;
    type Level = Read;
    type Output = AutomationsPage;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<AutomationsPage, String> {
        let shown = actions::list_automations(permit, context.sources)
            .await
            .ok_or_else(|| Unexplained::NotPolledYet.to_string())?;
        let page = self.paging.page_in_order(
            shown.automations,
            |automation| automation.name.clone(),
            |automation, after| automation.name.as_str() > after,
        );
        Ok(AutomationsPage {
            unparsed_automations: shown.unparsed_automations,
            automations: page.items,
            left_out: page.left_out,
            after: page.after,
        })
    }
}

/// See Linewright itself (`docs/specs/operation.md`, Server).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "serde reads a unit struct from null, and a tool's arguments are an object"
)]
pub(in crate::mcp) struct GetServer {}

impl Tool for GetServer {
    const ACTION: Action = Action::GetServer;
    type Level = Read;
    type Output = ServerScreen;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<ServerScreen, String> {
        Ok(actions::get_server(permit, context.sources).await)
    }
}
