use linewright::apply::filing::*;
use linewright::apply::{Asks, Designed, StatusTo};
use linewright::automation::{self, Automation};
use linewright::domain::{Number, StatusLabel};
use linewright::results::{
    AutomationIssue, AutomationIssueLabel, AutomationResult, ImplementResult, IssueWork, SpecGap,
    Tickets,
};
use linewright::spec_gap::FiledGaps;
use linewright::store::{NewIssue, NewPull};
use linewright::tracking::FiledTicket;
use rstest::rstest;
use serde_json::json;

use crate::common::{merged_pull, ticket};

/// Tickets titled `title`, each blocked by the earlier ones `blocked_by`
/// gives by position.
fn tickets(drafts: &[(&str, Vec<usize>)]) -> Tickets {
    let id = |i: usize| format!("t{i}");
    serde_json::from_value(json!(
        drafts
            .iter()
            .enumerate()
            .map(|(i, (title, blocked_by))| json!({
                "id": id(i),
                "title": title,
                "body": format!("Build the {title}."),
                "blocked_by": blocked_by.iter().map(|&b| id(b)).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()
    ))
    .unwrap()
}

fn new_issue(title: &str, body: &str, labels: &[&str]) -> NewIssue {
    NewIssue {
        title: title.into(),
        body: body.into(),
        labels: labels.iter().map(ToString::to_string).collect(),
    }
}

fn new_ticket(title: &str, labels: &[&str], blocked_by: &[usize]) -> NewTicket {
    NewTicket {
        issue: new_issue(title, &format!("Build the {title}."), labels),
        blocked_by: blocked_by.to_vec(),
    }
}

fn filed(tickets: &[(u64, &str)]) -> Vec<FiledTicket> {
    tickets
        .iter()
        .map(|&(number, title)| FiledTicket {
            number: Number(number),
            title: title.into(),
        })
        .collect()
}

fn gap(title: &str) -> SpecGap {
    SpecGap {
        title: title.into(),
        body: "Say what an empty widget shows.".into(),
    }
}

#[rstest]
#[case::an_issue_built_as_it_is_is_designed_and_stays_assigned(
    IssueWork::AsItIs,
    Asks {
        designed: Some(Designed::Add),
        ..Asks::default()
    }
)]
#[case::an_issue_cut_into_tickets_is_their_tracking_issue_with_no_status_or_linewright(
    IssueWork::Cut(serde_json::from_value(json!({
        "tickets": [{"id": "a", "title": "Store", "body": "Build it."}],
        "comment": "Small steps.",
    })).unwrap()),
    Asks {
        designed: Some(Designed::Add),
        status: Some(StatusTo::Clear),
        unassign: true,
        ..Asks::default()
    }
)]
fn a_designed_issue_asks(#[case] cut: IssueWork, #[case] expected: Asks) {
    assert_eq!(designed_asks(&cut), expected);
}

#[test]
fn a_designed_spec_change_is_labeled_designed_and_let_go_of() {
    assert_eq!(
        spec_change_designed(),
        Asks {
            designed: Some(Designed::Add),
            unassign: true,
            ..Asks::default()
        }
    );
}

#[rstest]
#[case::a_spec_change_gets_a_tracking_issue_titled_for_it(
    true,
    None,
    Some(TrackingIssue {
        issue: new_issue(
            "Spec change: Merged 5",
            "Tickets for the spec change in #5.\n\n",
            &[]
        ),
        spec_change: Number(5),
    })
)]
#[case::a_spec_changes_tracking_issue_carries_the_required_label(
    true,
    Some("from-linewright"),
    Some(TrackingIssue {
        issue: new_issue(
            "Spec change: Merged 5",
            "Tickets for the spec change in #5.\n\n",
            &["from-linewright"]
        ),
        spec_change: Number(5),
    })
)]
#[case::an_issue_is_its_own_tracking_issue(false, Some("from-linewright"), None)]
fn a_cut_files_a_tracking_issue_for_a_spec_change(
    #[case] spec_change: bool,
    #[case] required: Option<&str>,
    #[case] expected: Option<TrackingIssue>,
) {
    let pr = merged_pull(5, "maintainer", &[]);
    let under = if spec_change {
        CutUnder::SpecChange(&pr)
    } else {
        CutUnder::Issue
    };

    let cut = cut(&tickets(&[("store", vec![])]), under, required);

    assert_eq!(cut.tracking_issue, expected);
}

