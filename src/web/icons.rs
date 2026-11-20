//! Every icon the web UI draws (`docs/specs/web/design_system.md`, Icons):
//! Heroicons 2's 20 solid icons and two of GitHub's Octicons, vendored
//! beside this file with their licences, and which icon each part has.

use crate::phrase::Status;

use super::html::{self, Html};
use super::shell::Area;

/// The icons, each its variant, its name as Heroicons and Octicons name it,
/// and its vendored SVG file.
macro_rules! icons {
    ($($(#[$attribute:meta])* $variant:ident: $name:literal, $file:literal;)*) => {
        /// An icon, named as Heroicons and Octicons name it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) enum Icon {
            $($(#[$attribute])* $variant,)*
        }

        impl Icon {
            #[cfg(test)]
            const ALL: &[Icon] = &[$(Icon::$variant,)*];

            fn name(self) -> &'static str {
                match self {
                    $(Icon::$variant => $name,)*
                }
            }

            fn source(self) -> &'static str {
                match self {
                    $(Icon::$variant => include_str!($file),)*
                }
            }
        }
    };
}

icons! {
    ViewColumns: "view-columns", "icons/heroicons/view-columns.svg";
    CommandLine: "command-line", "icons/heroicons/command-line.svg";
    Bolt: "bolt", "icons/heroicons/bolt.svg";
    Beaker: "beaker", "icons/heroicons/beaker.svg";
    Server: "server", "icons/heroicons/server.svg";
    AdjustmentsHorizontal: "adjustments-horizontal", "icons/heroicons/adjustments-horizontal.svg";
    TableCells: "table-cells", "icons/heroicons/table-cells.svg";
    ChartBar: "chart-bar", "icons/heroicons/chart-bar.svg";
    CheckCircle: "check-circle", "icons/heroicons/check-circle.svg";
    XCircle: "x-circle", "icons/heroicons/x-circle.svg";
    HandRaised: "hand-raised", "icons/heroicons/hand-raised.svg";
    ArrowPath: "arrow-path", "icons/heroicons/arrow-path.svg";
    PauseCircle: "pause-circle", "icons/heroicons/pause-circle.svg";
    NoSymbol: "no-symbol", "icons/heroicons/no-symbol.svg";
    Plus: "plus", "icons/heroicons/plus.svg";
    StopCircle: "stop-circle", "icons/heroicons/stop-circle.svg";
    Trash: "trash", "icons/heroicons/trash.svg";
    Play: "play", "icons/heroicons/play.svg";
    XMark: "x-mark", "icons/heroicons/x-mark.svg";
    Bars3: "bars-3", "icons/heroicons/bars-3.svg";
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the button that expands and collapses the navigation is not drawn yet"
        )
    )]
    ChevronDoubleRight: "chevron-double-right", "icons/heroicons/chevron-double-right.svg";
    ChevronRight: "chevron-right", "icons/heroicons/chevron-right.svg";
    ChevronDown: "chevron-down", "icons/heroicons/chevron-down.svg";
    MagnifyingGlass: "magnifying-glass", "icons/heroicons/magnifying-glass.svg";
    ArrowTopRightOnSquare: "arrow-top-right-on-square", "icons/heroicons/arrow-top-right-on-square.svg";
    SignalSlash: "signal-slash", "icons/heroicons/signal-slash.svg";
    ArrowDown: "arrow-down", "icons/heroicons/arrow-down.svg";
    IssueOpened: "issue-opened", "icons/octicons/issue-opened-16.svg";
    GitPullRequest: "git-pull-request", "icons/octicons/git-pull-request-16.svg";
}

/// Where an icon sits, which sizes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IconSize {
    /// Before a `card-title`.
    CardTitle,
    /// In a `badge`, as daisyUI's badge with an icon has it.
    Badge,
    /// Anywhere else.
    Inline,
}

impl IconSize {
    fn class(self) -> &'static str {
        match self {
            IconSize::CardTitle => "size-5",
            IconSize::Badge => "size-[1em]",
            IconSize::Inline => "size-4",
        }
    }
}

/// How a screen reader reads an icon.
#[derive(Clone, Copy)]
enum Read<'a> {
    /// It skips it, for an icon beside the words it stands for.
    Hidden,
    /// As an image of this name.
    Named(&'a str),
}

