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

pub fn filed_for(snapshot: &Snapshot, ticket: Number) -> Vec<&Issue> {
    snapshot
        .issues
        .iter()
        .filter(|i| i.open && i.filed == Some(Filed::SpecGap { ticket }))
        .collect()
}
