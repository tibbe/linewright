//! A list of runs as a table (`docs/specs/runs.md`, Runs).

use askama::Template;

use linewright_core::actions::ItemRuns;
use linewright_core::domain::Number;
use linewright_core::phrase::Status;
use linewright_core::runs::{self, Listing, Run, RunOf};

use super::html::{self, Html};
use super::names::Naming;
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::run_path;
use super::table::{Align, Column, View};

/// A list of runs, ready for a page, or why there is none.
pub(super) enum RunsShown {
    Table(Box<RunsTable>),
    NoRuns,
    /// Why the runs were not listed, such as a narrowing that does not read.
    Unlisted(String),
}

/// Made only by [`RunsShown::all`] and [`RunsShown::of_item`].
pub(super) struct RunsTable {
    rows: Rows,
    left_out: Option<LeftOut>,
    pages: Option<PagesShown>,
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
    /// An item that Linewright has read: its title, cut to fit its line,
    /// then its number (`docs/specs/runs.md`, Runs, The item).
    Read {
        number: Number,
        title: String,
        repo: String,
    },
    /// An item that Linewright has not read, named by its number alone.
    Unread { number: Number, repo: String },
    /// The automation's name, for the cell to link to the automations page.
    Automation(String),
}

impl ItemCell {
    /// What the run of `of` is of, its item as `naming` names it.
    pub(super) fn of(of: &RunOf, naming: &Naming) -> ItemCell {
        match of {
            RunOf::Work { item, .. } => {
                let repo = naming.repo().to_owned();
                match naming.title(*item) {
                    Some(title) => ItemCell::Read {
                        number: *item,
                        title: title.to_owned(),
                        repo,
                    },
                    None => ItemCell::Unread {
                        number: *item,
                        repo,
                    },
                }
            }
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
    /// The `requested` page of the runs of `listing`, each with its item
    /// as `naming` names it, its links to other pages leading where `href`
    /// says.
    pub(super) fn all(
        listing: &Listing,
        naming: &Naming,
        requested: Requested,
        href: impl Fn(Requested) -> PageHref,
    ) -> RunsShown {
        let pages = Pages::of(listing.runs.len(), requested);
        let rows = Rows::WithItems(
            listing.runs[pages.range()]
                .iter()
                .map(|run| (ItemCell::of(&run.id.of, naming), RunRow::of(run, naming)))
                .collect(),
        );
        RunsShown::of(&listing.runs, rows, None, pages.shown("runs", href))
    }

    /// #`item`'s runs, as `runs` listed them narrowed to it, without the
    /// Item column; the earlier runs it left out listed on the runs page.
    pub(super) fn of_item(listed: &ItemRuns, item: Number, naming: &Naming) -> RunsShown {
        let rows = Rows::WithoutItems(
            listed
                .runs
                .iter()
                .map(|run| RunRow::of(run, naming))
                .collect(),
        );
        let left_out = (listed.left_out > 0).then(|| LeftOut {
            earlier: runs::earlier_runs(listed.left_out),
            href: format!("/runs?item={}", item.0),
        });
        RunsShown::of(&listed.runs, rows, left_out, None)
    }

    fn of(
        runs: &[Run],
        rows: Rows,
        left_out: Option<LeftOut>,
        pages: Option<PagesShown>,
    ) -> RunsShown {
        if runs.is_empty() {
            return RunsShown::NoRuns;
        }
        RunsShown::Table(Box::new(RunsTable {
            rows,
            left_out,
            pages,
        }))
    }
}

impl RunRow {
    fn of(run: &Run, naming: &Naming) -> RunRow {
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
            duration: runs::duration(run).map(|duration| html::phrase(&duration, naming)),
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

    /// The pages under the table, where it has more runs than a page shows.
    pub(super) fn pages(&self) -> Option<&PagesShown> {
        self.pages.as_ref()
    }
}