impl Icon {
    /// The icon in `size`, in the colour of the text around it, of class
    /// `icon`, and hidden from screen readers.
    pub(super) fn svg(self, size: IconSize) -> Html {
        self.drawn(size, None, Read::Hidden)
    }

    /// The icon as [`Icon::svg`] draws it, with the classes `class` too,
    /// such as one that shows it only while something is open.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "askama passes a template's values by reference, at any depth"
    )]
    pub(super) fn svg_classed(self, size: &IconSize, class: &str) -> Html {
        self.drawn(
            *size,
            Some(class).filter(|class| !class.is_empty()),
            Read::Hidden,
        )
    }

    /// The icon as [`Icon::svg`] draws it, but an image a screen reader
    /// reads as `name`.
    pub(super) fn named(self, size: IconSize, name: &str) -> Html {
        self.drawn(size, None, Read::Named(name))
    }

    fn drawn(self, size: IconSize, class: Option<&str>, read: Read) -> Html {
        let (view_box, paths) = view_box_and_paths(self.source());
        let class = class.map_or(String::new(), |class| format!(" {}", html::escape(class)));
        let read = match read {
            Read::Hidden => r#" aria-hidden="true""#.to_string(),
            Read::Named(name) => format!(r#" role="img" aria-label="{}""#, html::escape(name)),
        };
        html::built(format!(
            r#"<svg class="icon {size} shrink-0{class}" viewBox="{view_box}" fill="currentColor"{read} data-icon="{name}">{paths}</svg>"#,
            size = size.class(),
            name = self.name(),
        ))
    }
}

/// The `viewBox` of the SVG file `source`, and what its `svg` holds, with
/// no space between its elements.
///
/// # Panics
///
/// If `source` is not an `svg` element with a `viewBox`.
fn view_box_and_paths(source: &'static str) -> (&'static str, String) {
    let view_box = source
        .split_once(r#"viewBox=""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(view_box, _)| view_box)
        .expect("a vendored icon has a viewBox");
    let paths = source
        .strip_prefix("<svg ")
        .and_then(|svg| svg.split_once('>'))
        .and_then(|(_, rest)| rest.trim_end().strip_suffix("</svg>"))
        .expect("a vendored icon is an svg element")
        .lines()
        .map(str::trim)
        .collect();
    (view_box, paths)
}

/// The icon of a status (`docs/specs/web/design_system.md`, Icons).
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "askama passes a template's values by reference, at any depth"
)]
pub(super) fn status_icon(status: &Status) -> Icon {
    match status {
        Status::Done => Icon::CheckCircle,
        Status::Failed => Icon::XCircle,
        Status::Waiting => Icon::HandRaised,
        Status::InFlight => Icon::ArrowPath,
        Status::Paused => Icon::PauseCircle,
        Status::Cancelled => Icon::NoSymbol,
    }
}

/// The icon before a card's title, which stands for what the card holds
/// (`docs/specs/web/design_system.md`, Cards and Icons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CardIcon {
    /// What an area holds, by its icon in the navigation.
    Area(Area),
    /// A benchmark run's configurations.
    Configurations,
    /// A grid of passes, a task's under each configuration.
    Grid,
    /// A chart.
    Chart,
}

impl CardIcon {
    pub(super) fn icon(self) -> Icon {
        match self {
            CardIcon::Area(area) => area.icon(),
            CardIcon::Configurations => Icon::AdjustmentsHorizontal,
            CardIcon::Grid => Icon::TableCells,
            CardIcon::Chart => Icon::ChartBar,
        }
    }
}

/// Whether an item is an issue or a pull request, which its card carries
/// beside its number (`docs/specs/board.md`, Cards).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ItemKind {
    Issue,
    PullRequest,
}

impl ItemKind {
    /// A pull request where `pull_request`, and an issue otherwise.
    pub(super) fn of(pull_request: bool) -> ItemKind {
        if pull_request {
            ItemKind::PullRequest
        } else {
            ItemKind::Issue
        }
    }

    pub(super) fn icon(self) -> Icon {
        match self {
            ItemKind::Issue => Icon::IssueOpened,
            ItemKind::PullRequest => Icon::GitPullRequest,
        }
    }

    /// Its icon beside a number, which a screen reader reads before it.
    pub(super) fn svg(self) -> Html {
        let name = match self {
            ItemKind::Issue => "issue",
            ItemKind::PullRequest => "pull request",
        };
        self.icon().named(IconSize::Inline, name)
    }
}

