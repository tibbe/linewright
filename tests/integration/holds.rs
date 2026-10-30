use crate::common;

use common::*;
use linewright::domain::*;
use linewright::holds::Holds;

fn requiring_a_code_owner() -> Vec<Requirement> {
    vec![Requirement::CodeOwnerReview {
        owners: vec!["alice".into()],
    }]
}

/// Linewright's pull request #`number`, ready for review and `mergeability`.
fn ready(number: u64, mergeability: Mergeability) -> Pull {
    Pull {
        mergeability,
        ..open_pull(number, LINEWRIGHT, &[], &[])
    }
}

#[test]
fn a_ready_pull_request_of_linewrights_github_blocks_is_taken_as_held_and_one_it_has_not_worked_out_is_returned()
 {
    let blocked = || Mergeability::Blocked {
        requires: requiring_a_code_owner(),
    };
    let snapshot = Snapshot {
        pulls: vec![
            ready(1, blocked()),
            ready(2, Mergeability::Unknown),
            ready(3, Mergeability::Clean),
            Pull {
                state: PullState::Open { draft: true },
                ..ready(4, blocked())
            },
            Pull {
                author: Login::new("alice"),
                ..ready(5, blocked())
            },
            ready(6, blocked()),
            ready(7, Mergeability::Unknown),
        ],
        ..Snapshot::default()
    };
    let mut holds = Holds::default();

    let unknown = holds.take_shown(&snapshot, &linewright(), |pr| {
        pr != Number(6) && pr != Number(7)
    });

    assert_eq!(unknown, [Number(2)].into());
    let held: Vec<u64> = (1..=7)
        .filter(|&pr| holds.is_held_for(Number(pr), &requiring_a_code_owner()))
        .collect();
    assert_eq!(held, [1]);
}
