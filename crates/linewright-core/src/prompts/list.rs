//! A list Linewright fills into a prompt (docs/specs/runs.md, "Contract").

/// One or more items, each data in its form, such as `` `abc1234` `` or
/// `"done"`, or a filled part. It has no text of its own: it is rendered by
/// where its placeholder sits.
pub(super) struct List {
    first: String,
    rest: Vec<String>,
}

/// Where a list's placeholder sits.
#[derive(Clone, Copy)]
pub(super) enum Placement {
    /// Within a line of text.
    Inline,
    /// All of its line.
    Alone,
}

impl List {
    pub(super) fn new(first: String, rest: impl IntoIterator<Item = String>) -> List {
        List {
            first,
            rest: rest.into_iter().collect(),
        }
    }

    /// The `items` as a list, or `None` when there are none.
    pub(super) fn of(items: impl IntoIterator<Item = String>) -> Option<List> {
        let mut items = items.into_iter();
        let first = items.next()?;
        Some(List::new(first, items))
    }

    /// Inline, the items separated by ", ", as in "a, b, c". Alone, a
    /// `- item` line per item, an item's further lines indented under its
    /// bullet, with no line ending after the last.
    pub(super) fn render(&self, placement: Placement) -> String {
        let items = std::iter::once(&self.first).chain(&self.rest);
        match placement {
            Placement::Inline => items.map(String::as_str).collect::<Vec<_>>().join(", "),
            Placement::Alone => items
                .map(|item| {
                    let item = item.trim_end_matches(['\n', '\r']);
                    format!("- {}", item.replace('\n', "\n  "))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}
