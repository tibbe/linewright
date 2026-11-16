//! The follow-ups a session is sent back with, each reason filled from its
//! part in `briefs/parts/` (docs/specs/runs.md, "Contract").

use std::path::Path;

use serde_json::json;

use linewright::attachments::BadReference;
use linewright::briefs::Briefs;
use linewright::git;
use linewright::prompts::{self, LooseEnd, SentBack};
use linewright::results::{ContractError, Mismatch, Mismatches, ParseError};

fn result_error(reason: &SentBack) -> String {
    prompts::result_error(&Briefs::shipped().templates, reason)
}

fn loose_ends(problem: &LooseEnd) -> String {
    prompts::loose_ends(&Briefs::shipped().templates, problem)
}

/// What a session reads when its result can't be used for `reason`.
fn could_not_be_used(reason: &str) -> String {
    format!(
        "Your final message could not be used: {reason}\n\nAnswer again with only the result \
         JSON object."
    )
}

/// What a session reads when its work is not finished, for `problem`.
fn not_finished(problem: &str) -> String {
    format!(
        "Your work is not finished: {problem}\n\nFix that, then answer again with only the \
         result JSON object."
    )
}

fn parse_error(error: &ParseError) -> String {
    result_error(&SentBack::Result(error))
}

fn contract(error: ContractError) -> String {
    parse_error(&ParseError::Contract(error))
}

#[test_log::test]
fn a_final_message_with_no_json_object_is_sent_back_asking_for_one() {
    assert_eq!(
        parse_error(&ParseError::NoJson),
        could_not_be_used(
            "The final message holds no JSON object. End with the result as a single JSON \
             object."
        )
    );
}

#[test_log::test]
fn a_result_that_is_not_json_is_sent_back_with_serdes_message() {
    assert_eq!(
        parse_error(&ParseError::NotJson {
            error: "expected value at line 1 column 2".into()
        }),
        could_not_be_used("The result is not valid JSON: expected value at line 1 column 2.")
    );
}

#[test_log::test]
fn a_result_that_does_not_match_its_schema_is_sent_back_with_a_line_per_mismatch() {
    let outcomes = vec!["done".to_string(), "blocked".to_string()];
    let error = ParseError::Schema(Mismatches::new(
        Mismatch::Missing {
            key: "/question".into(),
        },
        vec![
            Mismatch::Outcome {
                outcome: Some(json!("finished")),
                outcomes: outcomes.clone(),
            },
            Mismatch::Outcome {
                outcome: None,
                outcomes,
            },
            Mismatch::At {
                at: "/issues/0/label".into(),
                error: r#""bug" is not one of "ready-for-agent" or "ready-for-human""#.into(),
            },
            Mismatch::Whole {
                error: r#"[] is not of type "object""#.into(),
            },
        ],
    ));

    assert_eq!(
        parse_error(&error),
        could_not_be_used(
            "The result does not match its schema:\n\n\
             - `/question` is missing\n\
             - `/outcome` is \"finished\"; it must be one of \"done\" or \"blocked\"\n\
             - `/outcome` is missing; it must be one of \"done\" or \"blocked\"\n\
             - `/issues/0/label`: \"bug\" is not one of \"ready-for-agent\" or \"ready-for-human\"\n\
             - the result: [] is not of type \"object\"\n"
        )
    );
}

#[test_log::test]
fn a_result_that_breaks_the_contract_is_sent_back_with_why() {
    for (error, why) in [
        (
            ContractError::Serde {
                error: "missing field `comment`".into(),
            },
            "missing field `comment`",
        ),
        (
            ContractError::Login {
                written: "the reporter".into(),
            },
            r#""the reporter" is not a GitHub login"#,
        ),
        (
            ContractError::DuplicateId { id: "a".into() },
            r#"two tickets have the id "a""#,
        ),
        (
            ContractError::UnknownBlocker {
                id: "a".into(),
                blocker: "b".into(),
            },
            r#"ticket "a" is blocked by "b", which is not a ticket id"#,
        ),
        (
            ContractError::Cycle,
            "the tickets' blocked_by edges form a cycle",
        ),
    ] {
        assert_eq!(
            contract(error),
            could_not_be_used(&format!(
                "The result JSON does not match the contract: {why}."
            ))
        );
    }
}