#[rstest]
#[case::one_ticket_with_no_blockers(
    vec![("store", vec![])],
    None,
    vec![new_ticket("store", &[], &[])]
)]
#[case::a_ticket_blocked_by_an_earlier_one(
    vec![("store", vec![]), ("listing", vec![0])],
    None,
    vec![new_ticket("store", &[], &[]), new_ticket("listing", &[], &[0])]
)]
#[case::a_ticket_blocked_by_two_earlier_ones(
    vec![("store", vec![]), ("api", vec![]), ("listing", vec![0, 1])],
    None,
    vec![
        new_ticket("store", &[], &[]),
        new_ticket("api", &[], &[]),
        new_ticket("listing", &[], &[0, 1]),
    ]
)]
#[case::every_ticket_carries_the_required_label(
    vec![("store", vec![]), ("listing", vec![0])],
    Some("from-linewright"),
    vec![
        new_ticket("store", &["from-linewright"], &[]),
        new_ticket("listing", &["from-linewright"], &[0]),
    ]
)]
fn a_cut_files_each_ticket_with_its_blockers(
    #[case] drafts: Vec<(&str, Vec<usize>)>,
    #[case] required: Option<&str>,
    #[case] expected: Vec<NewTicket>,
) {
    let cut = cut(&tickets(&drafts), CutUnder::Issue, required);

    assert_eq!(cut.tickets, expected);
}

#[test]
fn a_spec_changes_cut_files_the_same_tickets_as_an_issues() {
    let pr = merged_pull(5, "maintainer", &[]);
    let drafts = tickets(&[("store", vec![]), ("listing", vec![0])]);

    let cut = cut(&drafts, CutUnder::SpecChange(&pr), None);

    assert_eq!(
        cut.tickets,
        vec![
            new_ticket("store", &[], &[]),
            new_ticket("listing", &[], &[0])
        ]
    );
}

#[rstest]
#[case::an_issues_cut_has_no_tracking_body_or_spec_change_comment(
    &[(11, "Store"), (12, "Listing")],
    None,
    Published {
        tracking_body: None,
        design_comment: "A gadget store.\n\n## Tickets\n\n1. #11 Store\n2. #12 Listing\n\nSmall steps."
            .into(),
        ticket_asks: Asks {
            designed: Some(Designed::Add),
            status: Some(StatusTo::Set(StatusLabel::ReadyForAgent)),
            ..Asks::default()
        },
        spec_change_comment: None,
    }
)]
#[case::a_spec_changes_cut_lists_the_tickets_on_the_tracking_issue_and_the_spec_change(
    &[(11, "Store"), (12, "Listing")],
    Some(Number(5)),
    Published {
        tracking_body: Some(
            "Tickets for the spec change in #5.\n\n- #11 Store\n- #12 Listing\n".into()
        ),
        design_comment: "A gadget store.\n\n## Tickets\n\n1. #11 Store\n2. #12 Listing\n\nSmall steps."
            .into(),
        ticket_asks: Asks {
            designed: Some(Designed::Add),
            status: Some(StatusTo::Set(StatusLabel::ReadyForAgent)),
            ..Asks::default()
        },
        spec_change_comment: Some(Comment {
            on: Number(5),
            body: "Small steps.\n\nTracked in #10:\n\n1. #11 Store\n2. #12 Listing\n".into(),
        }),
    }
)]
#[case::one_ticket(
    &[(11, "Store")],
    Some(Number(5)),
    Published {
        tracking_body: Some("Tickets for the spec change in #5.\n\n- #11 Store\n".into()),
        design_comment: "A gadget store.\n\n## Tickets\n\n1. #11 Store\n\nSmall steps.".into(),
        ticket_asks: Asks {
            designed: Some(Designed::Add),
            status: Some(StatusTo::Set(StatusLabel::ReadyForAgent)),
            ..Asks::default()
        },
        spec_change_comment: Some(Comment {
            on: Number(5),
            body: "Small steps.\n\nTracked in #10:\n\n1. #11 Store\n".into(),
        }),
    }
)]
fn a_cut_is_published_with_its_filed_tickets_in_order(
    #[case] tickets: &[(u64, &str)],
    #[case] spec_change: Option<Number>,
    #[case] expected: Published,
) {
    let published = published(
        "A gadget store.",
        "Small steps.",
        Number(10),
        &filed(tickets),
        spec_change,
    );

    assert_eq!(published, expected);
}

