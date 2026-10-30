use linewright::apply::triage::*;
use linewright::apply::{Asks, StatusTo};
use linewright::domain::{Issue, Login, StatusLabel};
use linewright::results::{TriageOutcome, TriageResult};
use linewright::store::Categories;
use rstest::rstest;

use crate::common::{LINEWRIGHT, linewright, logins, ticket};

fn issue(author: &str, labels: &[&str]) -> Issue {
    Issue {
        author: Login::new(author),
        labels: labels.iter().map(ToString::to_string).collect(),
        assignees: vec![linewright()],
        ..ticket(7, "Crash")
    }
}

fn needs_info(ask: &[&str]) -> TriageOutcome {
    TriageOutcome::NeedsInfo { ask: logins(ask) }
}

fn result(outcome: TriageOutcome, category: &str, comment: &str) -> TriageResult {
    TriageResult {
        outcome,
        category: category.into(),
        comment: comment.into(),
    }
}

fn asks(status: StatusLabel, unassign: bool) -> Asks {
    Asks {
        status: Some(StatusTo::Set(status)),
        unassign,
        ..Asks::default()
    }
}

#[rstest]
#[case::ready_for_agent_keeps_linewright_on_it(
    "reporter",
    TriageOutcome::ReadyForAgent,
    "## Agent Brief\n\n**Summary:** Empty widgets crash.",
    &[],
    "## Agent Brief\n\n**Summary:** Empty widgets crash.",
    asks(StatusLabel::ReadyForAgent, false)
)]
#[case::needs_info_assigns_and_names_the_people_asked(
    "reporter",
    needs_info(&["tester", "outsider"]),
    "Which version, @tester?",
    &["tester", "outsider"],
    "@outsider\n\nWhich version, @tester?",
    asks(StatusLabel::NeedsInfo, true)
)]
#[case::needs_info_naming_no_one_asks_the_reporter(
    "reporter",
    needs_info(&[]),
    "Which version?",
    &["reporter"],
    "@reporter\n\nWhich version?",
    asks(StatusLabel::NeedsInfo, true)
)]
#[case::a_mention_in_another_case_names_the_person_asked(
    "reporter",
    needs_info(&["tester"]),
    "Which version, @Tester?",
    &["tester"],
    "Which version, @Tester?",
    asks(StatusLabel::NeedsInfo, true)
)]
#[case::linewright_never_asks_itself(
    "reporter",
    needs_info(&[LINEWRIGHT]),
    "Which version?",
    &["reporter"],
    "@reporter\n\nWhich version?",
    asks(StatusLabel::NeedsInfo, true)
)]
#[case::needs_info_on_an_issue_linewright_filed_naming_no_one_asks_no_one(
    LINEWRIGHT,
    needs_info(&[]),
    "Which version?",
    &[],
    "Which version?",
    asks(StatusLabel::NeedsInfo, true)
)]
#[case::ready_for_human_says_a_label_answers_it(
    "reporter",
    TriageOutcome::ReadyForHuman,
    "Should widgets be resizable?",
    &[],
    "Should widgets be resizable?\n\n\
     Answer with a label: `ready-for-agent` to implement it as decided or \
     `needs-triage` to triage it again. A comment alone starts nothing.",
    asks(StatusLabel::ReadyForHuman, true)
)]
fn a_triage_asks_comments_and_hands_on_by_its_outcome(
    #[case] author: &str,
    #[case] outcome: TriageOutcome,
    #[case] comment: &str,
    #[case] assign: &[&str],
    #[case] expected_comment: &str,
    #[case] expected_asks: Asks,
) {
    let triaged = triage(
        &issue(author, &["needs-triage"]),
        &result(outcome, "bug", comment),
        &Categories::Labels,
        &linewright(),
    );

    assert_eq!(
        triaged,
        Triage {
            assign: logins(assign),
            comment: expected_comment.into(),
            category: CategoryChange::Labels {
                remove: vec![],
                add: "bug".into(),
            },
            asks: expected_asks,
        }
    );
}

#[rstest]
#[case::a_label_is_added(
    &["needs-triage"],
    Categories::Labels,
    "bug",
    CategoryChange::Labels { remove: vec![], add: "bug".into() }
)]
#[case::the_other_category_label_is_removed(
    &["needs-triage", "enhancement"],
    Categories::Labels,
    "bug",
    CategoryChange::Labels { remove: vec!["enhancement"], add: "bug".into() }
)]
#[case::the_same_category_label_is_added_again(
    &["needs-triage", "bug"],
    Categories::Labels,
    "bug",
    CategoryChange::Labels { remove: vec![], add: "bug".into() }
)]
#[case::an_issue_type_is_set(
    &["needs-triage", "enhancement"],
    Categories::IssueTypes(vec!["Bug".into(), "Feature".into()]),
    "Bug",
    CategoryChange::IssueType("Bug".into())
)]
fn a_triage_categorizes_by_the_repositorys_scheme(
    #[case] labels: &[&str],
    #[case] categories: Categories,
    #[case] category: &str,
    #[case] expected: CategoryChange,
) {
    let triaged = triage(
        &issue("reporter", labels),
        &result(TriageOutcome::ReadyForAgent, category, "Triaged."),
        &categories,
        &linewright(),
    );

    assert_eq!(triaged.category, expected);
}
