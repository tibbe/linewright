//! A table's rows 25 at a time (`docs/specs/ui_patterns.md`, Tables,
//! Pages), with the nav under the table that the `pages` macro draws.

use std::num::{IntErrorKind, NonZeroUsize};
use std::ops::Range;

use serde::{Deserialize, Deserializer};

/// How many of a table's own rows a page shows.
const PER_PAGE: usize = 25;

/// The page a link asks for. As a query parameter, it reads as `of` reads
/// it, and a missing one is 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Requested(NonZeroUsize);

impl Requested {
    /// The page of the query parameter `field`. A missing, non-numeric or
    /// zero page is 1, and one too large to count is past the last.
    pub(super) fn of(field: Option<&str>) -> Requested {
        let Some(field) = field else {
            return Requested(NonZeroUsize::MIN);
        };
        Requested(match field.trim().parse::<NonZeroUsize>() {
            Ok(page) => page,
            Err(e) if *e.kind() == IntErrorKind::PosOverflow => NonZeroUsize::MAX,
            Err(_) => NonZeroUsize::MIN,
        })
    }

    /// The page as its query parameter carries it, empty for page 1 so that
    /// the parameter is left out.
    pub(super) fn field(self) -> String {
        if self.0 == NonZeroUsize::MIN {
            String::new()
        } else {
            self.0.to_string()
        }
    }
}

/// A page of a table, at most the last.
#[derive(Debug)]
pub(super) struct Pages {
    total: usize,
    current: NonZeroUsize,
}

/// Where a page link leads: the page's address and its stream's.
#[derive(Debug, PartialEq)]
pub(super) struct PageHref {
    pub(super) page: String,
    pub(super) stream: String,
}

impl PageHref {
    /// To the page at `page`, a path and its query, and its stream at the
    /// same under `/events`.
    pub(super) fn at(page: String) -> PageHref {
        PageHref {
            stream: format!("/events{page}"),
            page,
        }
    }
}

/// The nav under a table with more rows than a page shows.
#[derive(Debug, PartialEq)]
pub(super) struct PagesShown {
    /// The nav's accessible name, such as "Pages of runs".
    pub(super) label: String,
    /// Which rows show, of how many, such as "Showing 26–50 of 248 runs".
    pub(super) showing: String,
    /// `None` on the first page, where "Previous" is disabled.
    pub(super) previous: Option<PageHref>,
    /// The first page, the last and the two on each side of the current,
    /// with a gap in place of the pages between them.
    pub(super) items: Vec<PageItem>,
    /// `None` on the last page, where "Next" is disabled.
    pub(super) next: Option<PageHref>,
}

#[derive(Debug, PartialEq)]
pub(super) enum PageItem {
    Page {
        number: NonZeroUsize,
        href: PageHref,
        current: bool,
    },
    /// "…", in place of the pages between two numbers.
    Gap,
}

impl Pages {
    /// The `requested` page of a table of `total` rows, or its last page
    /// where the requested one does not exist.
    pub(super) fn of(total: usize, requested: Requested) -> Pages {
        Pages {
            total,
            current: requested.0.min(last_of(total)),
        }
    }

    /// The indexes of the table's own rows that the page shows.
    pub(super) fn range(&self) -> Range<usize> {
        let start = (self.current.get() - 1) * PER_PAGE;
        start..(start + PER_PAGE).min(self.total)
    }

    /// Of `rows`, all of the table's own rows, those that the page shows.
    pub(super) fn shows<T>(&self, rows: Vec<T>) -> Vec<T> {
        let range = self.range();
        rows.into_iter()
            .skip(range.start)
            .take(range.len())
            .collect()
    }

    /// The nav under the table, `None` while the table has `PER_PAGE` rows
    /// or fewer. `noun` is what the rows are, such as "runs", and `link`
    /// where the link to a page number leads: the screen's query with only
    /// this table's parameter changed.
    pub(super) fn shown(
        &self,
        noun: &str,
        link: impl Fn(Requested) -> PageHref,
    ) -> Option<PagesShown> {
        if self.total <= PER_PAGE {
            return None;
        }
        let last = last_of(self.total);
        let shown = |number: &NonZeroUsize| {
            *number == NonZeroUsize::MIN
                || *number == last
                || number.get().abs_diff(self.current.get()) <= 2
        };
        let mut items = vec![];
        let mut last_shown = 0;
        for number in (1..=last.get()).filter_map(NonZeroUsize::new).filter(shown) {
            if number.get() > last_shown + 1 {
                items.push(PageItem::Gap);
            }
            items.push(PageItem::Page {
                number,
                href: link(Requested(number)),
                current: number == self.current,
            });
            last_shown = number.get();
        }
        let next = self.current.checked_add(1).filter(|next| *next <= last);
        let range = self.range();
        Some(PagesShown {
            label: format!("Pages of {noun}"),
            showing: format!(
                "Showing {}–{} of {} {noun}",
                range.start + 1,
                range.end,
                self.total
            ),
            previous: NonZeroUsize::new(self.current.get() - 1)
                .map(|previous| link(Requested(previous))),
            items,
            next: next.map(|next| link(Requested(next))),
        })
    }
}

impl Default for Requested {
    fn default() -> Requested {
        Requested::of(None)
    }
}

