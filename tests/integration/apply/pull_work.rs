use chrono::TimeDelta;
use linewright::apply::pull_work::*;
use linewright::domain::{
    CheckFailure, Comment, Login, Mergeability, Number, Pull, PullState, Rebaseability, Review,
    ReviewState, Thread, Timestamp, UpdateMethod,
};
use linewright::git::Conflict;
use linewright::prompts::PullTask;
use linewright::reconcile::{Feedback, WorkKind};
use linewright::results::ThreadReply;
use rstest::rstest;

use crate::common::{LINEWRIGHT, at, linewright, logins};

fn t(minute: i64) -> Timestamp {
    at("2026-10-01T12:00:00Z") + TimeDelta::minutes(minute)
}

fn comment(author: &str, minute: i64) -> Comment {
    Comment {
        author: Login::new(author),
        body: "Rename it.".into(),
        created_at: t(minute),
    }
}

fn review(author: &str, minute: i64) -> Review {
    Review {
        author: Login::new(author),
        state: ReviewState::ChangesRequested,
        body: "Close.".into(),
        submitted_at: t(minute),
    }
}

/// A thread whose comments are by `authors`, the first opening it, a
/// minute apart from minute 1.
fn thread(id: &str, authors: &[&str]) -> Thread {
    Thread {
        id: id.into(),
        path: "src/x.txt".into(),
        resolved: false,
        comments: (1..)
            .zip(authors)
            .map(|(minute, author)| comment(author, minute))
            .collect(),
    }
}

fn pull(closes: &[u64], threads: Vec<Thread>) -> Pull {
    Pull {
        number: Number(9),
        title: "Do it".into(),
        body: "Does it.".into(),
        author: linewright(),
        created_at: t(0),
        updated_at: t(0),
        base_ref: "main".into(),
        head_ref: "7-do-it".into(),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: false },
        closes: closes.iter().copied().map(Number).collect(),
        pushes: vec![],
        mergeability: Mergeability::Clean,
        rebaseability: Rebaseability::Rebaseable,
        checks: vec![],
        comments: vec![],
        reviews: vec![],
        threads,
        labels: vec![],
        labelings: vec![],
        assignees: vec![linewright()],
        assignments: vec![],
        review_decision: None,
    }
}

fn reply(thread: &str, resolve: bool) -> ThreadReply {
    ThreadReply {
        thread: thread.into(),
        body: "Renamed.".into(),
        resolve,
    }
}

#[rstest]
#[case::no_new_description_leaves_it(&[7], None, None)]
#[case::a_new_description_closes_the_ticket(
    &[7],
    Some("Does it, renamed."),
    Some("Does it, renamed.\n\nCloses #7")
)]
#[case::a_new_description_keeps_every_issue_the_pull_request_closes(
    &[7, 12],
    Some("Does it, fixed.\n\nCloses #12"),
    Some("Does it, fixed.\n\nCloses #12\n\nCloses #7")
)]
#[case::a_new_description_closing_its_issues_is_kept_trimmed(
    &[7],
    Some("\nDoes it.\n\nCloses #7\n"),
    Some("Does it.\n\nCloses #7")
)]
fn a_new_description_closes_the_issues_the_pull_request_closes(
    #[case] closes: &[u64],
    #[case] pr_body: Option<&str>,
    #[case] expected: Option<&str>,
) {
    let writes = done(
        &pull(closes, vec![]),
        WorkKind::FixCi,
        pr_body,
        "Fixed.",
        &[],
        &[],
    );

    assert_eq!(writes.body.as_deref(), expected);
}

