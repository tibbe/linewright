//! The action that shows an item (`docs/specs/board.md`, Item), and what it
//! shows: the model that the page renders and the tool returns.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Serialize;

use super::{CardScreen, LaneHeadScreen};
use crate::access::{Permit, Read};
use crate::api::{self, Sources, Unexplained};
use crate::board::{self, CardStatus, Column};
use crate::domain::{Number, Snapshot, Timestamp};
use crate::explain;
use crate::item_facts::{self, ItemFacts, Related, StateFact};
use crate::lifecycle::State;
use crate::managed::{LeftAloneIssue, LeftAloneItem, LeftAlonePull, PullExclusion, Tickets};
use crate::phrase::Phrase;
use crate::reconcile::{NotSpecChange, Wait};
use crate::runs::{self, Run};

/// How many of an item's runs its screen lists, the latest
/// (`docs/specs/board.md`, Item, Runs).
const RUNS_SHOWN: usize = 10;

/// An item as its screen shows it (`docs/specs/board.md`, Item): its
/// facts, and its runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ItemScreen {
    #[serde(flatten)]
    pub facts: ItemFactsScreen,
    #[serde(flatten)]
    pub runs: ItemRuns,
}

/// An item's heading and facts (`docs/specs/board.md`, Item, Heading and
/// Facts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ItemFactsScreen {
    pub number: Number,
    pub title: String,
    pub pull_request: bool,
    /// When `serve` last read it.
    pub last_read: Timestamp,
    /// Its state (`docs/specs/repository.md`, State), `None` for an item
    /// with none.
    pub state: Option<State>,
    /// What its State fact says after its state.
    #[serde(flatten)]
    pub standing: Standing,
    /// Each thing Linewright waits for before it acts on it next.
    pub waiting_for: Vec<Wait>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracking_issue: Option<RelatedScreen>,
    /// A tracking issue's tickets, closed of total.
    #[serde(flatten)]
    pub tickets: Option<Tickets>,
    /// A tracking issue's tickets, in its order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ticket_items: Vec<RelatedScreen>,
    /// The spec change a spec change's issue was opened for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_change: Option<RelatedScreen>,
    /// The open or merged pull requests that close it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pull_requests: Vec<RelatedScreen>,
    /// The issues a pull request closes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub closes: Vec<RelatedScreen>,
}

/// What an item's State fact says after its state (`docs/specs/board.md`,
/// Item, Facts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "standing", rename_all = "kebab-case")]
pub enum Standing {
    /// A managed item, with its card: on the board, or the card it would
    /// have on its own.
    Card {
        column: Column,
        /// Its card's line, `None` where the card is its title alone.
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<Phrase>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<CardStatus>,
    },
    /// A tracking issue, with when its lane finished, if it did.
    Tracking {
        #[serde(skip_serializing_if = "Option::is_none")]
        finished: Option<Timestamp>,
    },
    /// An item Linewright leaves, and what keeps it out.
    LeftAlone { kept_out_by: Phrase },
}

/// An item a fact lists (`docs/specs/board.md`, Item, Related items): as
/// its card shows it, with its card status, or a tracking issue heading a
/// lane as that lane's head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum RelatedScreen {
    Card {
        number: Number,
        /// `None` where Linewright does not hold it.
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        pull_request: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<CardStatus>,
    },
    LaneHead(LaneHeadScreen),
}

/// An item's latest runs, newest first, as `runs N` lists them, with how
/// many earlier ones were left out (`docs/specs/board.md`, Item, Runs).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ItemRuns {
    pub runs: Vec<Run>,
    pub left_out: usize,
}

impl ItemFactsScreen {
    pub fn of(facts: &ItemFacts) -> ItemFactsScreen {
        let (state, standing) = match &facts.state {
            StateFact::Card {
                state,
                column,
                card,
            } => {
                let card = CardScreen::of(card, *column);
                (
                    Some(*state),
                    Standing::Card {
                        column: card.column,
                        line: card.line,
                        status: card.status,
                    },
                )
            }
            StateFact::Tracking { state, finished } => (
                *state,
                Standing::Tracking {
                    finished: *finished,
                },
            ),
            StateFact::LeftAlone { state, why } => (
                *state,
                Standing::LeftAlone {
                    kept_out_by: left_alone_worded(why),
                },
            ),
        };
        let related = |items: &[Related]| items.iter().map(RelatedScreen::of).collect();
        ItemFactsScreen {
            number: facts.number,
            title: facts.title.clone(),
            pull_request: facts.pull_request,
            last_read: facts.last_read,
            state,
            standing,
            waiting_for: facts.waiting_for.clone(),
            tracking_issue: facts.tracking_issue.as_ref().map(RelatedScreen::of),
            tickets: facts.tickets.as_ref().map(|tickets| Tickets {
                tickets_closed: tickets.closed,
                tickets: tickets.tickets.len(),
            }),
            ticket_items: facts
                .tickets
                .as_ref()
                .map(|tickets| related(&tickets.tickets))
                .unwrap_or_default(),
            spec_change: facts.spec_change.as_ref().map(RelatedScreen::of),
            pull_requests: related(&facts.pull_requests),
            closes: related(&facts.closes),
        }
    }
}

