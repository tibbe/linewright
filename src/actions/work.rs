//! The actions that show the runs and Linewright itself: Runs, Run and
//! Server.

use std::sync::Arc;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::AutomationsScreen;
use crate::access::{Permit, Read};
use crate::api::{self, Sources};
use crate::domain::{Snapshot, Timestamp};
use crate::environment::shown::BuildShown;
use crate::logs::KeptRuns;
use crate::runs::{self, ListError, Listing};
use crate::status::Linewright;

/// The runs `query` asks for, as of `now` (`docs/specs/runs.md`, Runs):
/// none for an item without runs.
pub async fn list_runs(
    _: &Permit<Read>,
    sources: &Sources,
    query: &runs::Query,
    now: Timestamp,
) -> Result<Listing> {
    match runs::answer(Some(&sources.runs), query, now).await {
        Ok(listing) => Ok(listing),
        Err(ListError::NoRuns(_)) => Ok(Listing {
            runs: vec![],
            left_out: 0,
        }),
        Err(ListError::Other(e)) => Err(e),
    }
}

/// The snapshot of `serve`'s view, which names the items, `None` before the
/// first poll.
pub fn view_snapshot(_: &Permit<Read>, sources: &Sources) -> Option<Arc<Snapshot>> {
    api::view(sources).map(|view| view.snapshot)
}

/// Where the transcripts of `serve`'s runs are read from.
pub fn run_transcripts(_: &Permit<Read>, sources: &Sources) -> KeptRuns {
    KeptRuns::Database(sources.runs.clone())
}

/// Linewright itself as Server shows it (`docs/specs/operation.md`,
/// Server), with the names `status` and `environment` give the same facts.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ServerScreen {
    /// The facts about Linewright that `status` shows.
    pub linewright: Linewright,
    /// The environment's builds, as `environment` shows them.
    pub builds: Vec<BuildShown>,
}

/// What every screen's frame shows of Linewright, from one read of
/// `serve`'s view, the runs and the environment: Linewright itself, the
/// automations, and the view's snapshot, which names the items.
pub struct Overview {
    pub server: ServerScreen,
    /// `None` before the first poll.
    pub automations: Option<AutomationsScreen>,
    /// `None` before the first poll.
    pub snapshot: Option<Arc<Snapshot>>,
}

/// Linewright itself as Server shows it now.
pub async fn get_server(permit: &Permit<Read>, sources: &Sources) -> ServerScreen {
    overview(permit, sources).await.server
}

/// What every screen's frame shows of Linewright now.
pub async fn overview(_: &Permit<Read>, sources: &Sources) -> Overview {
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
    Overview {
        automations: view
            .as_ref()
            .map(|view| AutomationsScreen::of(&status, view, &recorded, now)),
        snapshot: view.map(|view| view.snapshot),
        server: ServerScreen {
            linewright: status.linewright,
            builds,
        },
    }
}
