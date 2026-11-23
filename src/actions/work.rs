//! The actions that show the work and Linewright itself: Board, Item, Runs,
//! Run, Automations and Server.

use std::sync::Arc;

use anyhow::Result;

use crate::access::{Permit, Read};
use crate::api::{self, Sources, Unexplained};
use crate::board::{self, Board};
use crate::domain::{Number, Snapshot, Timestamp};
use crate::environment::shown::BuildShown;
use crate::explain;
use crate::item_facts::{self, ItemFacts};
use crate::logs::KeptRuns;
use crate::runs::{self, ListError, Listing, NoRuns, Recorded, RunId};
use crate::status::{Status, View};

/// The board (`docs/specs/board.md`, Board), empty before the first poll.
pub async fn get_board(_: &Permit<Read>, sources: &Sources) -> Board {
    api::board_answer(sources).await
}

/// The facts of #`number` (`docs/specs/board.md`, Item), with the snapshot
/// of the view they were answered from: from `serve`'s view for a managed
/// item, and otherwise as `serve` reads #`number` from GitHub now.
pub(crate) async fn get_item(
    _: &Permit<Read>,
    sources: &Sources,
    number: Number,
) -> Result<(ItemFacts, Arc<Snapshot>), Unexplained> {
    api::item_facts(sources, number).await
}

/// What `serve`'s view holds of an item, without reading GitHub.
pub struct ItemInView {
    /// The view's, `None` before the first poll or where the runs do not
    /// read.
    pub snapshot: Option<Arc<Snapshot>>,
    /// The item's facts, `None` where the view does not hold it.
    pub facts: Option<ItemFacts>,
}

/// #`number` as `serve`'s view explains it, without reading GitHub.
pub async fn item_in_view(_: &Permit<Read>, sources: &Sources, number: Number) -> ItemInView {
    let Ok(in_view) = api::explaining(sources).await else {
        return ItemInView {
            snapshot: None,
            facts: None,
        };
    };
    let facts = explain::explain_in_view(&in_view.view, &in_view.recorded, number, in_view.now)
        .map(|answer| {
            let board = board::board(&in_view.view, &in_view.recorded, in_view.now);
            item_facts::item_facts(
                &answer,
                &in_view.view,
                &board,
                &in_view.recorded,
                in_view.now,
            )
        });
    ItemInView {
        snapshot: Some(in_view.view.snapshot),
        facts,
    }
}

/// #`number`'s latest `limit` runs, from their records, whether or not
/// Linewright manages it.
pub async fn item_runs(
    _: &Permit<Read>,
    sources: &Sources,
    number: Number,
    limit: usize,
) -> Result<Listing, NoRuns> {
    let query = runs::Query {
        item: Some(number),
        limit: Some(limit),
        ..runs::Query::default()
    };
    runs::list(
        api::recorded_or_none(sources).await,
        &query,
        sources.clock.now(),
    )
}

/// The runs `query` asks for, as of `now` (`docs/specs/runs.md`, Runs).
pub async fn list_runs(
    _: &Permit<Read>,
    sources: &Sources,
    query: &runs::Query,
    now: Timestamp,
) -> Result<Listing, ListError> {
    runs::answer(Some(&sources.runs), query, now).await
}

/// The snapshot of `serve`'s view, which names the items, `None` before the
/// first poll.
pub fn view_snapshot(_: &Permit<Read>, sources: &Sources) -> Option<Arc<Snapshot>> {
    api::view(sources).map(|view| view.snapshot)
}

/// Run `run`'s record, `None` where there is no such run
/// (`docs/specs/runs.md`, Run).
pub async fn get_run(_: &Permit<Read>, sources: &Sources, run: &RunId) -> Result<Option<Recorded>> {
    sources.runs.try_read_run(run).await
}

/// Where the transcripts of `serve`'s runs are read from.
pub fn run_transcripts(_: &Permit<Read>, sources: &Sources) -> KeptRuns {
    KeptRuns::Database(sources.runs.clone())
}

/// What `status` answers of Linewright, and what it answered from
/// (`docs/specs/operation.md`, Server).
pub struct ServerAnswer {
    pub status: Status,
    /// `None` before the first poll.
    pub view: Option<View>,
    pub recorded: Vec<Recorded>,
    /// The environment's builds, from the same read as what `status` says
    /// of it.
    pub builds: Vec<BuildShown>,
    pub now: Timestamp,
}

/// What `status` answers of Linewright now, from one read of `serve`'s view,
/// the runs and the environment.
pub async fn get_server(_: &Permit<Read>, sources: &Sources) -> ServerAnswer {
    let view = api::view(sources);
    let recorded = api::recorded_or_none(sources).await;
    let now = sources.clock.now();
    let (environment, builds) = api::environment_or_none(sources)
        .await
        .and_then(|answer| answer.environment)
        .map_or((None, Vec::new()), |environment| {
            (Some(environment.shown), environment.builds)
        });
    let status = api::status_of(sources, view.as_ref(), &recorded, environment);
    ServerAnswer {
        status,
        view,
        recorded,
        builds,
        now,
    }
}