impl<'de> Deserialize<'de> for Requested {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Requested, D::Error> {
        let field = String::deserialize(deserializer)?;
        Ok(Requested::of(Some(&field)))
    }
}

/// The last page of a table of `total` rows, the first where it has none.
fn last_of(total: usize) -> NonZeroUsize {
    NonZeroUsize::new(total.div_ceil(PER_PAGE)).unwrap_or(NonZeroUsize::MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requested(page: usize) -> Requested {
        Requested::of(Some(&page.to_string()))
    }

    fn href(page: Requested) -> PageHref {
        PageHref {
            page: format!("/runs?page={}", page.0),
            stream: format!("/events/runs?page={}", page.0),
        }
    }

    /// The nav of page `page` of `total` rows, its items as numbers, "*"
    /// marking the current page, and "…".
    fn window(total: usize, page: usize) -> Vec<String> {
        let shown = Pages::of(total, requested(page))
            .shown("runs", href)
            .expect("a nav");
        shown
            .items
            .iter()
            .map(|item| match item {
                PageItem::Page {
                    number,
                    href,
                    current,
                } => {
                    assert_eq!(href.page, format!("/runs?page={number}"));
                    format!("{number}{}", if *current { "*" } else { "" })
                }
                PageItem::Gap => "…".into(),
            })
            .collect()
    }

    /// `docs/specs/ui_patterns.md`, Tables, Pages.
    #[test_log::test]
    fn a_non_numeric_or_zero_page_is_the_first() {
        for field in [Some(""), Some("two"), Some("-3"), Some("0"), Some("1.5")] {
            assert_eq!(
                Pages::of(100, Requested::of(field)).range(),
                0..25,
                "{field:?}"
            );
        }
    }

    /// `docs/specs/ui_patterns.md`, Tables, Pages.
    #[test_log::test]
    fn a_page_shows_25_of_the_tables_rows() {
        assert_eq!(Pages::of(248, requested(2)).range(), 25..50);
        assert_eq!(Pages::of(3, requested(1)).range(), 0..3);
        assert_eq!(Pages::of(0, requested(1)).range(), 0..0);
    }

    /// `docs/specs/ui_patterns.md`, Tables, Pages, Updates.
    #[test_log::test]
    fn a_page_that_does_not_exist_shows_as_the_last() {
        assert_eq!(Pages::of(248, requested(11)).range(), 225..248);
        assert_eq!(Pages::of(248, requested(400)).range(), 225..248);
        assert_eq!(Pages::of(20, requested(3)).range(), 0..20);
        assert_eq!(Pages::of(0, requested(3)).range(), 0..0);
        assert_eq!(
            Pages::of(248, Requested::of(Some("99999999999999999999999"))).range(),
            225..248
        );
        assert_eq!(window(60, 9), ["1", "2", "3*"]);
    }

    /// `docs/specs/ui_patterns.md`, Tables, Pages.
    #[test_log::test]
    fn a_table_of_25_rows_or_fewer_shows_no_pages() {
        assert_eq!(Pages::of(0, requested(1)).shown("runs", href), None);
        assert_eq!(Pages::of(25, requested(1)).shown("runs", href), None);
        assert!(Pages::of(26, requested(1)).shown("runs", href).is_some());
    }

    /// `docs/specs/ui_patterns.md`, Tables, Pages, Under the table.
    #[test_log::test]
    fn the_nav_says_which_rows_show_of_how_many() {
        let showing = |total, page| {
            Pages::of(total, requested(page))
                .shown("runs", href)
                .unwrap()
                .showing
        };
        assert_eq!(showing(248, 2), "Showing 26–50 of 248 runs");
    }

    /// `docs/specs/ui_patterns.md`, Tables, Pages, Under the table.
    #[test_log::test]
    fn the_numbers_are_the_first_the_last_and_two_on_each_side_with_gaps_between() {
        assert_eq!(window(248, 1), ["1*", "2", "3", "…", "10"]);
        assert_eq!(window(248, 10), ["1", "…", "8", "9", "10*"]);
        assert_eq!(
            window(248, 5),
            ["1", "…", "3", "4", "5*", "6", "7", "…", "10"]
        );
        assert_eq!(window(248, 4), ["1", "2", "3", "4*", "5", "6", "…", "10"]);
        assert_eq!(window(248, 7), ["1", "…", "5", "6", "7*", "8", "9", "10"]);
        assert_eq!(window(248, 3), ["1", "2", "3*", "4", "5", "…", "10"]);
        assert_eq!(window(100, 2), ["1", "2*", "3", "4"]);
    }

    /// `docs/specs/ui_patterns.md`, Tables, Pages, Under the table.
    #[test_log::test]
    fn previous_and_next_lead_to_the_pages_either_side_of_the_page_shown() {
        let shown = |page| Pages::of(248, requested(page)).shown("runs", href).unwrap();
        let middle = shown(5);
        assert_eq!(middle.previous, Some(href(requested(4))));
        assert_eq!(middle.next, Some(href(requested(6))));
        let past_the_last = shown(12);
        assert_eq!(past_the_last.previous, Some(href(requested(9))));
        assert_eq!(past_the_last.next, None);
    }
}
