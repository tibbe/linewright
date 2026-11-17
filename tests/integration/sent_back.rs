//! The follow-ups a session is sent back with, each reason filled from its
//! part in `briefs/parts/` (docs/specs/runs.md, "Contract"), as the markers
//! of the files Linewright picks, with the data it fills in.

use std::path::Path;

use serde_json::json;

use crate::common::{self, could_not_be_used, marker, not_finished};
use linewright::attachments::BadReference;
use linewright::git;
use linewright::prompts::{self, LooseEnd, SentBack};
use linewright::results::{ContractError, Mismatch, Mismatches, ParseError};
use linewright::store::{FoundSecret, FoundSecrets};

fn result_error(reason: &SentBack) -> String {
    prompts::result_error(&common::marker_briefs().templates, reason)
}

fn loose_ends(problem: &LooseEnd) -> String {
    prompts::loose_ends(&common::marker_briefs().templates, problem)
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
        could_not_be_used(&marker("parts/result-no-json.md", &[]))
    );
}

#[test_log::test]
fn a_result_that_is_not_json_is_sent_back_with_serdes_message() {
    assert_eq!(
        parse_error(&ParseError::NotJson {
            error: "expected value at line 1 column 2".into()
        }),
        could_not_be_used(&marker(
            "parts/result-not-json.md",
            &[("error", "expected value at line 1 column 2")]
        ))
    );
}

