use crate::domain::Number;

const TITLE_PREFIX: &str = "Spec change: ";

pub fn title(pr_title: &str) -> String {
    format!("{TITLE_PREFIX}{pr_title}")
}

/// A ticket Linewright filed.
pub struct FiledTicket {
    pub number: Number,
    pub title: String,
}

pub fn body(pr: Number, tickets: &[FiledTicket]) -> String {
    let list: String = tickets
        .iter()
        .map(|t| format!("- {} {}\n", t.number, t.title))
        .collect();
    format!("Tickets for the spec change in {pr}.\n\n{list}")
}

pub fn spec_change_of(title: &str, body: &str) -> Option<Number> {
    if !title.starts_with(TITLE_PREFIX) {
        return None;
    }
    let first = body.lines().next()?;
    let rest = first.strip_prefix("Tickets for the spec change in #")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().map(Number)
}
