//! Linewright's own components of the web UI (`docs/specs/components.md`).
//! Each is a `Template` that is `HtmlSafe`, drawn in a template with
//! `{{ … }}` and from Rust with [`html::rendered`](super::html::rendered).

use std::fmt;

use askama::Template;

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