#[test_log::test]
fn a_result_that_does_not_match_its_schema_is_sent_back_with_each_mismatch() {
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
    let [missing, outcome, outcome_missing, at, whole] = [
        marker("parts/result-missing.md", &[("key", "/question")]),
        marker(
            "parts/result-outcome.md",
            &[
                ("outcome", r#""finished""#),
                ("outcomes", r#""done", "blocked""#),
            ],
        ),
        marker(
            "parts/result-outcome-missing.md",
            &[("outcomes", r#""done", "blocked""#)],
        ),
        marker(
            "parts/result-at.md",
            &[
                ("at", "/issues/0/label"),
                (
                    "error",
                    r#""bug" is not one of "ready-for-agent" or "ready-for-human""#,
                ),
            ],
        ),
        marker(
            "parts/result-whole.md",
            &[("error", r#"[] is not of type "object""#)],
        ),
    ];

    assert_eq!(
        parse_error(&error),
        could_not_be_used(&marker(
            "parts/result-schema.md",
            &[(
                "mismatches",
                &format!("{missing}, {outcome}, {outcome_missing}, {at}, {whole}")
            )]
        ))
    );
}

#[test_log::test]
fn a_result_that_breaks_the_contract_is_sent_back_with_why() {
    for (error, why) in [
        (
            ContractError::Serde {
                error: "missing field `comment`".into(),
            },
            "missing field `comment`".to_string(),
        ),
        (
            ContractError::Login {
                written: "the reporter".into(),
            },
            marker("parts/result-login.md", &[("written", r#""the reporter""#)]),
        ),
        (
            ContractError::DuplicateId { id: "a".into() },
            marker("parts/result-ticket-id.md", &[("id", r#""a""#)]),
        ),
        (
            ContractError::UnknownBlocker {
                id: "a".into(),
                blocker: "b".into(),
            },
            marker(
                "parts/result-ticket-blocker.md",
                &[("id", r#""a""#), ("blocker", r#""b""#)],
            ),
        ),
        (
            ContractError::Cycle,
            marker("parts/result-ticket-cycle.md", &[]),
        ),
    ] {
        assert_eq!(
            contract(error),
            could_not_be_used(&marker("parts/result-contract.md", &[("error", &why)]))
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
        could_not_be_used(&marker(
            "parts/result-category.md",
            &[
                ("category", r#""chore""#),
                ("categories", r#""bug", "docs""#)
            ]
        ))
    );
}

#[test_log::test]
fn a_reply_to_a_thread_the_pull_request_lacks_is_sent_back() {
    assert_eq!(
        result_error(&SentBack::UnknownThread { thread: "T9" }),
        could_not_be_used(&marker("parts/result-thread.md", &[("thread", "T9")]))
    );
}

#[test_log::test]
fn a_reference_that_is_not_a_file_url_linewright_can_attach_is_sent_back() {
    let reference = BadReference::InvalidUrl {
        reference: "/record/attachments/a b.png".into(),
    };
    assert_eq!(
        result_error(&SentBack::Reference(&reference)),
        could_not_be_used(&marker(
            "parts/attachment-invalid-url.md",
            &[("file", "/record/attachments/a b.png")]
        ))
    );
}

#[test_log::test]
fn a_reference_that_is_not_a_regular_file_is_sent_back() {
    let reference = BadReference::NotAFile {
        reference: "/record/attachments/missing.png".into(),
    };
    assert_eq!(
        result_error(&SentBack::Reference(&reference)),
        could_not_be_used(&marker(
            "parts/attachment-not-a-file.md",
            &[("file", "/record/attachments/missing.png")]
        ))
    );
}

#[test_log::test]
fn a_reference_outside_the_directory_is_sent_back() {
    let reference = BadReference::Outside {
        reference: "/record/attachments/../notes.txt".into(),
    };
    assert_eq!(
        result_error(&SentBack::Reference(&reference)),
        could_not_be_used(&marker(
            "parts/attachment-outside.md",
            &[("file", "/record/attachments/../notes.txt")]
        ))
    );
}

#[test_log::test]
fn a_reference_to_a_kind_linewright_does_not_attach_is_sent_back_with_the_kinds() {
    let reference = BadReference::Kind {
        reference: "/record/attachments/tree.txt".into(),
    };
    assert_eq!(
        result_error(&SentBack::Reference(&reference)),
        could_not_be_used(&marker(
            "parts/attachment-kind.md",
            &[
                ("file", "/record/attachments/tree.txt"),
                (
                    "kinds",
                    "`.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.svg`, `.mp4`, `.mov`, \
                     `.webm`"
                )
            ]
        ))
    );
}

#[test_log::test]
fn an_attachment_github_refuses_is_sent_back_with_githubs_reason() {
    assert_eq!(
        result_error(&SentBack::AttachmentRefused {
            file: Path::new("/record/attachments/big.mp4"),
            reason: "the file is too large",
        }),
        could_not_be_used(&marker(
            "parts/attachment-refused.md",
            &[
                ("file", "/record/attachments/big.mp4"),
                ("reason", "the file is too large")
            ]
        ))
    );
}

#[test_log::test]
fn an_operation_in_progress_is_sent_back_to_be_finished() {
    assert_eq!(
        loose_ends(&LooseEnd::Git(&git::LooseEnd::RebaseInProgress)),
        not_finished(&marker("parts/rebase-in-progress.md", &[]))
    );
    assert_eq!(
        loose_ends(&LooseEnd::Git(&git::LooseEnd::MergeInProgress)),
        not_finished(&marker("parts/merge-in-progress.md", &[]))
    );
}

#[test_log::test]
fn uncommitted_changes_are_sent_back_with_gits_status() {
    assert_eq!(
        loose_ends(&LooseEnd::Git(&git::LooseEnd::Uncommitted {
            status: " M src/lib.rs\n?? notes.txt".into()
        })),
        not_finished(&marker(
            "parts/uncommitted.md",
            &[("status", " M src/lib.rs\n?? notes.txt")]
        ))
    );
}

#[test_log::test]
fn a_done_answer_with_no_commits_is_sent_back() {
    assert_eq!(
        loose_ends(&LooseEnd::NoCommits),
        not_finished(&marker("parts/no-commits.md", &[]))
    );
}

#[test_log::test]
fn the_follow_up_for_an_attachment_holding_a_secret_names_its_variable_and_the_file() {
    assert_eq!(
        loose_ends(&LooseEnd::AttachmentSecret {
            variable: "NPM_TOKEN",
            reference: "/record/attachments/log.txt",
        }),
        not_finished(&marker(
            "parts/attachment-secret.md",
            &[
                ("variable", "NPM_TOKEN"),
                ("file", "/record/attachments/log.txt")
            ]
        ))
    );
}

#[test_log::test]
fn the_follow_up_for_unfinished_commits_names_each_of_them_once() {
    for (rest, named) in [
        (vec![], "`abc1234`"),
        (vec!["def5678".to_string()], "`abc1234`, `def5678`"),
    ] {
        assert_eq!(
            loose_ends(&LooseEnd::Unfinished(&git::UnfinishedCommits {
                first: "abc1234".into(),
                rest,
            })),
            not_finished(&marker("parts/fold-unfinished.md", &[("commits", named)]))
        );
    }
}

#[test_log::test]
fn the_follow_up_for_secrets_push_protection_found_names_each_of_them() {
    let found = |kind: &str, commit: &str, path: &str| FoundSecret {
        kind: kind.into(),
        commit: commit.into(),
        path: path.into(),
    };
    let secrets = FoundSecrets::new(vec![
        found("GitHub Personal Access Token", "abc1234", ".env:1"),
        found("npm Access Token", "def5678", ".npmrc:2"),
    ])
    .unwrap();
    let marked = |kind: &str, commit: &str, path: &str| {
        marker(
            "parts/pushed-secret-found.md",
            &[("kind", kind), ("commit", commit), ("path", path)],
        )
    };

    assert_eq!(
        loose_ends(&LooseEnd::PushedSecret(&secrets)),
        not_finished(&marker(
            "parts/pushed-secret.md",
            &[(
                "secrets",
                &format!(
                    "{}, {}",
                    marked("GitHub Personal Access Token", "abc1234", ".env:1"),
                    marked("npm Access Token", "def5678", ".npmrc:2")
                )
            )]
        ))
    );
}
