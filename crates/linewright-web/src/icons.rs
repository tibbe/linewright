//! The icon set the web UI draws from (`docs/specs/design_system.md`,
//! Icons): Heroicons 2's 20 solid icons and two of GitHub's Octicons,
//! vendored beside this file with their licences, and which icon each part
//! has.

use linewright_core::phrase::Status;

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
    ClipboardDocumentCheck: "clipboard-document-check", "icons/heroicons/clipboard-document-check.svg";
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
    PencilSquare: "pencil-square", "icons/heroicons/pencil-square.svg";
    Check: "check", "icons/heroicons/check.svg";
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
    ArrowRightStartOnRectangle: "arrow-right-start-on-rectangle", "icons/heroicons/arrow-right-start-on-rectangle.svg";
    Users: "users", "icons/heroicons/users.svg";
    LockClosed: "lock-closed", "icons/heroicons/lock-closed.svg";
    LockOpen: "lock-open", "icons/heroicons/lock-open.svg";
    Key: "key", "icons/heroicons/key.svg";
    LinkSlash: "link-slash", "icons/heroicons/link-slash.svg";
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

/// How a screen reader reads an icon or a logo.
#[derive(Clone, Copy)]
pub(super) enum Read<'a> {
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
        drawn_svg(self.source(), size, class, read, Marker::Icon(self.name()))
    }
}

/// The attribute that says which icon or logo an `svg` draws.
#[derive(Clone, Copy)]
pub(super) enum Marker<'a> {
    /// `data-icon`, an icon of the set, by its name.
    Icon(&'a str),
    /// `data-logo`, a logo, by its name.
    Logo(&'a str),
}

/// The vendored SVG file `source` in `size`, in the colour of the text
/// around it, of class `icon` and the classes `class` too, read as `read`
/// says and marked as `marker` says.
///
/// # Panics
///
/// If `source` is not an `svg` element with a `viewBox`.
pub(super) fn drawn_svg(
    source: &'static str,
    size: IconSize,
    class: Option<&str>,
    read: Read,
    marker: Marker,
) -> Html {
    let (view_box, paths) = view_box_and_paths(source);
    let class = class.map_or(String::new(), |class| format!(" {}", html::escape(class)));
    let read = match read {
        Read::Hidden => r#" aria-hidden="true""#.to_string(),
        Read::Named(name) => format!(r#" role="img" aria-label="{}""#, html::escape(name)),
    };
    let marker = match marker {
        Marker::Icon(name) => format!(r#"data-icon="{name}""#),
        Marker::Logo(name) => format!(r#"data-logo="{name}""#),
    };
    html::built(format!(
        r#"<svg class="icon {size} shrink-0{class}" viewBox="{view_box}" fill="currentColor"{read} {marker}>{paths}</svg>"#,
        size = size.class(),
    ))
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
        .expect("a vendored SVG has a viewBox");
    let paths = source
        .strip_prefix("<svg ")
        .and_then(|svg| svg.split_once('>'))
        .and_then(|(_, rest)| rest.trim_end().strip_suffix("</svg>"))
        .expect("a vendored SVG is an svg element")
        .lines()
        .map(str::trim)
        .collect();
    (view_box, paths)
}

/// The icon of a status (`docs/specs/ui_patterns.md`, Status).
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

/// The icon of a status, named by the status (`docs/specs/ui_patterns.md`,
/// Status), drawn inline.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "askama passes a template's values by reference, at any depth"
)]
pub(super) fn named_status_icon(status: &Status) -> Html {
    let name = match status {
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Waiting => "waiting",
        Status::InFlight => "in flight",
        Status::Paused => "paused",
        Status::Cancelled => "cancelled",
    };
    status_icon(status).named(IconSize::Inline, name)
}

/// The icon before a card's title, which stands for what the card holds
/// (`docs/specs/ui_patterns.md`, Cards and Icons).
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
    /// A task's criteria.
    Criteria,
    /// One's own authorizations.
    Authorizations,
}

impl CardIcon {
    pub(super) fn icon(self) -> Icon {
        match self {
            CardIcon::Area(area) => area.icon(),
            CardIcon::Configurations => Icon::AdjustmentsHorizontal,
            CardIcon::Grid => Icon::TableCells,
            CardIcon::Chart => Icon::ChartBar,
            CardIcon::Criteria => Icon::ClipboardDocumentCheck,
            CardIcon::Authorizations => Icon::Key,
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
    use super::*;

    /// `docs/specs/ui_patterns.md`, Icons, Which icon: each part has the icon
    /// that its section gives it.
    #[test_log::test]
    fn each_part_has_the_icon_the_specs_table_gives_it() {
        for (area, icon) in [
            (Area::Automations, Icon::Bolt),
            (Area::Server, Icon::Server),
            (Area::People, Icon::Users),
        ] {
            assert_eq!(CardIcon::Area(area).icon(), icon, "{area:?}");
        }
        for (status, icon) in [
            (Status::Done, Icon::CheckCircle),
            (Status::Waiting, Icon::HandRaised),
            (Status::InFlight, Icon::ArrowPath),
            (Status::Cancelled, Icon::NoSymbol),
        ] {
            assert_eq!(status_icon(&status), icon, "{status:?}");
        }
    }

    /// `docs/specs/design_system.md`, Icons, Sizes: an icon is `size-5` before
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