#[rstest]
#[case::a_bot_thread_whose_reply_asks_is_resolved(Some(&["linter[bot]"][..]), true, true)]
#[case::a_bot_thread_whose_reply_does_not_ask_is_left_open(Some(&["linter[bot]"][..]), false, false)]
#[case::a_persons_thread_is_left_for_them_to_resolve(Some(&["maintainer"][..]), true, false)]
#[case::a_bot_thread_a_person_answered_in_is_resolved(
    Some(&["linter[bot]", "maintainer"][..]),
    true,
    true
)]
#[case::a_persons_thread_a_bot_answered_in_is_left_open(
    Some(&["maintainer", "linter[bot]"][..]),
    true,
    false
)]
#[case::a_thread_not_on_the_pull_request_is_left_alone(None, true, false)]
fn a_reply_resolves_its_thread_only_where_a_bot_opened_it(
    #[case] opened_by: Option<&[&str]>,
    #[case] asks_to_resolve: bool,
    #[case] resolve: bool,
) {
    let threads = opened_by.map(|authors| thread("T2-1", authors));
    let writes = done(
        &pull(&[7], threads.into_iter().collect()),
        WorkKind::Rework,
        None,
        "Renamed as asked.",
        &[reply("T2-1", asks_to_resolve)],
        &[],
    );

    assert_eq!(
        writes.replies,
        [Reply {
            thread: "T2-1".into(),
            body: "Renamed.".into(),
            resolve,
        }]
    );
}

#[test]
fn every_reply_is_made_in_order_and_the_comment_is_posted() {
    let writes = done(
        &pull(
            &[7],
            vec![
                thread("T2-1", &["maintainer"]),
                thread("T2-2", &["linter[bot]"]),
            ],
        ),
        WorkKind::Rework,
        None,
        "Renamed as asked.",
        &[reply("T2-2", true), reply("T2-1", false)],
        &[],
    );

    assert_eq!(
        writes,
        PullWorkWrites {
            body: None,
            replies: vec![
                Reply {
                    thread: "T2-2".into(),
                    body: "Renamed.".into(),
                    resolve: true
                },
                Reply {
                    thread: "T2-1".into(),
                    body: "Renamed.".into(),
                    resolve: false
                },
            ],
            comment: "Renamed as asked.".into(),
            reviewers: vec![],
        }
    );
}

#[rstest]
#[case::a_rework_asks_the_people_whose_feedback_it_answered(
    WorkKind::Rework,
    &["reviewer"],
    &["reviewer"]
)]
#[case::a_rework_asks_each_person_once_in_order(
    WorkKind::Rework,
    &["reviewer", "designer", "Reviewer"],
    &["designer", "reviewer"]
)]
#[case::a_rework_of_no_ones_feedback_asks_no_one(WorkKind::Rework, &[], &[])]
#[case::fixing_ci_asks_no_one(WorkKind::FixCi, &["reviewer"], &[])]
#[case::an_update_asks_no_one(WorkKind::Update, &["reviewer"], &[])]
fn a_rework_requests_review_from_the_people_whose_feedback_it_answered(
    #[case] kind: WorkKind,
    #[case] feedback_authors: &[&str],
    #[case] expected: &[&str],
) {
    let writes = done(
        &pull(&[7], vec![]),
        kind,
        None,
        "Done.",
        &[],
        &logins(feedback_authors),
    );

    assert_eq!(writes.reviewers, logins(expected));
}

#[rstest]
#[case::a_comment(Some("reviewer"), None, None, &["reviewer"])]
#[case::a_review(None, Some("reviewer"), None, &["reviewer"])]
#[case::a_thread(None, None, Some("reviewer"), &["reviewer"])]
#[case::a_bot(Some("linter[bot]"), None, None, &[])]
#[case::linewright(None, None, Some(LINEWRIGHT), &[])]
fn the_people_who_gave_feedback_are_its_authors_but_bots_and_linewright(
    #[case] commenter: Option<&str>,
    #[case] reviewer: Option<&str>,
    #[case] in_thread: Option<&str>,
    #[case] expected: &[&str],
) {
    let comments: Vec<Comment> = commenter.map(|a| comment(a, 1)).into_iter().collect();
    let reviews: Vec<Review> = reviewer.map(|a| review(a, 1)).into_iter().collect();
    let thread = thread("T2-1", &in_thread.into_iter().collect::<Vec<_>>());
    let thread_comments: Vec<&Comment> = thread.comments.iter().collect();
    let feedback = Feedback {
        comments: comments.iter().collect(),
        reviews: reviews.iter().collect(),
        threads: if thread_comments.is_empty() {
            vec![]
        } else {
            vec![(&thread, thread_comments)]
        },
    };

    assert_eq!(feedback_people(&feedback, &linewright()), logins(expected));
}

