//! A list of runs as a table (`docs/specs/design_system.md`, Lists of runs),
//! as the runs page and the item page both show one, drawn by
//! `macros.html`'s `runs`.

use super::html::{self, Html};
use super::run_path;
use crate::domain::{Number, Snapshot};
use crate::phrase::Tone;
use crate::runs::{self, Listing, NoRuns, Run, RunOf};

/// A list of runs, ready for a page, or why there is none.
pub(super) enum RunsShown {
    Table(RunsTable),
    NoRuns,
    /// Why the runs were not listed, such as a narrowing that does not read.
    Unlisted(String),
}

/// Made only by [`RunsShown::all`] and [`RunsShown::of_item`].
pub(super) struct RunsTable {
    rows: Rows,
    left_out: Option<LeftOut>,
}

/// Newest first, with an Item cell in every row or in none.
enum Rows {
    WithItems(Vec<(ItemCell, RunRow)>),
    WithoutItems(Vec<RunRow>),
}

pub(super) struct RunRow {
    pub(super) tone: Tone,
    pub(super) word: &'static str,
    pub(super) id: Html,
    pub(super) href: String,
    /// `None` for an automation's run, which has no stage.
    pub(super) stage: Option<&'static str>,
    pub(super) started: Html,
    pub(super) by_hand: bool,
    /// `None` for an interrupted run, which ran for no known time.
    pub(super) duration: Option<Html>,
}

pub(super) enum ItemCell {
    /// The item's number, and its title where known, linking to its page.
    Item(Html),
    /// The automation's name, for the cell to link to the automations page.
    Automation(String),
}

/// How many earlier runs a cut list left out, and where they are listed.
pub(super) struct LeftOut {
    pub(super) earlier: String,
    pub(super) href: String,
}

impl RunsShown {
    /// The runs of `listing`, each with its item, titled where `snapshot`
    /// holds it; the earlier runs it left out listed at `all_runs_href`.
    pub(super) fn all(
        listing: &Listing,
        snapshot: Option<&Snapshot>,
        repo: &str,
        all_runs_href: String,
    ) -> RunsShown {
        let item = |run: &Run| match &run.id.of {
            RunOf::Work { item, .. } => {
                ItemCell::Item(match snapshot.and_then(|snapshot| snapshot.title(*item)) {
                    Some(title) => html::titled(*item, title, repo),
                    None => html::item(*item, repo),
                })
            }
            RunOf::Automation { automation } => ItemCell::Automation(automation.to_string()),
        };
        let rows = Rows::WithItems(
            listing
                .runs
                .iter()
                .map(|run| (item(run), RunRow::of(run, repo)))
                .collect(),
        );
        RunsShown::of(listing, rows, all_runs_href)
    }

    /// #`item`'s runs, as `runs` listed them narrowed to it, without the
    /// Item column; the earlier runs it left out listed on the runs page.
    pub(super) fn of_item(listed: Result<Listing, NoRuns>, item: Number, repo: &str) -> RunsShown {
        let Ok(listing) = listed else {
            return RunsShown::NoRuns;
        };
        let rows = Rows::WithoutItems(
            listing
                .runs
                .iter()
                .map(|run| RunRow::of(run, repo))
                .collect(),
        );
        RunsShown::of(&listing, rows, format!("/runs?item={}", item.0))
    }

    fn of(listing: &Listing, rows: Rows, all_runs_href: String) -> RunsShown {
        if listing.runs.is_empty() {
            return RunsShown::NoRuns;
        }
        let left_out = (listing.left_out > 0).then(|| LeftOut {
            earlier: runs::earlier_runs(listing.left_out),
            href: all_runs_href,
        });
        RunsShown::Table(RunsTable { rows, left_out })
    }
}

impl RunRow {
    fn of(run: &Run, repo: &str) -> RunRow {
        RunRow {
            tone: run.outcome.tone(),
            word: run.outcome.word(),
            id: html::escape(&run.id.to_string()),
            href: run_path(&run.id),
            stage: match run.id.of {
                RunOf::Work { stage, .. } => Some(stage.name()),
                RunOf::Automation { .. } => None,
            },
            started: html::time(run.started_at),
            by_hand: run.started().by_hand(),
            duration: runs::duration(run).map(|duration| html::phrase(&duration, repo)),
        }
    }
}

impl RunsTable {
    pub(super) fn has_items(&self) -> bool {
        matches!(self.rows, Rows::WithItems(_))
    }

    /// Each row, with its Item cell if the table has the column.
    pub(super) fn rows(&self) -> Vec<(Option<&ItemCell>, &RunRow)> {
        match &self.rows {
            Rows::WithItems(rows) => rows.iter().map(|(item, row)| (Some(item), row)).collect(),
            Rows::WithoutItems(rows) => rows.iter().map(|row| (None, row)).collect(),
        }
    }

    /// The heads of the columns the table shows, Item among them only when
    /// its rows have Item cells.
    pub(super) fn heads(&self) -> Vec<&'static str> {
        let item = self.has_items().then_some("Item");
        ["Outcome", "Run"]
            .into_iter()
            .chain(item)
            .chain(["Stage", "Started", "Duration"])
            .collect()
    }

    /// How many columns the table shows, which the cut list's last row
    /// spans.
    pub(super) fn columns(&self) -> usize {
        self.heads().len()
    }

    pub(super) fn left_out(&self) -> Option<&LeftOut> {
        self.left_out.as_ref()
    }
}
