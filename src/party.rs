//! Who is party to an item (`GLOSSARY.md`, **Party**;
//! `docs/specs/design_system.md`, Filtering by person).

use std::collections::BTreeSet;

use crate::domain::{Login, Number, Snapshot};

/// The people party to #`number` or a pull request the snapshot holds that
/// closes it ([`Snapshot::pulls_on`]): those [`party_to`] each.
pub fn parties(snapshot: &Snapshot, linewright: &Login, number: Number) -> BTreeSet<Login> {
    std::iter::once(number)
        .chain(snapshot.pulls_on(number))
        .flat_map(|item| party_to(snapshot, linewright, item))
        .collect()
}

/// The people party to #`number` alone: those who opened it, are assigned to it or review it. An item Linewright
/// opened counts as opened by whoever opened what it was opened for,
/// followed back ([`Snapshot::opening`]). `linewright` is party to nothing.
pub fn party_to(snapshot: &Snapshot, linewright: &Login, number: Number) -> BTreeSet<Login> {
    let mut parties: BTreeSet<Login> = openers(snapshot, linewright, number).into_iter().collect();
    parties.extend(snapshot.roles(number).into_iter().cloned());
    parties.remove(linewright);
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
