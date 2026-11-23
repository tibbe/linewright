//! A page of one card in its middle, outside the shell
//! (`docs/specs/access.md`, Sign in), drawn by `card_page.html`.

use linewright_core::api::Sources;
use linewright_core::domain::Repo;

use super::HASHED;

/// What a card page shows around what its card holds.
pub(super) struct CardPage {
    /// Its title, before "· Linewright".
    pub(super) title: &'static str,
    /// The class of its card, naming the screen.
    pub(super) kind: &'static str,
    pub(super) heading: String,
    pub(super) css: &'static str,
    /// The repository, whose name is under the heading.
    pub(super) repo: Repo,
}

impl CardPage {
    /// The page `title`, its card of the class `kind` headed `heading`,
    /// for `sources`' repository.
    pub(super) fn new(
        title: &'static str,
        kind: &'static str,
        heading: impl Into<String>,
        sources: &Sources,
    ) -> CardPage {
        CardPage {
            title,
            kind,
            heading: heading.into(),
            css: HASHED.names["linewright.css"].as_str(),
            repo: sources.work_dir.repo.clone(),
        }
    }
}