#[rstest]
#[case::feedback_before_is_dropped(1, 0)]
#[case::feedback_at_the_time_is_dropped(2, 0)]
#[case::feedback_after_is_kept(3, 1)]
fn late_feedback_is_what_came_after_the_time(#[case] minute: i64, #[case] kept: usize) {
    let comments = [comment("reviewer", minute)];
    let reviews = [review("reviewer", minute)];
    let thread = Thread {
        comments: vec![comment("reviewer", minute)],
        ..thread("T2-1", &[])
    };
    let feedback = Feedback {
        comments: comments.iter().collect(),
        reviews: reviews.iter().collect(),
        threads: vec![(&thread, thread.comments.iter().collect())],
    };

    let late = late(&feedback, t(2));

    assert_eq!(
        (late.comments.len(), late.reviews.len(), late.threads.len()),
        (kept, kept, kept)
    );
}

#[test]
fn late_feedback_in_a_thread_keeps_only_its_late_comments() {
    let thread = thread("T2-1", &["maintainer", "reviewer", "designer"]);
    let feedback = Feedback {
        threads: vec![(&thread, thread.comments.iter().collect())],
        ..Feedback::default()
    };

    let late = late(&feedback, t(2));

    let authors: Vec<&str> = late.threads[0]
        .1
        .iter()
        .map(|c| c.author.as_str())
        .collect();
    assert_eq!(authors, ["designer"]);
}

#[rstest]
#[case::work_its_session_changed_on_an_open_pull_request_is_kept(true, true, true)]
#[case::work_its_session_left_as_it_was_is_not(false, true, false)]
#[case::work_on_a_pull_request_that_closed_is_not(true, false, false)]
#[case::unchanged_work_on_a_closed_pull_request_is_not(false, false, false)]
fn a_stopped_pull_request_runs_work(#[case] changed: bool, #[case] open: bool, #[case] kept: bool) {
    assert_eq!(keeps_work(changed, open), kept);
}

/// The work of a case, which owns what it refers to.
#[derive(Clone, Copy, Debug)]
enum Job {
    Rework,
    FixCi,
    Update,
}

#[rstest]
#[case::a_rework_without_a_conflict_addresses_its_feedback(Job::Rework, false, Some(Job::Rework))]
#[case::a_rework_with_a_conflict_also_resolves_it(Job::Rework, true, Some(Job::Rework))]
#[case::a_ci_fix_without_a_conflict_fixes_its_failures(Job::FixCi, false, Some(Job::FixCi))]
#[case::a_ci_fix_with_a_conflict_also_resolves_it(Job::FixCi, true, Some(Job::FixCi))]
#[case::an_update_with_a_conflict_resolves_it(Job::Update, true, Some(Job::Update))]
#[case::an_update_without_a_conflict_starts_no_session(Job::Update, false, None)]
fn a_pull_request_runs_session_is_asked(
    #[case] job: Job,
    #[case] conflicted: bool,
    #[case] asked: Option<Job>,
) {
    let conflict = Conflict {
        method: UpdateMethod::Rebase,
        files: vec!["src/x.txt".into()],
    };
    let feedback_comment = comment("maintainer", 2);
    let feedback = || Feedback {
        comments: vec![&feedback_comment],
        ..Feedback::default()
    };
    let failures = [CheckFailure {
        name: "test".into(),
        summary: "1 failed".into(),
        log: None,
    }];
    let conflict = conflicted.then_some(&conflict);
    let pull_job = match job {
        Job::Rework => PullJob::Rework(feedback()),
        Job::FixCi => PullJob::FixCi(&failures),
        Job::Update => PullJob::Update,
    };

    let task = session_task(pull_job, conflict);

    let expected = asked.map(|asked| match asked {
        Job::Rework => PullTask::Rework {
            feedback: feedback(),
            conflict,
        },
        Job::FixCi => PullTask::FixCi {
            failures: &failures,
            conflict,
        },
        Job::Update => PullTask::Update(conflict.unwrap()),
    });
    assert_eq!(task, expected);
}
