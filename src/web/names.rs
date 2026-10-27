//! How a page names items (`docs/specs/design_system.md`, Navigation): by
//! number, and by title where Linewright knows it.

use super::html::{self, Html};
use crate::domain::{Number, Snapshot};

/// The repo and the titles a page knows, which every phrase it draws names
/// its items with.
#[derive(Clone, Copy)]
pub(crate) struct Names<'a> {
    repo: &'a str,
    /// `None` before the first poll, and where a page names items by number
    /// alone.
    snapshot: Option<&'a Snapshot>,
}

impl<'a> Names<'a> {
    pub(crate) fn of(repo: &'a str, snapshot: Option<&'a Snapshot>) -> Names<'a> {
        Names { repo, snapshot }
    }

    /// #`number`, with its title where the snapshot holds it, linking to its
    /// page, and a link to it on GitHub.
    pub(crate) fn item(&self, number: Number) -> Html {
        match self.snapshot.and_then(|snapshot| snapshot.title(number)) {
            Some(title) => html::titled(number, title, self.repo),
            None => html::item(number, self.repo),
        }
    }

    /// The same, naming every item by its number alone.
    pub(crate) fn numbers_only(&self) -> Names<'a> {
        Names::of(self.repo, None)
    }

    pub(crate) fn repo(&self) -> &'a str {
        self.repo
    }
}
