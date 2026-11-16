//! A list Linewright fills into a prompt (docs/specs/runs.md, "Contract").

/// The word before a list's last item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Conjunction {
    And,
    Or,
}

/// `first` and the `rest` as one list: joined with ", ", or with "; " where
/// an item holds a comma, and the last joined by the `conjunction` alone, as
/// in "a, b and c" or "a, b; c, d or e, f".
pub(super) fn list(
    first: String,
    rest: impl IntoIterator<Item = String>,
    conjunction: Conjunction,
) -> String {
    let mut leading = Vec::new();
    let mut last = first;
    for item in rest {
        leading.push(std::mem::replace(&mut last, item));
    }
    if leading.is_empty() {
        return last;
    }
    let separator = if leading.iter().chain([&last]).any(|item| item.contains(',')) {
        "; "
    } else {
        ", "
    };
    let before_last = match conjunction {
        Conjunction::And => " and ",
        Conjunction::Or => " or ",
    };
    format!("{}{before_last}{last}", leading.join(separator))
}
