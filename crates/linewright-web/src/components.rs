//! Linewright's own components of the web UI (`docs/specs/components.md`).
//! Each is a `Template` that is `HtmlSafe`, drawn in a template with
//! `{{ … }}` and from Rust with [`html::rendered`](super::html::rendered).

use std::fmt;

use askama::Template;

use linewright_core::domain::{ItemKind, Number};
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

/// How an item name fits the line it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fit {
    /// On one line, with only the title cut to fit, whole in the page's
    /// tooltip while it is cut (`docs/specs/ui_patterns.md`, Layout), and
    /// the number always shown.
    OneLine,
    /// The title wraps as the text around it does.
    Wrap,
}

/// An item name (`docs/specs/components.md`, Item name): the title, linking
/// to the item's Item, then the number in secondary text and `font-normal`,
/// linking to the item on GitHub, in the repo `repo`.
#[derive(Template)]
#[template(
    source = r#"{% match fit %}{% when Fit::OneLine %}<span class="inline-flex max-w-full min-w-0 items-baseline gap-[0.25em]"><a class="link link-hover min-w-0 truncate" href="/items/{{ number.0 }}" data-tip-cut>{{ title }}</a> <a class="link link-hover shrink-0 font-normal text-base-content/60" href="{{ self::github(repo, number) }}">{{ number }}</a></span>{% when Fit::Wrap %}<span><a class="link link-hover" href="/items/{{ number.0 }}">{{ title }}</a> <a class="link link-hover font-normal text-base-content/60" href="{{ self::github(repo, number) }}">{{ number }}</a></span>{% endmatch %}"#,
    ext = "html"
)]
pub(super) struct ItemName<'a> {
    pub(super) number: Number,
    pub(super) title: &'a str,
    pub(super) repo: &'a str,
    pub(super) fit: Fit,
}

impl askama::filters::HtmlSafe for ItemName<'_> {}

/// An item's number alone (`docs/specs/board.md`, Cards, Its line), linking
/// to the item on GitHub, in the repo `repo`; with Octicons'
/// `git-pull-request` before it where `kind` is a pull request's.
#[derive(Template)]
#[template(
    source = r#"{% if kind == Some(ItemKind::PullRequest) %}{{ crate::icons::pull_request_svg_before_text() }}{% endif %}<a class="link link-hover" href="{{ self::github(repo, number) }}">{{ number }}</a>"#,
    ext = "html"
)]
pub(super) struct ItemNumber<'a> {
    pub(super) number: Number,
    pub(super) repo: &'a str,
    pub(super) kind: Option<ItemKind>,
}

impl askama::filters::HtmlSafe for ItemNumber<'_> {}

/// Where item `number` of the repo `repo` is on GitHub, which leads to a
/// pull request from its issue's address too.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "askama passes a template's values by reference, at any depth"
)]
fn github(repo: &str, number: &Number) -> String {
    format!("https://github.com/{repo}/issues/{}", number.0)
}

#[cfg(test)]
mod tests {
    use super::super::html;
    use super::*;

    const REPO: &str = "owner/widgets";

    /// `docs/specs/components.md`, Item name.
    #[test_log::test]
    fn an_item_name_on_one_line_cuts_only_its_title_and_links_title_then_number() {
        let name = html::rendered(&ItemName {
            number: Number(3),
            title: "Name <the> columns",
            repo: REPO,
            fit: Fit::OneLine,
        })
        .to_string();
        assert_eq!(
            name,
            r#"<span class="inline-flex max-w-full min-w-0 items-baseline gap-[0.25em]"><a class="link link-hover min-w-0 truncate" href="/items/3" data-tip-cut>Name &#60;the&#62; columns</a> <a class="link link-hover shrink-0 font-normal text-base-content/60" href="https://github.com/owner/widgets/issues/3">#3</a></span>"#
        );
    }

    /// `docs/specs/components.md`, Item name.
    #[test_log::test]
    fn an_item_name_that_wraps_cuts_nothing() {
        let name = html::rendered(&ItemName {
            number: Number(3),
            title: "Name the columns",
            repo: REPO,
            fit: Fit::Wrap,
        })
        .to_string();
        assert_eq!(
            name,
            r#"<span><a class="link link-hover" href="/items/3">Name the columns</a> <a class="link link-hover font-normal text-base-content/60" href="https://github.com/owner/widgets/issues/3">#3</a></span>"#
        );
    }

    /// `docs/specs/board.md`, Cards, Its line.
    #[test_log::test]
    fn an_item_number_links_to_github_with_a_pull_requests_icon_before_it() {
        let link =
            r#"<a class="link link-hover" href="https://github.com/owner/widgets/issues/4">#4</a>"#;
        for kind in [None, Some(ItemKind::Issue)] {
            let number = html::rendered(&ItemNumber {
                number: Number(4),
                repo: REPO,
                kind,
            })
            .to_string();
            assert_eq!(number, link, "{kind:?}");
        }
        let pull = html::rendered(&ItemNumber {
            number: Number(4),
            repo: REPO,
            kind: Some(ItemKind::PullRequest),
        })
        .to_string();
        assert!(
            pull.starts_with(
                r#"<svg class="icon size-[1em] inline-block align-[calc(0.5cap-0.5em)] shrink-0 me-1" viewBox="0 0 16 16" fill="currentColor" role="img" aria-label="pull request" data-icon="git-pull-request">"#
            ),
            "{pull}"
        );
        assert!(pull.ends_with(&format!("</svg>{link}")), "{pull}");
    }

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
}
