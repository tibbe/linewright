//! A list Linewright fills into a prompt (docs/specs/runs.md, "Contract").

/// The word before a list's last item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Conjunction {
    And,
    Or,
}

/// One or more items, each data in its form, such as `` `abc1234` `` or
/// `"done"`, or a filled part. It has no text of its own: it is rendered by
/// where its placeholder sits.
pub(super) struct List {
    first: String,
    rest: Vec<String>,
    conjunction: Conjunction,
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
    pub(super) fn new(
        first: String,
        rest: impl IntoIterator<Item = String>,
        conjunction: Conjunction,
    ) -> List {
        List {
            first,
            rest: rest.into_iter().collect(),
            conjunction,
        }
    }

    /// The `items` as a list, or `None` when there are none.
    pub(super) fn of(
        items: impl IntoIterator<Item = String>,
        conjunction: Conjunction,
    ) -> Option<List> {
        let mut items = items.into_iter();
        let first = items.next()?;
        Some(List::new(first, items, conjunction))
    }

    /// Inline, the items joined with ", ", or with "; " where an item holds
    /// a comma, and the last joined by the conjunction alone, as in "a, b and
    /// c" or "a, b; c, d or e, f". Alone, a `- item` line per item, an
    /// item's further lines indented under its bullet, with no line ending
    /// after the last.
    pub(super) fn render(&self, placement: Placement) -> String {
        let items: Vec<&str> = std::iter::once(&self.first)
            .chain(&self.rest)
            .map(String::as_str)
            .collect();
        match placement {
            Placement::Inline => match items.split_last() {
                Some((last, leading)) if !leading.is_empty() => {
                    let separator = if items.iter().any(|item| item.contains(',')) {
                        "; "
                    } else {
                        ", "
                    };
                    let before_last = match self.conjunction {
                        Conjunction::And => " and ",
                        Conjunction::Or => " or ",
                    };
                    format!("{}{before_last}{last}", leading.join(separator))
                }
                _ => self.first.clone(),
            },
            Placement::Alone => items
                .iter()
                .map(|item| {
                    let item = item.trim_end_matches(['\n', '\r']);
                    format!("- {}", item.replace('\n', "\n  "))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}
