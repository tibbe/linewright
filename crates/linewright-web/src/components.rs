//! Linewright's own components of the web UI (`docs/specs/components.md`).
//! Each is a `Template` that is `HtmlSafe`, drawn in a template with
//! `{{ … }}` and from Rust with [`html::rendered`](super::html::rendered).

use std::fmt;

use askama::Template;

use linewright_core::phrase::Status;

use super::html::Html;
use super::icons::CardIcon;

/// The level of a card's title: `h1` for the screen's own name, `h2` under
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeadingLevel {
    H1,
    H2,
}

impl fmt::Display for HeadingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HeadingLevel::H1 => "h1",
            HeadingLevel::H2 => "h2",
        })
    }
}

/// A card's `card-title`. A card with icon has its icon first
/// (`docs/specs/components.md`, Card with icon); every other card has none.
#[derive(Template)]
#[template(
    source = r#"<{{ level }} class="card-title">{% if let Some(icon) = icon %}{{ icon.svg() }}{% endif %}{{ name }}</{{ level }}>"#,
    ext = "html"
)]
pub(super) struct CardTitle {
    pub(super) level: HeadingLevel,
    pub(super) icon: Option<CardIcon>,
    /// What the title says, escaped.
    pub(super) name: Html,
}

impl askama::filters::HtmlSafe for CardTitle {}

impl CardTitle {
    /// The `level` title `name`, plain text, with `icon` before it if it is
    /// a card with icon.
    pub(super) fn text(level: HeadingLevel, icon: Option<CardIcon>, name: &str) -> CardTitle {
        CardTitle {
            level,
            icon,
            name: super::html::escape(name),
        }
    }
}

/// A status's badge (`docs/specs/ui_patterns.md`, Status): the badge of the
/// spec's table, `badge-sm`, holding the status's icon, then `word`, so
/// that it is never told by colour alone. Paused's is a paused badge
/// (`docs/specs/components.md`, Paused badge). It is of class `tone`, which
/// a badge that is no status's is not.
#[derive(Template)]
#[template(
    source = r#"<span class="tone {{ status.badge() }} badge-sm">{{ crate::icons::status_icon(status).svg(crate::icons::IconSize::Badge) }}<span>{{ word }}</span></span>"#,
    ext = "html"
)]
pub(super) struct StatusBadge<'a> {
    pub(super) status: Status,
    pub(super) word: &'a str,
}

impl askama::filters::HtmlSafe for StatusBadge<'_> {}

impl<'a> StatusBadge<'a> {
    /// The badge of `status`, holding `word`.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "askama passes a template's values by reference, at any depth"
    )]
    pub(super) fn of(status: &Status, word: &'a str) -> StatusBadge<'a> {
        StatusBadge {
            status: *status,
            word,
        }
    }
}

/// A status in a transcript's entry (`docs/specs/components.md`,
/// Transcript, A status in an entry): the status's icon, `size-[1em]`, in
/// its colour, then `word`, `gap-1` apart, on the line's baseline.
#[derive(Template)]
#[template(
    source = r#"<span class="inline-flex items-baseline gap-1">{{ self.icon() }}<span>{{ word }}</span></span>"#,
    ext = "html"
)]
pub(super) struct EntryStatus<'a> {
    pub(super) status: Status,
    pub(super) word: &'a str,
}

impl askama::filters::HtmlSafe for EntryStatus<'_> {}

impl EntryStatus<'_> {
    /// Its status's icon, in its colour, centred on its word's line.
    fn icon(&self) -> Html {
        super::icons::status_icon(&self.status).svg_classed(
            &super::icons::IconSize::Badge,
            &format!("self-center {}", self.status.text()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::html;
    use super::*;

    /// `docs/specs/ui_patterns.md`, Status, and `docs/specs/components.md`,
    /// Paused badge.
    #[test_log::test]
    fn a_status_badge_is_its_statuss_small_badge_with_its_icon_first() {
        let done = html::rendered(&StatusBadge::of(&Status::Done, "done")).to_string();
        assert!(
            done.starts_with(
                r#"<span class="tone badge badge-success badge-sm"><svg class="icon size-[1em] shrink-0" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" data-icon="check-circle">"#
            ),
            "{done}"
        );
        assert!(done.ends_with("</svg><span>done</span></span>"), "{done}");

        let paused = html::rendered(&StatusBadge::of(&Status::Paused, "<queued>")).to_string();
        assert!(
            paused.starts_with(
                r#"<span class="tone badge badge-outline text-base-content/60 badge-sm"><svg class="icon size-[1em] shrink-0""#
            ),
            "{paused}"
        );
        assert!(
            paused.ends_with("<span>&#60;queued&#62;</span></span>"),
            "{paused}"
        );
    }

    /// `docs/specs/components.md`, Transcript, A status in an entry.
    #[test_log::test]
    fn a_status_in_an_entry_is_its_icon_in_its_colour_then_its_word_gap_1_apart() {
        let running = html::rendered(&EntryStatus {
            status: Status::InFlight,
            word: "running",
        })
        .to_string();
        assert!(
            running.starts_with(
                r#"<span class="inline-flex items-baseline gap-1"><svg class="icon size-[1em] shrink-0 self-center text-info" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" data-icon="arrow-path">"#
            ),
            "{running}"
        );
        assert!(
            running.ends_with("</svg><span>running</span></span>"),
            "{running}"
        );
    }
}
