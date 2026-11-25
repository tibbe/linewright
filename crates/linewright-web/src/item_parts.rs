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
    /// (`docs/specs/design_system.md`, Layout, Hover and focus).
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

/// How heavy an item's number is beside its title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Weight {
    /// `font-normal`, whatever the weight of the text around it.
    Normal,
    /// The weight of the text around it.
    Inherit,
}

/// An item's number, in secondary text, a daisyUI `link`, `link-hover`, to
/// the item on GitHub, in the repo `repo`, as heavy as `weight` says.
/// Beside a title cut to fit, it never shrinks, so that it always shows.
#[derive(Template)]
#[template(
    source = r#"<a class="link link-hover shrink-0{% if weight == Weight::Normal %} font-normal{% endif %} text-base-content/60" href="{{ self::github(repo, number) }}">{{ number }}</a>"#,
    ext = "html"
)]
pub(super) struct ItemNumberLink<'a> {
    number: Number,
    repo: &'a str,
    weight: Weight,
}

impl askama::filters::HtmlSafe for ItemNumberLink<'_> {}

impl<'a> ItemNumberLink<'a> {
    /// The number of `title`'s item, in the repo `repo`, as heavy as
    /// `weight` says.
    pub(super) fn after(
        title: &ItemTitle<'_>,
        repo: &'a str,
        weight: Weight,
    ) -> ItemNumberLink<'a> {
        ItemNumberLink {
            number: title.number,
            repo,
            weight,
        }
    }

    /// Item `number`'s number alone, in the repo `repo`, as an item that
    /// Linewright has not read is named: in the weight of the text around
    /// it.
    pub(super) fn alone(number: Number, repo: &'a str) -> ItemNumberLink<'a> {
        ItemNumberLink {
            number,
            repo,
            weight: Weight::Inherit,
        }
    }
}

/// An item on a line of its own: its title, cut to fit, then its number,
/// `gap-1` apart, as heavy as `weight` says (`docs/specs/board.md`, Cards,
/// and `docs/specs/runs.md`, Runs, The item).
#[derive(Template)]
#[template(
    source = r#"<span class="flex min-w-0 items-baseline gap-1">{{ title }} {{ number }}</span>"#,
    ext = "html"
)]
pub(super) struct OneLineItem<'a> {
    title: ItemTitle<'a>,
    number: ItemNumberLink<'a>,
}

impl askama::filters::HtmlSafe for OneLineItem<'_> {}

impl<'a> OneLineItem<'a> {
    /// Item `number`, titled `title`, in the repo `repo`, its number as
    /// heavy as `weight` says.
    pub(super) fn of(
        number: Number,
        title: &'a str,
        repo: &'a str,
        weight: Weight,
    ) -> OneLineItem<'a> {
        let title = ItemTitle {
            number,
            title,
            cut: Cut::ToFit,
        };
        OneLineItem {
            number: ItemNumberLink::after(&title, repo, weight),
            title,
        }
    }
}

/// An item as one run of text: its title, wrapping, then a space, then its
/// number, as heavy as `weight` says, in one plain span with no class, so
/// that the two wrap between words as the text around them does, even in a flex
/// parent such as a `card-title` (`docs/specs/board.md`, Item, Heading, and
/// Items in Waiting for and in what keeps #N out; `docs/specs/runs.md`,
/// Run, The heading's item).
#[derive(Template)]
#[template(source = "<span>{{ title }} {{ number }}</span>", ext = "html")]
pub(super) struct TextItem<'a> {
    title: ItemTitle<'a>,
    number: ItemNumberLink<'a>,
}

impl askama::filters::HtmlSafe for TextItem<'_> {}

impl<'a> TextItem<'a> {
    /// Item `number`, titled `title`, in the repo `repo`, its number as
    /// heavy as `weight` says.
    pub(super) fn of(
        number: Number,
        title: &'a str,
        repo: &'a str,
        weight: Weight,
    ) -> TextItem<'a> {
        let title = ItemTitle {
            number,
            title,
            cut: Cut::Wrap,
        };
        TextItem {
            number: ItemNumberLink::after(&title, repo, weight),
            title,
        }
    }
}

/// An item among words: its title, wrapping, then its number, `gap-1`
/// apart, the number in the weight of the words around it
/// (`docs/specs/benchmarks.md`, Task, The task's item).
#[derive(Template)]
#[template(
    source = r#"<span class="inline-flex max-w-full items-baseline gap-1">{{ title }} {{ number }}</span>"#,
    ext = "html"
)]
pub(super) struct InlineItem<'a> {
    title: ItemTitle<'a>,
    number: ItemNumberLink<'a>,
}

