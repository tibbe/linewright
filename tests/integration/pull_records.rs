use crate::common;

use common::*;
use linewright::domain::*;
use linewright::pull_records::PullRecords;

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

#[test_log::test]
fn with_no_record_a_ready_pull_request_of_linewrights_github_blocks_counts_as_commented_on_now_and_one_it_has_not_worked_out_is_returned()
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
    let mut records = PullRecords::default();
    let now: Timestamp = "2026-10-01T10:00:00Z".parse().unwrap();

    let unknown = records.take_shown(&snapshot, &linewright(), now, |pr| {
        pr != Number(6) && pr != Number(7)
    });

    assert_eq!(unknown, [Number(2)].into());
    let commented: Vec<(u64, Timestamp)> = (1..=7)
        .filter_map(|pr| Some((pr, records.last_commented(Number(pr))?)))
        .collect();
    assert_eq!(commented, [(1, now)]);
    assert!((1..=7).all(|pr| records.failed_merges(Number(pr)).is_empty()));
}