impl RelatedScreen {
    fn of(item: &Related) -> RelatedScreen {
        match item {
            Related::Card {
                number,
                title,
                pull_request,
                status,
            } => RelatedScreen::Card {
                number: *number,
                title: title.clone(),
                pull_request: *pull_request,
                status: *status,
            },
            Related::LaneHead(lane) => RelatedScreen::LaneHead(LaneHeadScreen::of(lane)),
        }
    }
}

/// What keeps `item` out of Linewright's work, in the UI's own words.
fn left_alone_worded(item: &LeftAloneItem) -> Phrase {
    match item {
        LeftAloneItem::Issue(LeftAloneIssue::Closed)
        | LeftAloneItem::PullRequest(LeftAlonePull::Closed) => "closed".into(),
        LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone }) => left_alone.worded(),
        LeftAloneItem::PullRequest(LeftAlonePull::Open { left_alone }) => match left_alone {
            PullExclusion::NotWorked { author } => {
                format!("opened by {author}, and Linewright has not worked it").into()
            }
        },
        LeftAloneItem::PullRequest(LeftAlonePull::Merged { left_alone }) => match left_alone {
            NotSpecChange::Linewrights => "opened by Linewright".into(),
            NotSpecChange::OtherBranch { base } => Phrase::from("merged into ")
                .code(base.clone())
                .text(", not the default branch"),
            NotSpecChange::NoSpecFile { spec_glob } => Phrase::from("touches no file matching ")
                .code(spec_glob.clone())
                .text(", the spec-glob"),
            NotSpecChange::TakenUp { issue } => {
                Phrase::from("a spec change taken up in ").item(*issue)
            }
            NotSpecChange::Designed { label } => Phrase::from("a spec change labeled ")
                .code(label.name())
                .text(", taken up already"),
            NotSpecChange::BeforeLookback {
                merged_at,
                window_opened,
            } => Phrase::from("merged ")
                .time(*merged_at)
                .text(", before the spec-lookback window opened ")
                .time(*window_opened),
            NotSpecChange::RequiredLabel { label } => Phrase::from("not labeled ")
                .code(label.clone())
                .text(", the required label"),
        },
        LeftAloneItem::PullRequest(LeftAlonePull::TakingUp) => {
            "a spec change Linewright takes up at its next poll".into()
        }
    }
}

/// #`number` as its screen shows it, with the snapshot of the view it was
/// answered from: from `serve`'s view for a managed item, and otherwise as
/// `serve` reads #`number` from GitHub now.
pub(crate) async fn get_item(
    permit: &Permit<Read>,
    sources: &Sources,
    number: Number,
) -> Result<(ItemScreen, Arc<Snapshot>), Unexplained> {
    let (facts, snapshot) = api::item_facts(sources, number).await?;
    let item = ItemScreen {
        facts: ItemFactsScreen::of(&facts),
        runs: item_runs(permit, sources, number).await,
    };
    Ok((item, snapshot))
}

/// What `serve`'s view holds of an item, without reading GitHub.
pub struct ItemInView {
    /// The view's, `None` before the first poll or where the runs do not
    /// read.
    pub snapshot: Option<Arc<Snapshot>>,
    /// The item's facts, `None` where the view does not hold it.
    pub facts: Option<ItemFactsScreen>,
}

/// #`number`'s facts as `serve`'s view explains them, without reading
/// GitHub.
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
            ItemFactsScreen::of(&item_facts::item_facts(
                &answer,
                &in_view.view,
                &board,
                &in_view.recorded,
                in_view.now,
            ))
        });
    ItemInView {
        snapshot: Some(in_view.view.snapshot),
        facts,
    }
}

/// #`number`'s latest runs, from their records, whether or not Linewright
/// manages it.
pub async fn item_runs(_: &Permit<Read>, sources: &Sources, number: Number) -> ItemRuns {
    let query = runs::Query {
        item: Some(number),
        limit: Some(RUNS_SHOWN),
        ..runs::Query::default()
    };
    runs::list(
        api::recorded_or_none(sources).await,
        &query,
        sources.clock.now(),
    )
    .map(|listing| ItemRuns {
        runs: listing.runs,
        left_out: listing.left_out,
    })
    .unwrap_or_default()
}