impl askama::filters::HtmlSafe for InlineItem<'_> {}

impl<'a> InlineItem<'a> {
    /// Item `number`, titled `title`, in the repo `repo`.
    pub(super) fn of(number: Number, title: &'a str, repo: &'a str) -> InlineItem<'a> {
        let title = ItemTitle {
            number,
            title,
            cut: Cut::Wrap,
        };
        InlineItem {
            number: ItemNumberLink::after(&title, repo, Weight::Inherit),
            title,
        }
    }
}

/// An item's number as a card's line names it (`docs/specs/board.md`,
/// Cards, Its line): a daisyUI `link`, `link-hover`, to the item on
/// GitHub, in the repo `repo`. A pull request's has Octicons'
/// `git-pull-request` `gap-1` before it, the two in one `inline-flex` whose
/// baseline is the number's, so that it sits on the line's.
#[derive(Template)]
#[template(
    source = r#"{% if kind == ItemKind::PullRequest %}<span class="inline-flex items-baseline gap-1">{{ crate::icons::pull_request_svg_in_flex() }}{% endif %}<a class="link link-hover" href="{{ self::github(repo, number) }}">{{ number }}</a>{% if kind == ItemKind::PullRequest %}</span>{% endif %}"#,
    ext = "html"
)]
pub(super) struct CardLineNumber<'a> {
    pub(super) number: Number,
    pub(super) repo: &'a str,
    pub(super) kind: ItemKind,
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

    /// `docs/specs/board.md`, Cards, Its number, and Item, Related items.
    #[test_log::test]
    fn a_number_is_secondary_text_linking_to_github_in_its_weight() {
        let title = ItemTitle {
            number: Number(3),
            title: "Name the columns",
            cut: Cut::ToFit,
        };
        let number =
            |weight| html::rendered(&ItemNumberLink::after(&title, REPO, weight)).to_string();
        assert_eq!(
            number(Weight::Normal),
            r#"<a class="link link-hover shrink-0 font-normal text-base-content/60" href="https://github.com/owner/widgets/issues/3">#3</a>"#
        );
        assert_eq!(
            number(Weight::Inherit),
            r#"<a class="link link-hover shrink-0 text-base-content/60" href="https://github.com/owner/widgets/issues/3">#3</a>"#
        );
        assert_eq!(
            html::rendered(&ItemNumberLink::alone(Number(3), REPO)).to_string(),
            number(Weight::Inherit),
            "a number alone is in the weight of the text around it"
        );
    }

    /// `docs/specs/board.md`, Item, Heading, and `docs/specs/runs.md`,
    /// Run, The heading's item.
    #[test_log::test]
    fn an_item_as_one_text_is_its_title_a_space_and_its_number_in_a_plain_span() {
        let item = html::rendered(&TextItem::of(
            Number(3),
            "Name the columns",
            REPO,
            Weight::Normal,
        ))
        .to_string();
        assert_eq!(
            item,
            r#"<span><a class="link link-hover" href="/items/3">Name the columns</a> <a class="link link-hover shrink-0 font-normal text-base-content/60" href="https://github.com/owner/widgets/issues/3">#3</a></span>"#
        );
    }

    /// `docs/specs/board.md`, Cards, Its line.
    #[test_log::test]
    fn a_card_lines_number_links_to_github_with_a_pull_requests_icon_gap_1_before_it() {
        let link =
            r#"<a class="link link-hover" href="https://github.com/owner/widgets/issues/4">#4</a>"#;
        let issue = html::rendered(&CardLineNumber {
            number: Number(4),
            repo: REPO,
            kind: ItemKind::Issue,
        })
        .to_string();
        assert_eq!(issue, link);
        let pull = html::rendered(&CardLineNumber {
            number: Number(4),
            repo: REPO,
            kind: ItemKind::PullRequest,
        })
        .to_string();
        assert!(
            pull.starts_with(
                r#"<span class="inline-flex items-baseline gap-1"><svg class="icon size-[1em] inline-block align-[calc(0.5cap-0.5em)] shrink-0 self-center" viewBox="0 0 16 16" fill="currentColor" role="img" aria-label="pull request" data-icon="git-pull-request">"#
            ),
            "{pull}"
        );
        assert!(pull.ends_with(&format!("</svg>{link}</span>")), "{pull}");
    }
}
