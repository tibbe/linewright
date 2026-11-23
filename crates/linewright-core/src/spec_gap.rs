use crate::domain::{Filed, Issue, Number, Snapshot};

const FOOTER_PREFIX: &str = "Found while working on #";

pub fn body(gap: &str, ticket: Number) -> String {
    format!("{}\n\n{FOOTER_PREFIX}{}.", gap.trim(), ticket.0)
}

pub fn ticket_of(body: &str) -> Option<Number> {
    let last = body.trim_end().lines().last()?;
    let digits = last.strip_prefix(FOOTER_PREFIX)?.strip_suffix('.')?;
    digits.parse().ok().map(Number)
}

/// An item and the spec gaps filed for it that are still open.
pub struct FiledGaps<'a> {
    pub item: Number,
    pub issues: &'a [&'a Issue],
}

/// The gap in `filed` titled `title`: a session told which gaps are filed
/// may still return one, so titles are compared.
pub fn filed_as<'a>(filed: &FiledGaps<'a>, title: &str) -> Option<&'a Issue> {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    filed
        .issues
        .iter()
        .copied()
        .find(|issue| same(&issue.title, title))
}

pub fn filed_for(snapshot: &Snapshot, ticket: Number) -> Vec<&Issue> {
    snapshot
        .issues
        .iter()
        .filter(|i| i.open && i.filed == Some(Filed::SpecGap { ticket }))
        .collect()
}
