//! What a run of a stage was given as it started, as GitHub gave it then
//! (`docs/specs/benchmarks.md`, "Recording a run's input").

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{Filed, Number, Snapshot, Timestamp};
use crate::git::CommitId;
use crate::reconcile::Work;
use crate::spec_gap;
use crate::store::{API_VERSION, RecordedItem, Resource, Store};

/// A run's input, as recorded at its start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    /// The `X-GitHub-Api-Version` the items were read with.
    pub api_version: String,
    pub start: Timestamp,
    /// Each with only the comments written at or before `start`.
    pub items: Vec<RecordedItem>,
    /// Each branch the run was given that existed at the start, with its
    /// head.
    pub heads: BTreeMap<String, CommitId>,
}

/// The items and branches a run is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub items: BTreeSet<Number>,
    /// The default branch, then, for an implement run, its ticket's branch.
    pub branches: Vec<String>,
}

/// What a run of `work` on `snapshot` is given: its item, the item's parent,
/// the spec change it was opened for or cut from and the spec gaps filed for
/// it and still open, each it has, and its branches. `None` for rework,
/// fix-ci and update, which record no input.
pub fn wanted(snapshot: &Snapshot, work: &Work, default_branch: &str) -> Option<Wanted> {
    let (number, implement) = match work {
        Work::Triage { issue } | Work::DesignIssue { issue } => (*issue, false),
        Work::Implement { issue } => (*issue, true),
        Work::Rework { .. } | Work::FixCi { .. } | Work::Update { .. } => return None,
    };
    let mut items = BTreeSet::from([number]);
    let mut branches = vec![default_branch.to_string()];
    if let Some(issue) = snapshot.issue(number) {
        let parent = issue.parent.as_ref();
        items.extend(parent.map(|parent| parent.number));
        items.extend(
            issue
                .filed
                .and_then(Filed::spec_change)
                .or_else(|| parent.and_then(|parent| parent.spec_change)),
        );
        items.extend(
            spec_gap::filed_for(snapshot, number)
                .into_iter()
                .map(|gap| gap.number),
        );
        if implement {
            branches.push(snapshot.ticket_branch(issue));
        }
    }
    Some(Wanted { items, branches })
}

/// `items`, each with only the comments GitHub says were created at or
/// before `start`.
pub fn cut_comments_at(items: Vec<RecordedItem>, start: Timestamp) -> Vec<RecordedItem> {
    items
        .into_iter()
        .map(|mut item| {
            let comments = Resource::Comments.path(item.number);
            if let Some(Value::Array(listed)) = item.resources.get_mut(&comments) {
                listed.retain(|comment| {
                    comment["created_at"]
                        .as_str()
                        .and_then(|at| at.parse::<Timestamp>().ok())
                        .is_some_and(|at| at <= start)
                });
            }
            item
        })
        .collect()
}

/// Reads from `store` what `wanted` names, as it stands at `start`.
pub async fn record(store: &dyn Store, wanted: &Wanted, start: Timestamp) -> Result<Input> {
    let items = store.record_items(&wanted.items).await?;
    let heads = store.heads(&wanted.branches).await?;
    Ok(Input {
        api_version: API_VERSION.to_string(),
        start,
        items: cut_comments_at(items, start),
        heads,
    })
}
