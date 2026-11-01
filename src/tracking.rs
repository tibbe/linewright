//! The title and body of a spec change's issue
//! (`docs/specs/sources_of_work.md`, "Spec changes").

use crate::domain::{Filed, Number};

const TITLE_PREFIX: &str = "Spec change: ";

/// How a spec change's issue's body begins, before the spec change's number.
const ISSUE_LINK: &str = "Opened for the spec change in ";

/// How the body of the tracking issue Linewright filed for a spec change's
/// tickets began, before a spec change had an issue of its own.
const TRACKING_LINK: &str = "Tickets for the spec change in ";

pub fn title(pr_title: &str) -> String {
    format!("{TITLE_PREFIX}{pr_title}")
}

/// A ticket Linewright filed.
pub struct FiledTicket {
    pub number: Number,
    pub title: String,
}

/// The body of the tracking issue of `tickets`, cut from spec change `pr`.
pub fn body(pr: Number, tickets: &[FiledTicket]) -> String {
    let list: String = tickets
        .iter()
        .map(|t| format!("- {} {}\n", t.number, t.title))
        .collect();
    format!("{TRACKING_LINK}{pr}.\n\n{list}")
}

/// What an issue so titled was filed as, by the link to a spec change its
/// body begins with.
pub fn filed_of(title: &str, body: &str) -> Option<Filed> {
    if !title.starts_with(TITLE_PREFIX) {
        return None;
    }
    let first = body.lines().next()?;
    if let Some(rest) = first.strip_prefix(ISSUE_LINK) {
        return linked(rest).map(|spec_change| Filed::SpecChange { spec_change });
    }
    linked(first.strip_prefix(TRACKING_LINK)?).map(|spec_change| Filed::Tracking { spec_change })
}

/// The number `#N` that `text` begins with.
fn linked(text: &str) -> Option<Number> {
    let digits: String = text
        .strip_prefix('#')?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok().map(Number)
}
