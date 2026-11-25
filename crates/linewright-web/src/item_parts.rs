//! The parts that every place naming an item draws the same, each one
//! element: an item's title, linking to its Item, and its number, linking
//! to the item on GitHub. Each place composes them in a container of its
//! own.

use askama::Template;

use linewright_core::domain::{ItemKind, Number};

/// How a title fits the line it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cut {
    /// Cut to fit its line, whole in the page's tooltip while it is cut
    /// (`docs/specs/ui_patterns.md`, Layout).
    ToFit,
    /// Wrapping as the text around it does.
    Wrap,
}

/// An item's title, a daisyUI `link`, `link-hover`, to the item's Item
/// (`docs/specs/board.md`, Item), cut as `cut` says.
#[derive(Template)]
#[template(
    source = r#"<a class="link link-hover{% if cut == Cut::ToFit %} min-w-0 truncate{% endif %}" href="/items/{{ number.0 }}"{% if cut == Cut::ToFit %} data-tip-cut{% endif %}>{{ title }}</a>"#,
    ext = "html"
)]
pub(super) struct ItemTitle<'a> {
    pub(super) number: Number,
    pub(super) title: &'a str,
    pub(super) cut: Cut,
}

impl askama::filters::HtmlSafe for ItemTitle<'_> {}

/// An item's number after its title, in secondary text and `font-normal`,
/// a daisyUI `link`, `link-hover`, to the item on GitHub. After a title cut
/// to fit, it never shrinks, so that it always shows.
#[derive(Template)]
#[template(
    source = r#"<a class="link link-hover{% if beside == Cut::ToFit %} shrink-0{% endif %} font-normal text-base-content/60" href="{{ self::github(repo, number) }}">{{ number }}</a>"#,
    ext = "html"
)]
pub(super) struct ItemNumberLink<'a> {
    number: Number,
    repo: &'a str,
    beside: Cut,
}

impl askama::filters::HtmlSafe for ItemNumberLink<'_> {}

impl<'a> ItemNumberLink<'a> {
    /// The number of `title`'s item, in the repo `repo`, after `title`.
    pub(super) fn after(title: &ItemTitle<'_>, repo: &'a str) -> ItemNumberLink<'a> {
        ItemNumberLink {
            number: title.number,
            repo,
            beside: title.cut,
        }
    }
}

/// An item's number as a card's line names it (`docs/specs/board.md`,
/// Cards, Its line), and as an item that Linewright has not read is named:
/// a daisyUI `link`, `link-hover`, to the item on GitHub, in the repo
/// `repo`, with Octicons' `git-pull-request` before it where `kind` is a
/// pull request's. An item whose kind is not drawn has no `kind`.
#[derive(Template)]
#[template(
    source = r#"{% if kind == Some(ItemKind::PullRequest) %}{{ crate::icons::pull_request_svg_before_text() }}{% endif %}<a class="link link-hover" href="{{ self::github(repo, number) }}">{{ number }}</a>"#,
    ext = "html"
)]
pub(super) struct CardLineNumber<'a> {
    pub(super) number: Number,
    pub(super) repo: &'a str,
    pub(super) kind: Option<ItemKind>,
}

impl askama::filters::HtmlSafe for CardLineNumber<'_> {}

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

    /// `docs/specs/board.md`, Cards, Its title.
    #[test_log::test]
    fn a_title_cut_to_fit_links_to_its_item_and_is_whole_in_the_tooltip() {
        let title = html::rendered(&ItemTitle {
            number: Number(3),
            title: "Name <the> columns",
            cut: Cut::ToFit,
        })
        .to_string();
        assert_eq!(
            title,
            r#"<a class="link link-hover min-w-0 truncate" href="/items/3" data-tip-cut>Name &#60;the&#62; columns</a>"#
        );
    }

    /// `docs/specs/benchmarks.md`, Task, The task's item.
    #[test_log::test]
    fn a_title_that_wraps_cuts_nothing() {
        let title = html::rendered(&ItemTitle {
            number: Number(3),
            title: "Name the columns",
            cut: Cut::Wrap,
        })
        .to_string();
        assert_eq!(
            title,
            r#"<a class="link link-hover" href="/items/3">Name the columns</a>"#
        );
    }

    /// `docs/specs/board.md`, Cards, Its number.
    #[test_log::test]
    fn a_number_is_secondary_text_linking_to_github_and_never_shrinks_beside_a_cut_title() {
        let number = |cut| {
            let title = ItemTitle {
                number: Number(3),
                title: "Name the columns",
                cut,
            };
            html::rendered(&ItemNumberLink::after(&title, REPO)).to_string()
        };
        assert_eq!(
            number(Cut::ToFit),
            r#"<a class="link link-hover shrink-0 font-normal text-base-content/60" href="https://github.com/owner/widgets/issues/3">#3</a>"#
        );
        assert_eq!(
            number(Cut::Wrap),
            r#"<a class="link link-hover font-normal text-base-content/60" href="https://github.com/owner/widgets/issues/3">#3</a>"#
        );
    }

    /// `docs/specs/board.md`, Cards, Its line.
    #[test_log::test]
    fn a_card_lines_number_links_to_github_with_a_pull_requests_icon_before_it() {
        let link =
            r#"<a class="link link-hover" href="https://github.com/owner/widgets/issues/4">#4</a>"#;
        for kind in [None, Some(ItemKind::Issue)] {
            let number = html::rendered(&CardLineNumber {
                number: Number(4),
                repo: REPO,
                kind,
            })
            .to_string();
            assert_eq!(number, link, "{kind:?}");
        }
        let pull = html::rendered(&CardLineNumber {
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
}