#[test_log::test]
fn a_category_outside_the_categories_is_sent_back_with_the_categories() {
    assert_eq!(
        parse_error(&ParseError::Category {
            category: "chore".into(),
            categories: vec!["bug".into(), "docs".into()],
        }),
        could_not_be_used(r#"`category` is "chore"; it must be one of "bug" or "docs"."#)
    );
}

#[test_log::test]
fn categories_holding_a_comma_are_listed_with_semicolons_and_a_last_or() {
    assert_eq!(
        parse_error(&ParseError::Category {
            category: "chore".into(),
            categories: vec!["bug, minor".into(), "docs".into(), "feature".into()],
        }),
        could_not_be_used(
            r#"`category` is "chore"; it must be one of "bug, minor"; "docs" or "feature"."#
        )
    );
}

#[test_log::test]
fn the_outcomes_a_result_may_have_are_listed_with_commas_and_a_last_or() {
    let error = ParseError::Schema(Mismatches::new(
        Mismatch::Outcome {
            outcome: None,
            outcomes: vec!["done".into(), "blocked".into(), "gave-up".into()],
        },
        vec![],
    ));

    assert_eq!(
        parse_error(&error),
        could_not_be_used(
            "The result does not match its schema:\n\n\
             - `/outcome` is missing; it must be one of \"done\", \"blocked\" or \"gave-up\"\n"
        )
    );
}

#[test_log::test]
fn a_reply_to_a_thread_the_pull_request_lacks_is_sent_back() {
    assert_eq!(
        result_error(&SentBack::UnknownThread { thread: "T9" }),
        could_not_be_used("`T9` is not a thread on the pull request")
    );
}

#[test_log::test]
fn a_reference_that_is_not_a_regular_file_is_sent_back() {
    let reference = BadReference::NotAFile {
        path: "/record/attachments/missing.png".into(),
    };
    assert_eq!(
        result_error(&SentBack::Reference(&reference)),
        could_not_be_used(
            "`/record/attachments/missing.png` is in the attachments directory but is not a \
             regular file there: put the file itself at that path, not a symlink to it, or take \
             the reference out"
        )
    );
}

#[test_log::test]
fn a_reference_outside_the_directory_is_sent_back() {
    let reference = BadReference::Outside {
        path: "/record/attachments/../notes.txt".into(),
    };
    assert_eq!(
        result_error(&SentBack::Reference(&reference)),
        could_not_be_used(
            "`/record/attachments/../notes.txt` is outside the attachments directory: copy the \
             file into the directory and refer to it there"
        )
    );
}

#[test_log::test]
fn a_reference_to_a_kind_linewright_does_not_attach_is_sent_back_with_the_kinds() {
    let reference = BadReference::Kind {
        path: "/record/attachments/tree.txt".into(),
    };
    assert_eq!(
        result_error(&SentBack::Reference(&reference)),
        could_not_be_used(
            "`/record/attachments/tree.txt` is not an image or a video: only `.png`, `.jpg`, \
             `.jpeg`, `.gif`, `.webp`, `.svg`, `.mp4`, `.mov` and `.webm` files can be attached. \
             Put anything else, such as text output or a log, in the Markdown itself, in a \
             fenced code block or a `<details>` block when it is long, or take the reference out"
        )
    );
}

#[test_log::test]
fn an_attachment_github_refuses_is_sent_back_with_githubs_reason() {
    assert_eq!(
        result_error(&SentBack::AttachmentRefused {
            file: Path::new("/record/attachments/big.mp4"),
            reason: "the file is too large",
        }),
        could_not_be_used(
            "GitHub refused the attachment `/record/attachments/big.mp4`: the file is too \
             large. Make the file one GitHub takes, such as a smaller one, or take the \
             reference out"
        )
    );
}

#[test_log::test]
fn an_operation_in_progress_is_sent_back_to_be_finished() {
    assert_eq!(
        loose_ends(&LooseEnd::Git(&git::LooseEnd::RebaseInProgress)),
        not_finished("a rebase is in progress; finish it.")
    );
    assert_eq!(
        loose_ends(&LooseEnd::Git(&git::LooseEnd::MergeInProgress)),
        not_finished("a merge is in progress; finish it.")
    );
}

#[test_log::test]
fn uncommitted_changes_are_sent_back_with_gits_status() {
    assert_eq!(
        loose_ends(&LooseEnd::Git(&git::LooseEnd::Uncommitted {
            status: " M src/lib.rs\n?? notes.txt".into()
        })),
        not_finished(
            "there are uncommitted changes; commit them:\n\n```\n M src/lib.rs\n?? notes.txt\n```"
        )
    );
}

#[test_log::test]
fn a_done_answer_with_no_commits_is_sent_back() {
    assert_eq!(
        loose_ends(&LooseEnd::NoCommits),
        not_finished(
            "you answered `done`, but the branch has no commits. Commit your work, or answer \
             `gave-up` with the reason if the ticket needs no change."
        )
    );
}

#[test_log::test]
fn the_follow_up_for_an_attachment_holding_a_secret_names_its_variable_and_the_file() {
    assert_eq!(
        loose_ends(&LooseEnd::AttachmentSecret {
            variable: "NPM_TOKEN",
            file: Path::new("/record/attachments/log.txt"),
        }),
        not_finished(
            "the attachment `/record/attachments/log.txt` holds the value of `NPM_TOKEN`, a \
             secret. Linewright uploads no attachment that holds a secret, so take the secret \
             out of the file, or take the reference to it out of your result."
        )
    );
}
