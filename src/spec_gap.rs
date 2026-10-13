//! A spec gap that did not block its ticket (`docs/specs/implementation.md`,
//! "Runs"): an issue whose body the manager writes and a store reads back
//! ([`crate::domain::Filed`]), so later sessions on the ticket see the gaps
//! already filed and don't file them again.

use crate::domain::{Filed, Issue, Number, Snapshot};

const FOOTER_PREFIX: &str = "Found while working on #";

pub fn body(gap: &str, ticket: Number) -> String {
    format!("{}\n\n{FOOTER_PREFIX}{}.", gap.trim(), ticket.0)
}

/// The ticket a spec gap with this body was found on.
pub fn ticket_of(body: &str) -> Option<Number> {
    let last = body.trim_end().lines().last()?;
    let digits = last.strip_prefix(FOOTER_PREFIX)?.strip_suffix('.')?;
    digits.parse().ok().map(Number)
}

/// The open spec gaps filed while working on the ticket.
pub fn filed_for(snapshot: &Snapshot, ticket: Number) -> Vec<&Issue> {
    snapshot
        .issues
        .iter()
        .filter(|i| i.open && i.filed == Some(Filed::SpecGap { ticket }))
        .collect()
}
