use crate::common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use linewright::domain::{Filed, Issue, Number, Parent, Snapshot};
use linewright::input::{self, Wanted};
use linewright::reconcile::Work;
use linewright::store::RecordedItem;
use serde_json::{Value, json};

/// Comments GitHub listed for tibbe/linewright#1130 on 2026-11-12, written
/// at 20:41:48, 20:42:19 and 21:25:33 on 2026-11-10.
const COMMENTS_1130: &str = include_str!("../../src/store/fixtures/rest-issue-1130-comments.json");

/// Ticket #7, filed under #3, which was opened for the spec change #2, with
/// the spec gaps #8, open, and #9, closed, filed for it.
fn ticket_cut_from_a_spec_change() -> Snapshot {
    let ticket = Issue {
        parent: Some(Parent::new(
            Number(3),
            Some(Filed::SpecChange {
                spec_change: Number(2),
            }),
        )),
        ..common::ticket(7, "Add gadgets")
    };
    let gap = |number: u64, open: bool| Issue {
        open,
        filed: Some(Filed::SpecGap { ticket: Number(7) }),
        ..common::ticket(number, "Should gadgets expire?")
    };
    snapshot_of(vec![ticket, gap(8, true), gap(9, false)])
}

fn numbers(numbers: &[u64]) -> BTreeSet<Number> {
    numbers.iter().copied().map(Number).collect()
}

#[test_log::test]
fn a_design_wants_the_item_its_parent_its_spec_change_and_its_open_spec_gaps_on_the_default_branch()
{
    let snapshot = ticket_cut_from_a_spec_change();

    let wanted = input::wanted(&snapshot, &Work::DesignIssue { issue: Number(7) }, "main");

    assert_eq!(
        wanted,
        Some(Wanted {
            items: numbers(&[2, 3, 7, 8]),
            branches: vec!["main".into()],
        })
    );
}

#[test_log::test]
fn an_issue_opened_for_a_spec_change_wants_its_own_spec_change() {
    let issue = Issue {
        filed: Some(Filed::SpecChange {
            spec_change: Number(4),
        }),
        ..common::ticket(5, "Spec change: Gadgets")
    };

    let wanted = input::wanted(
        &snapshot_of(vec![issue]),
        &Work::Triage { issue: Number(5) },
        "main",
    );

    assert_eq!(wanted.map(|w| w.items), Some(numbers(&[4, 5])));
}

#[test_log::test]
fn an_implement_run_wants_the_default_branch_and_its_tickets_branch() {
    let snapshot = ticket_cut_from_a_spec_change();

    let wanted = input::wanted(&snapshot, &Work::Implement { issue: Number(7) }, "main");

    assert_eq!(
        wanted.map(|w| w.branches),
        Some(vec!["main".into(), "7-add-gadgets".into()])
    );
}

#[test_log::test]
fn rework_fix_ci_and_update_want_no_input() {
    let snapshot = ticket_cut_from_a_spec_change();
    let (issue, pr) = (Number(7), Number(10));

    for work in [
        Work::Rework {
            issue,
            pr,
            update: false,
        },
        Work::FixCi {
            issue,
            pr,
            failing: vec!["test".into()],
        },
        Work::Update { issue, pr },
    ] {
        assert_eq!(input::wanted(&snapshot, &work, "main"), None, "{work:?}");
    }
}

#[test_log::test]
fn an_items_comments_are_those_written_at_or_before_the_start() {
    let comments: Value = serde_json::from_str(COMMENTS_1130).unwrap();
    let issue = json!({"number": 1130});
    let item = RecordedItem {
        number: Number(1130),
        resources: BTreeMap::from([
            ("issues/1130".to_string(), issue.clone()),
            ("issues/1130/comments".to_string(), comments.clone()),
        ]),
    };

    let cut = input::cut_comments_at(vec![item], at("2026-11-10T20:42:19Z"));

    assert_eq!(
        cut,
        [RecordedItem {
            number: Number(1130),
            resources: BTreeMap::from([
                ("issues/1130".to_string(), issue),
                (
                    "issues/1130/comments".to_string(),
                    Value::Array(comments.as_array().unwrap()[..2].to_vec()),
                ),
            ]),
        }]
    );
}