#[rstest]
#[case::no_gaps_file_no_issues(&[], None, vec![])]
#[case::a_gap_is_filed_for_a_human_naming_the_item(
    &["Empty widgets"],
    None,
    vec![new_issue(
        "Empty widgets",
        "Say what an empty widget shows.\n\nFound while working on #7.",
        &["ready-for-human"]
    )]
)]
#[case::each_gap_carries_the_required_label(
    &["Empty widgets", "Huge widgets"],
    Some("from-linewright"),
    vec![
        new_issue(
            "Empty widgets",
            "Say what an empty widget shows.\n\nFound while working on #7.",
            &["ready-for-human", "from-linewright"]
        ),
        new_issue(
            "Huge widgets",
            "Say what an empty widget shows.\n\nFound while working on #7.",
            &["ready-for-human", "from-linewright"]
        ),
    ]
)]
fn spec_gaps_are_filed_for_the_item(
    #[case] titles: &[&str],
    #[case] required: Option<&str>,
    #[case] expected: Vec<NewIssue>,
) {
    let gaps: Vec<SpecGap> = titles.iter().map(|t| gap(t)).collect();

    assert_eq!(spec_gap_issues(&gaps, Number(7), required), expected);
}

#[rstest]
#[case::a_gap_not_filed_is_kept(&["Huge widgets"], &["Huge widgets"], &[])]
#[case::a_gap_filed_under_the_same_title_is_dropped(
    &["Empty widgets", "Huge widgets"],
    &["Huge widgets"],
    &[("Empty widgets", 20)]
)]
#[case::titles_are_compared_ignoring_case_and_surrounding_space(
    &[" empty WIDGETS "],
    &[],
    &[(" empty WIDGETS ", 20)]
)]
fn a_spec_gap_already_filed_is_not_filed_again(
    #[case] returned: &[&str],
    #[case] kept: &[&str],
    #[case] dropped: &[(&str, u64)],
) {
    let filed_issue = ticket(20, "Empty widgets");
    let issues = [&filed_issue];
    let filed = FiledGaps {
        item: Number(7),
        issues: &issues,
    };
    let mut result = ImplementResult::Blocked {
        question: "Which?".into(),
        spec_gaps: returned.iter().map(|t| gap(t)).collect(),
    };

    let already = unfiled_gaps(&mut result, &filed);

    assert_eq!(
        result,
        ImplementResult::Blocked {
            question: "Which?".into(),
            spec_gaps: kept.iter().map(|t| gap(t)).collect(),
        }
    );
    assert_eq!(
        already,
        dropped
            .iter()
            .map(|&(title, issue)| AlreadyFiled {
                gap: gap(title),
                issue: Number(issue),
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_result_without_spec_gaps_drops_none() {
    let issue = ticket(20, "Empty widgets");
    let issues = [&issue];
    let mut result = ImplementResult::GaveUp {
        reason: "Too hard.".into(),
    };

    let already = unfiled_gaps(
        &mut result,
        &FiledGaps {
            item: Number(7),
            issues: &issues,
        },
    );

    assert_eq!(already, vec![]);
}

#[rstest]
#[case::the_pull_request_closes_its_ticket("Does it.", "Does it.\n\nCloses #7")]
#[case::a_body_closing_the_ticket_is_kept("Does it.\n\nCloses #7", "Does it.\n\nCloses #7")]
#[case::a_body_closing_another_issue_closes_the_ticket_as_well(
    "Does it.\n\nFixes #12",
    "Does it.\n\nFixes #12\n\nCloses #7"
)]
fn an_implement_run_opens_a_pull_request_closing_its_ticket(
    #[case] body: &str,
    #[case] expected_body: &str,
) {
    assert_eq!(
        new_pull("7-do-it", "Do it".into(), body, Number(7)),
        NewPull {
            head: "7-do-it".into(),
            title: "Do it".into(),
            body: expected_body.into(),
        }
    );
}

fn weekly(max_issues: Option<u32>) -> Automation {
    let max = max_issues.map_or(String::new(), |n| format!("max-issues: {n}\n"));
    automation::parse(
        &"weekly".parse().unwrap(),
        &format!("---\non:\n  schedule: \"0 9 * * 1\"\n{max}---\nLook.\n"),
    )
    .unwrap()
}

fn found(title: &str, label: AutomationIssueLabel) -> AutomationIssue {
    AutomationIssue {
        title: title.into(),
        body: format!("{title} is broken."),
        label,
    }
}

fn found_issues(titles: &[&str]) -> AutomationResult {
    AutomationResult {
        summary: "Looked.".into(),
        issues: titles
            .iter()
            .map(|t| found(t, AutomationIssueLabel::ReadyForAgent))
            .collect(),
    }
}

#[rstest]
#[case::fewer_than_max_issues_are_all_kept(Some(3), &["A", "B"], &["A", "B"], &[])]
#[case::max_issues_are_all_kept(Some(2), &["A", "B"], &["A", "B"], &[])]
#[case::those_past_max_issues_are_dropped_in_order(Some(1), &["A", "B", "C"], &["A"], &["B", "C"])]
#[case::max_issues_zero_drops_every_one(Some(0), &["A"], &[], &["A"])]
#[case::max_issues_is_three_unless_set(None, &["A", "B", "C", "D"], &["A", "B", "C"], &["D"])]
fn an_automation_files_no_more_than_max_issues(
    #[case] max_issues: Option<u32>,
    #[case] returned: &[&str],
    #[case] kept: &[&str],
    #[case] dropped: &[&str],
) {
    let mut result = found_issues(returned);

    let past = past_max_issues(&weekly(max_issues), &mut result);

    assert_eq!(result, found_issues(kept));
    assert_eq!(past, found_issues(dropped).issues);
}

#[rstest]
#[case::an_issue_for_an_agent(
    AutomationIssueLabel::ReadyForAgent,
    None,
    &["ready-for-agent"]
)]
#[case::an_issue_for_a_human(
    AutomationIssueLabel::ReadyForHuman,
    None,
    &["ready-for-human"]
)]
#[case::an_issue_carries_the_required_label(
    AutomationIssueLabel::ReadyForAgent,
    Some("from-linewright"),
    &["ready-for-agent", "from-linewright"]
)]
fn an_automation_files_its_issues_saying_which_automation_filed_them(
    #[case] label: AutomationIssueLabel,
    #[case] required: Option<&str>,
    #[case] labels: &[&str],
) {
    let result = AutomationResult {
        summary: "Looked.".into(),
        issues: vec![found("Stale docs", label)],
    };

    assert_eq!(
        automation_issues(&weekly(None), &result, required),
        vec![new_issue(
            "Stale docs",
            "Stale docs is broken.\n\nFiled by the automation `weekly`.",
            labels
        )]
    );
}
