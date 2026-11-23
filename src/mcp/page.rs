//! How a tool that lists pages what it lists (`docs/specs/access.md`, The
//! MCP server, Lists).

use std::num::NonZeroUsize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many a list gives where its call names no `limit`.
const DEFAULT_LIMIT: usize = 100;

/// What a call of a tool that lists asks of the list.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub(super) struct Paging {
    /// How many to give at most: 100 without it.
    limit: Option<NonZeroUsize>,
    /// The `after` of the list that gave the ones before: gives those after
    /// it.
    after: Option<String>,
}

/// What a tool that lists gives: some of what it lists, in its screen's
/// order, with how many it left out after them and the `after` that gives
/// those.
#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub(super) struct Page<T> {
    pub(super) items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

impl Paging {
    /// The page of `items`, each found by `id`, that it asks for, or else
    /// why not: an `after` that names none of them.
    pub(super) fn page<T>(
        &self,
        items: Vec<T>,
        id: impl Fn(&T) -> String,
    ) -> Result<Page<T>, String> {
        let start = match &self.after {
            Some(after) => {
                1 + items
                    .iter()
                    .position(|item| id(item) == *after)
                    .ok_or_else(|| format!("Nothing listed has the ID {after}."))?
            }
            None => 0,
        };
        Ok(self.page_from(items, start, id))
    }

    /// As [`Paging::page`], where an `after` that names none of `items`,
    /// such as one gone since, gives those that `follows` it.
    pub(super) fn page_in_order<T>(
        &self,
        items: Vec<T>,
        id: impl Fn(&T) -> String,
        follows: impl Fn(&T, &str) -> bool,
    ) -> Page<T> {
        let start = match &self.after {
            Some(after) => items
                .iter()
                .position(|item| id(item) == *after)
                .map(|found| found + 1)
                .or_else(|| items.iter().position(|item| follows(item, after)))
                .unwrap_or(items.len()),
            None => 0,
        };
        self.page_from(items, start, id)
    }

    /// The page of `items` from `start` on.
    fn page_from<T>(&self, items: Vec<T>, start: usize, id: impl Fn(&T) -> String) -> Page<T> {
        let limit = self.limit.map_or(DEFAULT_LIMIT, NonZeroUsize::get);
        let mut items: Vec<T> = items.into_iter().skip(start).collect();
        let left_out = items.len().saturating_sub(limit);
        items.truncate(limit);
        let after = (left_out > 0).then(|| items.last().map(&id)).flatten();
        Page {
            items,
            left_out: (left_out > 0).then_some(left_out),
            after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paging(limit: Option<usize>, after: Option<&str>) -> Paging {
        Paging {
            limit: limit.map(|limit| NonZeroUsize::new(limit).unwrap()),
            after: after.map(ToString::to_string),
        }
    }

    fn page(of: usize, paging: &Paging) -> Result<Page<usize>, String> {
        paging.page((1..=of).collect(), ToString::to_string)
    }

    /// `docs/specs/access.md`, The MCP server, Lists.
    #[test_log::test]
    fn a_list_gives_the_first_100_and_says_how_many_it_left_out_and_after_which() {
        let first = page(250, &Paging::default()).unwrap();

        assert_eq!(first.items, (1..=100).collect::<Vec<_>>());
        assert_eq!(first.left_out, Some(150));
        assert_eq!(first.after.as_deref(), Some("100"));
        let next = page(250, &paging(None, Some("100"))).unwrap();
        assert_eq!(next.items, (101..=200).collect::<Vec<_>>());
        let last = page(250, &paging(None, Some("200"))).unwrap();
        assert_eq!(last.items, (201..=250).collect::<Vec<_>>());
        assert_eq!((last.left_out, last.after), (None, None));
    }

    #[test_log::test]
    fn a_limit_gives_that_many_and_a_list_that_fits_leaves_nothing_out() {
        assert_eq!(
            page(5, &paging(Some(2), Some("1"))).unwrap(),
            Page {
                items: vec![2, 3],
                left_out: Some(2),
                after: Some("3".into()),
            }
        );
        assert_eq!(
            page(3, &paging(Some(3), None)).unwrap(),
            Page {
                items: vec![1, 2, 3],
                left_out: None,
                after: None,
            }
        );
    }

    #[test_log::test]
    fn an_after_gone_from_a_list_in_order_gives_those_that_follow_it() {
        let follows = |item: &usize, after: &str| *item > after.parse().unwrap();
        let without_3: Vec<usize> = vec![1, 2, 4, 5];

        let next = paging(Some(1), Some("3")).page_in_order(
            without_3.clone(),
            ToString::to_string,
            follows,
        );
        let past = paging(None, Some("9")).page_in_order(without_3, ToString::to_string, follows);

        assert_eq!(next.items, [4]);
        assert_eq!((next.left_out, next.after.as_deref()), (Some(1), Some("4")));
        assert_eq!(past.items, Vec::<usize>::new());
    }

    #[test_log::test]
    fn an_after_that_names_nothing_listed_is_refused() {
        assert_eq!(
            page(3, &paging(None, Some("7"))),
            Err("Nothing listed has the ID 7.".to_string())
        );
    }
}