#[cfg(test)]
mod tests {
    use super::super::buttons::Verb;
    use super::*;

    /// `docs/specs/web/design_system.md`, Icons, Which icon.
    #[test_log::test]
    fn each_part_has_the_icon_the_specs_table_gives_it() {
        let areas = [
            (Area::Board, Icon::ViewColumns),
            (Area::Runs, Icon::CommandLine),
            (Area::Automations, Icon::Bolt),
            (Area::Benchmarks, Icon::Beaker),
            (Area::Server, Icon::Server),
        ];
        assert_eq!(areas.len(), Area::ALL.len());
        for (area, icon) in areas {
            assert_eq!(area.icon(), icon, "{area:?}");
            assert_eq!(CardIcon::Area(area).icon(), icon, "{area:?}");
        }
        for (card, icon) in [
            (CardIcon::Configurations, Icon::AdjustmentsHorizontal),
            (CardIcon::Grid, Icon::TableCells),
            (CardIcon::Chart, Icon::ChartBar),
        ] {
            assert_eq!(card.icon(), icon, "{card:?}");
        }
        for (status, icon) in [
            (Status::Done, Icon::CheckCircle),
            (Status::Failed, Icon::XCircle),
            (Status::Waiting, Icon::HandRaised),
            (Status::InFlight, Icon::ArrowPath),
            (Status::Paused, Icon::PauseCircle),
            (Status::Cancelled, Icon::NoSymbol),
        ] {
            assert_eq!(status_icon(&status), icon, "{status:?}");
        }
        for (verb, label, icon) in [
            (Verb::AddTask, "Add task", Icon::Plus),
            (Verb::CancelWork, "Cancel", Icon::StopCircle),
            (Verb::Remove, "Remove", Icon::Trash),
            (Verb::Create, "Create", Icon::Plus),
            (Verb::RunBenchmark, "Run benchmark", Icon::Play),
            (Verb::AddConfiguration, "Add a configuration", Icon::Plus),
            (Verb::KeepRunning, "Keep running", Icon::Play),
            (Verb::CloseDialog, "Cancel", Icon::XMark),
        ] {
            assert_eq!((verb.label(), verb.icon()), (label, icon), "{verb:?}");
        }
        assert_eq!(ItemKind::Issue.icon(), Icon::IssueOpened);
        assert_eq!(ItemKind::PullRequest.icon(), Icon::GitPullRequest);
    }

    /// `docs/specs/web/design_system.md`, Icons: an icon is `size-5` before
    /// a `card-title`, `size-[1em]` in a `badge`, and `size-4` elsewhere.
    #[test_log::test]
    fn an_icon_is_sized_by_where_it_sits_and_takes_the_colour_of_its_text() {
        for (size, class) in [
            (IconSize::CardTitle, "size-5"),
            (IconSize::Badge, "size-[1em]"),
            (IconSize::Inline, "size-4"),
        ] {
            assert_eq!(
                Icon::Plus.svg(size).to_string(),
                format!(
                    r#"<svg class="icon {class} shrink-0" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" data-icon="plus"><path d="M10.75 4.75a.75.75 0 0 0-1.5 0v4.5h-4.5a.75.75 0 0 0 0 1.5h4.5v4.5a.75.75 0 0 0 1.5 0v-4.5h4.5a.75.75 0 0 0 0-1.5h-4.5v-4.5Z"/></svg>"#
                ),
            );
        }
    }

    #[test_log::test]
    fn every_icon_draws_its_vendored_svg() {
        for icon in Icon::ALL {
            let svg = icon.svg(IconSize::Inline).to_string();
            assert!(svg.contains(r#" viewBox="0 0 "#), "{icon:?}: {svg}");
            assert!(svg.contains("><path "), "{icon:?}: {svg}");
        }
    }

    #[test_log::test]
    fn a_named_icon_is_an_image_a_screen_reader_reads() {
        assert_eq!(
            ItemKind::Issue.svg().to_string(),
            r#"<svg class="icon size-4 shrink-0" viewBox="0 0 16 16" fill="currentColor" role="img" aria-label="issue" data-icon="issue-opened"><path d="M8 9.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z"/><path d="M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0ZM1.5 8a6.5 6.5 0 1 0 13 0 6.5 6.5 0 0 0-13 0Z"/></svg>"#,
        );
    }
}
