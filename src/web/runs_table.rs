//! A list of runs as a table (`docs/specs/runs.md`, Runs).

use askama::Template;

use super::html::{self, Html};
use super::names::Names;
use super::run_path;
use super::table::{Align, Column, View};
use crate::domain::Number;
use crate::phrase::Status;
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
    pub(super) status: Status,
    pub(super) word: &'static str,
    pub(super) id: Html,
    pub(super) view: View,
    /// `None` for an automation's run, which has no stage.
    pub(super) stage: Option<&'static str>,
    pub(super) started: Html,
    pub(super) by_hand: bool,
    /// `None` for an interrupted run, which ran for no known time.
    pub(super) duration: Option<Html>,
}

/// A column of a list of runs.
#[derive(Clone, Copy)]
pub(super) enum RunsColumn {
    Outcome,
    Run,
    Item,
    Stage,
    Started,
    Duration,
    /// "View", linking to the run's Run.
    View,
}

impl RunsColumn {
    pub(super) fn head(self) -> &'static str {
        match self {
            RunsColumn::Outcome => "Outcome",
            RunsColumn::Run => "Run",
            RunsColumn::Item => "Item",
            RunsColumn::Stage => "Stage",
            RunsColumn::Started => "Started",
            RunsColumn::Duration => "Duration",
            RunsColumn::View => "",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    pub(super) fn align(self) -> &'static str {
        let column = match self {
            RunsColumn::Duration => Column::Duration,
            RunsColumn::Outcome
            | RunsColumn::Run
            | RunsColumn::Item
            | RunsColumn::Stage
            | RunsColumn::Started
            | RunsColumn::View => Column::Other,
        };
        Align::of(column).class()
    }
}

/// What a run is of, as the Item column names it.
pub(super) enum ItemCell {
    /// The item's number, and its title where known, linking to its page.
    Item(Html),
    /// The automation's name, for the cell to link to the automations page.
    Automation(String),
}

impl ItemCell {
    /// What the run of `of` is of, its item as `names` names it.
    pub(super) fn of(of: &RunOf, names: &Names) -> ItemCell {
        match of {
            RunOf::Work { item, .. } => ItemCell::Item(names.item(*item)),
            RunOf::Automation { automation } => ItemCell::Automation(automation.to_string()),
        }
    }

    /// The cell's content as markup.
    pub(super) fn html(&self) -> Html {
        html::rendered(&ItemCellHtml { item: self })
    }
}

#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::item_cell(item) %}{% endcall %}"#,
    ext = "html"
)]
struct ItemCellHtml<'a> {
    item: &'a ItemCell,
}

/// A list of runs as the region `runs` of its page.
#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::runs(runs) %}{% endcall %}"#,
    ext = "html"
)]
pub(super) struct RunsRegion<'a> {
    pub(super) runs: &'a RunsShown,
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
        let rows = Rows::WithItems(
            listing
                .runs
                .iter()
                .map(|run| (ItemCell::of(&run.id.of, names), RunRow::of(run, names)))
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
            status: run.outcome.status(),
            word: run.outcome.word(),
            id: html::escape(&run.id.to_string()),
            view: View {
                href: run_path(&run.id),
                name: format!("run {}", run.id),
            },
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
    fn has_items(&self) -> bool {
        matches!(self.rows, Rows::WithItems(_))
    }

    /// Each row, with its Item cell if the table has the column.
    pub(super) fn rows(&self) -> Vec<(Option<&ItemCell>, &RunRow)> {
        match &self.rows {
            Rows::WithItems(rows) => rows.iter().map(|(item, row)| (Some(item), row)).collect(),
            Rows::WithoutItems(rows) => rows.iter().map(|row| (None, row)).collect(),
        }
    }

    /// The columns the table shows, Item among them only when its rows
    /// have Item cells.
    pub(super) fn columns(&self) -> Vec<RunsColumn> {
        let item = self.has_items().then_some(RunsColumn::Item);
        [RunsColumn::Outcome, RunsColumn::Run]
            .into_iter()
            .chain(item)
            .chain([
                RunsColumn::Stage,
                RunsColumn::Started,
                RunsColumn::Duration,
                RunsColumn::View,
            ])
            .collect()
    }

    /// How many columns the table shows.
    pub(super) fn column_count(&self) -> usize {
        self.columns().len()
    }

    pub(super) fn left_out(&self) -> Option<&LeftOut> {
        self.left_out.as_ref()
    }
}
