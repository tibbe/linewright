//! Who is party to an item (`GLOSSARY.md`, **Party**;
//! `docs/specs/design_system.md`, Filtering by person).

use std::collections::BTreeSet;

use crate::domain::{Login, Number, Snapshot};

/// The people party to #`number`, sorted by login and each once: those who
/// opened it, are assigned to it or review it, or a pull request the
/// snapshot holds that closes it ([`Snapshot::pulls_on`]). An item
/// Linewright opened counts as opened by whoever opened what it was opened
/// for, followed back ([`Snapshot::opening`]). `linewright` is party to
/// nothing.
pub fn parties(snapshot: &Snapshot, linewright: &Login, number: Number) -> Vec<Login> {
    let mut parties: Vec<Login> = std::iter::once(number)
        .chain(snapshot.pulls_on(number))
        .flat_map(|item| {
            let mut parties = openers(snapshot, linewright, item);
            parties.extend(snapshot.roles(item).into_iter().cloned());
            parties
        })
        .filter(|login| login != linewright)
        .collect();
    parties.sort();
    parties.dedup();
    parties
}

/// Who opened #`number`: its opener, or where Linewright opened it, the
/// openers of what it was opened for, followed back to accounts other than
/// `linewright`.
fn openers(snapshot: &Snapshot, linewright: &Login, number: Number) -> Vec<Login> {
    let mut seen = BTreeSet::new();
    let mut to_follow = vec![number];
    let mut openers = vec![];
    while let Some(item) = to_follow.pop() {
        if !seen.insert(item) {
            continue;
        }
        let Some(opening) = snapshot.opening(item, linewright) else {
            continue;
        };
        if opening.author == *linewright {
            to_follow.extend(opening.opened_for);
        } else {
            openers.push(opening.author);
        }
    }
    openers
}
