//! The tracking issue of a spec change (`docs/specs/implementation.md`,
//! "Tickets"): its title and body, which the manager writes and a store reads
//! back ([`crate::domain::Filed`]) to find the spec change it belongs to.

use crate::domain::Number;

const TITLE_PREFIX: &str = "Spec change: ";

pub fn title(pr_title: &str) -> String {
    format!("{TITLE_PREFIX}{pr_title}")
}

pub fn body(pr: Number, tickets: &[(Number, String)]) -> String {
    let list: String = tickets
        .iter()
        .map(|(number, title)| format!("- {number} {title}\n"))
        .collect();
    format!("Tickets for the spec change in {pr}.\n\n{list}")
}

/// The spec change a tracking issue with this title and body tracks.
pub fn spec_change_of(title: &str, body: &str) -> Option<Number> {
    if !title.starts_with(TITLE_PREFIX) {
        return None;
    }
    let first = body.lines().next()?;
    let rest = first.strip_prefix("Tickets for the spec change in #")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().map(Number)
}
