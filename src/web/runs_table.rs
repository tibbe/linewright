//! A list of runs as a table (`docs/specs/design_system.md`, Lists of runs),
//! as the runs page and the item page both show one, drawn by
//! `macros.html`'s `runs`.

use super::html::{self, Html};
use super::names::Names;
use super::run_path;
use crate::domain::Number;
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
    /// The runs of `listing`, uncut, each with its item as `names` names
    /// it.
    pub(super) fn all(listing: &Listing, names: &Names) -> RunsShown {
        let item = |run: &Run| match &run.id.of {
            RunOf::Work { item, .. } => ItemCell::Item(names.item(*item)),
            RunOf::Automation { automation } => ItemCell::Automation(automation.to_string()),
        };
        let rows = Rows::WithItems(
            listing
                .runs
                .iter()
                .map(|run| (item(run), RunRow::of(run, names)))
                .collect(),
        );
        RunsShown::of(listing, rows, None)
    }

    /// #`item`'s runs, as `runs` listed them narrowed to it, without the
    /// Item column; the earlier runs it left out listed on the runs page.
    pub(super) fn of_item(
        listed: Result<Listing, NoRuns>,
        item: Number,
        names: &Names,
    ) -> RunsShown {
        let Ok(listing) = listed else {
            return RunsShown::NoRuns;
        };
        let rows = Rows::WithoutItems(
            listing
                .runs
                .iter()
                .map(|run| RunRow::of(run, names))
                .collect(),
        );
        let left_out = (listing.left_out > 0).then(|| LeftOut {
            earlier: runs::earlier_runs(listing.left_out),
            href: format!("/runs?item={}", item.0),
        });
        RunsShown::of(&listing, rows, left_out)
    }

    fn of(listing: &Listing, rows: Rows, left_out: Option<LeftOut>) -> RunsShown {
        if listing.runs.is_empty() {
            return RunsShown::NoRuns;
        }
        RunsShown::Table(RunsTable { rows, left_out })
    }
}

impl RunRow {
    fn of(run: &Run, names: &Names) -> RunRow {
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
            duration: runs::duration(run).map(|duration| html::phrase(&duration, names)),
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
