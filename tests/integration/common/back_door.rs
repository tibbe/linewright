//! Creation Methods: each writes into the fakes the state a stage leaves,
//! with the scripts the stage's sessions answered with, so that a later
//! stage answers as they would. `front_door` checks each against its stage.

use chrono::TimeDelta;
use linewright::domain::{CheckState, Login, Number, Requirement};
use linewright::harness::SessionError;
use linewright::reconcile::WorkKind;
use linewright::store::{NewIssue, PushMode, Store};
use linewright::tracking::{self, FiledTicket};
use serde_json::{Value, json};

use super::files::commit_in;
use super::harness::subject_file;
use super::world::{
    BUILT_AS_ASKED, Implementation, LINEWRIGHT, World, check, design_issues, implemented_file,
};

/// Each triage asks the maintainer whether widgets should be resizable,
/// until a comment answers that they should.
pub fn triage_for_the_maintainer(world: &World) {
    world.harness.on(WorkKind::Triage, |call| {
        let outcome = if call.spec.prompt.contains("Yes, make them resizable.") {
            json!({
                "outcome": "ready-for-agent",
                "category": "enhancement",
                "comment": "## Agent Brief\n\n**Summary:** Make widgets resizable."
            })
        } else {
            json!({
                "outcome": "ready-for-human",
                "category": "enhancement",
                "comment": "Should widgets be resizable?"
            })
        };
        Ok(outcome.to_string())
    });
}

/// Each triage asks the people `ask` names which version, commenting
/// `comment`, until a comment answers version 3.
pub fn triage_asking(world: &World, ask: Value, comment: &'static str) {
    world.harness.on(WorkKind::Triage, move |call| {
        let outcome = if call.spec.prompt.contains("Version 3.") {
            json!({"outcome": "ready-for-agent", "category": "bug", "comment": "Triaged again."})
        } else {
            json!({"outcome": "needs-info", "category": "bug", "ask": ask, "comment": comment})
        };
        Ok(outcome.to_string())
    });
}

/// The design, the tickets in order, with their bodies, and the comment of
/// each cut [`tickets_script`] scripts.
const CUT_DESIGN: &str = "A gadget store, and a listing that reads it.";
const CUT_TICKETS: [(&str, &str); 2] = [
    ("Store gadgets", "## What to build\n\nGadgets persist."),
    ("List gadgets", "## What to build\n\nList them."),
];
const CUT_COMMENT: &str = "Two slices: storing, then listing.";

/// Each design cuts its item into two tickets, storing gadgets and then,
/// blocked by it, listing them.
pub fn tickets_script(world: &World) {
    let [(store_title, store_body), (list_title, list_body)] = CUT_TICKETS;
    let result = json!({
        "outcome": "designed",
        "design": CUT_DESIGN,
        "tickets": [
            {"id": "store", "title": store_title, "body": store_body, "blocked_by": []},
            {"id": "list", "title": list_title, "body": list_body, "blocked_by": ["store"]}
        ],
        "comment": CUT_COMMENT
    });
    world
        .harness
        .on(WorkKind::Design, move |_| Ok(result.to_string()));
}

/// What each implement run [`implement_gadgets`] scripts commits and answers.
const GADGETS: Implementation = Implementation {
    contents: "gadget\n",
    message: "Add gadgets",
    title: "Add gadgets",
    body: "## Summary\n\nGadgets.",
};

/// Each implement run commits a file named for its ticket and answers that
/// it is done.
pub fn implement_gadgets(world: &World) {
    world.harness.on(WorkKind::Implement, |call| {
        let file = implemented_file(&subject_file(call));
        commit_in(
            &call.spec.dir,
            &[(&file, GADGETS.contents)],
            GADGETS.message,
        );
        Ok(
            json!({"outcome": "done", "pr_title": GADGETS.title, "pr_body": GADGETS.body})
                .to_string(),
        )
    });
}

/// Merges a spec change titled "Gadgets" that changes the spec only.
pub async fn merge_spec_change(world: &World) -> Number {
    merge_gadgets(world, &[SPEC_OF_GADGETS]).await
}

/// Merges a spec change titled "Gadgets" that changes the spec and code.
pub async fn merge_spec_and_code_change(world: &World) -> Number {
    merge_gadgets(
        world,
        &[
            SPEC_OF_GADGETS,
            ("src/gadgets.rs", "// Gadgets live here.\n"),
        ],
    )
    .await
}

const SPEC_OF_GADGETS: (&str, &str) = ("docs/spec.md", "# Spec\n\nGadgets exist.\n");

async fn merge_gadgets(world: &World, files: &[(&str, &str)]) -> Number {
    world
        .store
        .merge_change("maintainer", "Gadgets", "Adds gadgets.", files)
        .await
        .unwrap()
}

/// A `ready-for-agent` issue not designed yet.
pub fn open_undesigned_gadgets(world: &World) -> Number {
    world.store.open_issue(
        "maintainer",
        "Gadgets",
        "Add gadgets.",
        &["ready-for-agent"],
    )
}

/// Opens the ticket "Gadgets", ready for an agent and designed.
pub fn open_designed_gadgets(world: &World) -> Number {
    world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    )
}

/// The subject of the commit Linewright makes of what a stopped run's
/// session left uncommitted.
pub const UNFINISHED: &str = "Unfinished work, kept when its run stopped";

/// The whole message of that commit.
const UNFINISHED_MESSAGE: &str = "Unfinished work, kept when its run stopped\n\n\
     Linewright committed what the session left uncommitted when its run stopped. The next \
     run folds it into its own commits.\n\nLinewright-Unfinished: true";

/// How a hand-back starts when the session timed out.
pub const TIMED_OUT: &str = "The session ended unexpectedly: it timed out. The work may need \
                             cutting smaller, or `session-timeout` may need to be longer.";

/// How a hand-back says where the work so far is kept.
pub fn kept_on(branch: &str) -> String {
    format!("\n\nThe work so far is on the branch `{branch}`.")
}

/// A session's answer that it timed out.
pub fn timed_out() -> Result<String, SessionError> {
    Err(SessionError::TimedOut {
        log_tail: String::new(),
    })
}

/// The file [`implement_giving_up_with_earlier_work`]'s runs leave on their
/// ticket's branch.
pub const EARLIER_WORK: &str = "src/partial.txt";

/// The message of the first commit a session makes on a ticket.
const START: &str = "Start";

/// What a session commits to `EARLIER_WORK` as it starts.
const STARTED: (&str, &str) = (EARLIER_WORK, "half\n");

/// Each implement run commits a start, `EARLIER_WORK`, then gives up.
pub fn implement_giving_up_with_earlier_work(world: &World) {
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[STARTED], START);
        Ok(json!({"outcome": "gave-up", "reason": "Too hard."}).to_string())
    });
}

/// What [`start_and_leave_edits`] commits, and then leaves uncommitted
/// besides an ignored file.
const STARTED_IGNORING_LOGS: [(&str, &str); 2] = [(".gitignore", "*.log\n"), STARTED];
const LEFT_AFTER_STARTING: [(&str, &str); 2] = [
    ("src/partial.txt", "half and more\n"),
    ("src/new.txt", "new\n"),
];

/// Commits a start, then leaves an edit to it, a new file and an ignored
/// file uncommitted.
pub fn start_and_leave_edits(dir: &std::path::Path) {
    commit_in(dir, &STARTED_IGNORING_LOGS, START);
    write_in(dir, &LEFT_AFTER_STARTING);
    write_in(dir, &[("build.log", "noise\n")]);
}

/// Writes `files` in `dir`, leaving them uncommitted.
fn write_in(dir: &std::path::Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        std::fs::write(dir.join(path), contents).unwrap();
    }
}

/// Each implement run [`start_and_leave_edits`], then times out.
pub fn implement_timing_out_with_edits(world: &World) {
    world.harness.on(WorkKind::Implement, |call| {
        start_and_leave_edits(&call.spec.dir);
        timed_out()
    });
}

/// What [`rename_and_leave_edits`] and each rework [`World::rework_asked`]
/// scripts commit, with its message, and what the first then leaves
/// uncommitted.
pub(super) const RENAMED: (&str, &str) = ("src/renamed.txt", "renamed\n");
pub(super) const RENAME: &str = "Rename";
const LEFT_AFTER_RENAMING: [(&str, &str); 2] = [
    ("src/renamed.txt", "renamed and more\n"),
    ("src/new.txt", "new\n"),
];

/// How each rework [`World::rework_asked`] scripts answers.
pub(super) const RENAMED_COMMENT: &str = "Renamed.";

/// Who asks for rework in [`World::rework_asked`].
pub(super) const ASKS_FOR_REWORK: &str = "maintainer";

/// Commits a rename, then leaves an edit to it and a new file uncommitted.
pub fn rename_and_leave_edits(dir: &std::path::Path) {
    commit_in(dir, &[RENAMED], RENAME);
    write_in(dir, &LEFT_AFTER_RENAMING);
}

/// Each rework [`rename_and_leave_edits`], then times out.
pub fn rework_timing_out_with_edits(world: &World) {
    world.harness.on(WorkKind::Rework, |call| {
        rename_and_leave_edits(&call.spec.dir);
        timed_out()
    });
}

/// The spec gap the `…_filing_a_spec_gap` scripts file, by title and body.
pub const SPEC_GAP: (&str, &str) = ("Spec: gadget colors", "The spec doesn't say.");

/// A session's answer that it is blocked on whether gadgets expire, filing
/// [`SPEC_GAP`].
fn blocked_filing_a_spec_gap() -> String {
    let (title, body) = SPEC_GAP;
    json!({
        "outcome": "blocked",
        "question": "Should gadgets expire?",
        "spec_gaps": [{"title": title, "body": body}]
    })
    .to_string()
}

/// Each implement run answers it is blocked, filing a spec gap.
pub fn implement_blocked_filing_a_spec_gap(world: &World) {
    world
        .harness
        .on(WorkKind::Implement, |_| Ok(blocked_filing_a_spec_gap()));
}

/// Each design answers it is blocked, filing a spec gap.
pub fn design_blocked_filing_a_spec_gap(world: &World) {
    world
        .harness
        .on(WorkKind::Design, |_| Ok(blocked_filing_a_spec_gap()));
}

/// Each design gives up, for now.
pub fn design_giving_up(world: &World) {
    world.harness.on(WorkKind::Design, |_| {
        Ok(json!({"outcome": "gave-up", "reason": "Not now."}).to_string())
    });
}

/// A spec change [`World::spec_change_cut_into_tickets`] cut: the spec
/// change, its tracking issue and the tickets filed under it, in order.
pub struct SpecCut {
    pub spec_change: Number,
    pub tracking: Number,
    pub tickets: [Number; 2],
}

const BLOCKED: &str = "Blocked on a question the spec cannot decide:\n\nShould gadgets expire?";
const DESIGN_IT_AGAIN: &str = "\n\nAssign @factory to this pull request again to design it again.";

impl World {
    /// An issue needing triage, which triage handed to the maintainer with a
    /// question, each triage scripted as [`triage_for_the_maintainer`]
    /// scripts it.
    pub async fn issue_triaged_for_the_maintainer(&self) -> Number {
        triage_for_the_maintainer(self);
        let issue = self
            .store
            .open_issue("reporter", "Resize widgets", "", &["needs-triage"]);
        self.triaged(
            issue,
            &[],
            "Should widgets be resizable?\n\nAnswer with a label: `ready-for-agent` to \
             implement it as decided or `needs-triage` to triage it again. A comment alone \
             starts nothing.",
            &["enhancement", "ready-for-human"],
        )
        .await;
        issue
    }

    /// An issue needing triage, which triage asked its reporter about, each
    /// triage scripted as [`triage_asking`] scripts it to ask the reporter.
    pub async fn issue_asking_its_reporter(&self) -> Number {
        let comment = "Which version, @reporter?";
        triage_asking(self, json!(["reporter"]), comment);
        let issue = self
            .store
            .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
        self.triaged(issue, &["reporter"], comment, &["bug", "needs-info"])
            .await;
        issue
    }

    /// What triage leaves of `issue`, which needed triage: asking `asked`,
    /// commenting `comment` and labeling it `labels`.
    async fn triaged(&self, issue: Number, asked: &[&str], comment: &str, labels: &[&str]) {
        self.store.assign(issue).await.unwrap();
        let asked: Vec<Login> = asked.iter().map(|login| Login::new(*login)).collect();
        if !asked.is_empty() {
            self.store.assign_people(issue, &asked).await.unwrap();
        }
        self.store.comment(issue, comment).await.unwrap();
        for label in labels {
            self.store.add_label(issue, label).await.unwrap();
        }
        self.store
            .remove_label(issue, "needs-triage")
            .await
            .unwrap();
        self.store.unassign(issue).await.unwrap();
    }

    /// A spec change of the spec and of code, merged, which design cut into
    /// the tickets [`tickets_script`] scripts, filed under a tracking issue;
    /// each implement run scripted as [`implement_gadgets`] scripts it.
    pub async fn spec_change_cut_into_tickets(&self) -> SpecCut {
        tickets_script(self);
        implement_gadgets(self);
        let spec_change = merge_spec_and_code_change(self).await;
        self.store.assign(spec_change).await.unwrap();
        let tracking = self
            .store
            .create_issue(NewIssue {
                title: tracking::title("Gadgets"),
                body: tracking::body(spec_change, &[]),
                labels: vec![],
            })
            .await
            .unwrap();
        let filed = self.file_cut_under(tracking).await;
        self.store
            .edit_body(tracking, &tracking::body(spec_change, &filed))
            .await
            .unwrap();
        self.publish_cut(tracking, &filed).await;
        self.store
            .comment(
                spec_change,
                &format!(
                    "{CUT_COMMENT}\n\nTracked in {tracking}:\n\n{}",
                    in_order(&filed)
                ),
            )
            .await
            .unwrap();
        self.store
            .add_label(spec_change, "linewright:designed")
            .await
            .unwrap();
        self.store.unassign(spec_change).await.unwrap();
        SpecCut {
            spec_change,
            tracking,
            tickets: [filed[0].number, filed[1].number],
        }
    }

    /// [`World::spec_change_cut_into_tickets`]'s, with its first ticket
    /// implemented and its pull request opened.
    pub async fn spec_change_ticket_in_review(&self) -> (SpecCut, Number) {
        let cut = self.spec_change_cut_into_tickets().await;
        let pr = self.implemented(cut.tickets[0], &GADGETS).await;
        (cut, pr)
    }

    /// A `ready-for-agent` issue, which design cut into the tickets
    /// [`tickets_script`] scripts, filed under it: the issue and its tickets
    /// in order. Each implement run is scripted as [`implement_gadgets`]
    /// scripts it.
    pub async fn issue_cut_into_tickets(&self) -> (Number, [Number; 2]) {
        tickets_script(self);
        implement_gadgets(self);
        let issue = open_undesigned_gadgets(self);
        self.store.assign(issue).await.unwrap();
        let filed = self.file_cut_under(issue).await;
        self.publish_cut(issue, &filed).await;
        self.store
            .add_label(issue, "linewright:designed")
            .await
            .unwrap();
        self.store
            .remove_label(issue, "ready-for-agent")
            .await
            .unwrap();
        self.store.unassign(issue).await.unwrap();
        (issue, [filed[0].number, filed[1].number])
    }

    /// Files the tickets of [`tickets_script`]'s cut under `tracking`, each
    /// linked to the one blocking it.
    async fn file_cut_under(&self, tracking: Number) -> Vec<FiledTicket> {
        let mut filed: Vec<FiledTicket> = vec![];
        for (title, body) in CUT_TICKETS {
            let number = self
                .store
                .create_issue(NewIssue {
                    title: title.into(),
                    body: body.into(),
                    labels: vec![],
                })
                .await
                .unwrap();
            self.store.add_sub_issue(tracking, number).await.unwrap();
            if let Some(blocker) = filed.last() {
                self.store
                    .add_blocked_by(number, blocker.number)
                    .await
                    .unwrap();
            }
            filed.push(FiledTicket {
                number,
                title: title.into(),
            });
        }
        filed
    }

    /// Posts the cut's design on `tracking`, then readies its tickets.
    async fn publish_cut(&self, tracking: Number, filed: &[FiledTicket]) {
        self.store
            .comment(
                tracking,
                &format!(
                    "{CUT_DESIGN}\n\n## Tickets\n\n{}\n{CUT_COMMENT}",
                    in_order(filed)
                ),
            )
            .await
            .unwrap();
        for ticket in filed {
            for label in ["linewright:designed", "ready-for-agent"] {
                self.store.add_label(ticket.number, label).await.unwrap();
            }
        }
    }

    /// What design leaves of `issue`, labeled `ready-for-agent` and not
    /// claimed, designed on the issue as [`design_issues`] scripts each
    /// design.
    pub async fn designed_on_the_issue(&self, issue: Number) {
        design_issues(self);
        self.store.assign(issue).await.unwrap();
        if self.issue(issue).has_label("linewright:stalled") {
            self.store
                .remove_label(issue, "linewright:stalled")
                .await
                .unwrap();
        }
        self.store.comment(issue, BUILT_AS_ASKED).await.unwrap();
        self.store
            .add_label(issue, "linewright:designed")
            .await
            .unwrap();
    }

    /// A ticket the implement stage handed back, its branch keeping
    /// [`EARLIER_WORK`], each implement run scripted as
    /// [`implement_giving_up_with_earlier_work`] scripts it: the ticket and
    /// its branch.
    pub async fn ticket_handed_back_with_its_branch(&self) -> (Number, String) {
        implement_giving_up_with_earlier_work(self);
        let ticket = open_designed_gadgets(self);
        self.store.assign(ticket).await.unwrap();
        let branch = self.ticket_branch(ticket);
        self.store
            .commit(LINEWRIGHT, &branch, &[STARTED], START)
            .await
            .unwrap();
        let comment = format!(
            "The implementer handed the ticket back:\n\nToo hard.{}",
            kept_on(&branch)
        );
        self.ticket_handed_back(ticket, &comment, false).await;
        (ticket, branch)
    }

    /// A ticket whose implement session timed out, its branch keeping what
    /// [`start_and_leave_edits`] committed and left as an [`UNFINISHED`]
    /// commit, each implement run scripted as
    /// [`implement_timing_out_with_edits`] scripts it: the ticket and its
    /// branch.
    pub async fn ticket_timed_out_with_edits(&self) -> (Number, String) {
        implement_timing_out_with_edits(self);
        let ticket = open_designed_gadgets(self);
        self.store.assign(ticket).await.unwrap();
        let branch = self.ticket_branch(ticket);
        self.store
            .commit(LINEWRIGHT, &branch, &STARTED_IGNORING_LOGS, START)
            .await
            .unwrap();
        self.store
            .commit(
                LINEWRIGHT,
                &branch,
                &LEFT_AFTER_STARTING,
                UNFINISHED_MESSAGE,
            )
            .await
            .unwrap();
        let comment = format!("{TIMED_OUT}{}", kept_on(&branch));
        self.ticket_handed_back(ticket, &comment, true).await;
        (ticket, branch)
    }

    /// A ticket the implement stage handed back blocked on a question, having
    /// filed [`SPEC_GAP`], each implement run scripted as
    /// [`implement_blocked_filing_a_spec_gap`] scripts it.
    pub async fn ticket_blocked_filing_a_spec_gap(&self) -> Number {
        implement_blocked_filing_a_spec_gap(self);
        let ticket = open_designed_gadgets(self);
        self.store.assign(ticket).await.unwrap();
        self.file_spec_gap(ticket).await;
        self.ticket_handed_back(ticket, BLOCKED, false).await;
        ticket
    }

    /// What a hand-back leaves of `ticket`, claimed: `comment`, its design
    /// taken back, and marked stalled when `stalled`.
    async fn ticket_handed_back(&self, ticket: Number, comment: &str, stalled: bool) {
        self.store.comment(ticket, comment).await.unwrap();
        self.store
            .remove_label(ticket, "linewright:designed")
            .await
            .unwrap();
        if stalled {
            self.store
                .add_label(ticket, "linewright:stalled")
                .await
                .unwrap();
        }
        self.store
            .add_label(ticket, "ready-for-human")
            .await
            .unwrap();
        self.store
            .remove_label(ticket, "ready-for-agent")
            .await
            .unwrap();
        self.store.unassign(ticket).await.unwrap();
    }

    /// Files [`SPEC_GAP`], found while working on `item`.
    async fn file_spec_gap(&self, item: Number) {
        let (title, body) = SPEC_GAP;
        self.store
            .create_issue(NewIssue {
                title: title.into(),
                body: format!("{body}\n\nFound while working on {item}."),
                labels: vec!["ready-for-human".into()],
            })
            .await
            .unwrap();
    }

    /// A spec change of the spec, merged, which design handed back having
    /// given up, each design scripted as [`design_giving_up`] scripts it.
    pub async fn spec_change_handed_back(&self) -> Number {
        design_giving_up(self);
        let spec_change = merge_spec_change(self).await;
        self.store.assign(spec_change).await.unwrap();
        self.spec_change_handed_back_saying(
            spec_change,
            "The designer handed the spec change back:\n\nNot now.",
        )
        .await;
        spec_change
    }

    /// A spec change of the spec, merged, which design handed back blocked on
    /// a question, having filed [`SPEC_GAP`], each design scripted as
    /// [`design_blocked_filing_a_spec_gap`] scripts it.
    pub async fn spec_change_blocked_filing_a_spec_gap(&self) -> Number {
        design_blocked_filing_a_spec_gap(self);
        let spec_change = merge_spec_change(self).await;
        self.store.assign(spec_change).await.unwrap();
        self.file_spec_gap(spec_change).await;
        self.spec_change_handed_back_saying(spec_change, BLOCKED)
            .await;
        spec_change
    }

    async fn spec_change_handed_back_saying(&self, spec_change: Number, why: &str) {
        self.store
            .comment(spec_change, &format!("{why}{DESIGN_IT_AGAIN}"))
            .await
            .unwrap();
        self.store.unassign(spec_change).await.unwrap();
    }

    /// `pr`, Linewright's draft with no CI reported, marked ready for review
    /// once CI's grace has passed.
    pub async fn ready_for_review(&self, pr: Number) {
        self.clock.advance(TimeDelta::minutes(6));
        self.marked_ready(pr, "No CI reported: ready for review.")
            .await;
    }

    /// `pr`, Linewright's draft with no CI reported, which GitHub blocks from
    /// merging until a code owner reviews it, held for that review by auto-merge
    /// once CI's grace has passed.
    pub async fn held_for_a_code_owners_review(&self, pr: Number) {
        self.store.block_merge(
            pr,
            vec![Requirement::CodeOwnerReview {
                owners: vec!["alice".into()],
            }],
        );
        self.clock.advance(TimeDelta::minutes(6));
        self.marked_ready(
            pr,
            "Ready for review: GitHub requires a code owner's review (@alice) before \
             merging. Linewright merges it once GitHub allows it.",
        )
        .await;
    }

    /// `pr`, Linewright's draft, whose check `test` passed, marked ready for
    /// review.
    pub async fn ready_for_review_on_green_checks(&self, pr: Number) {
        self.store
            .set_checks(pr, vec![check("test", CheckState::Passed, "")])
            .await
            .unwrap();
        self.marked_ready(pr, "CI is green: ready for review.")
            .await;
    }

    async fn marked_ready(&self, pr: Number, comment: &str) {
        self.store.mark_ready(pr).await.unwrap();
        self.store.comment(pr, comment).await.unwrap();
    }

    /// `pr`, Linewright's, after [`ASKS_FOR_REWORK`] asked for rework as
    /// [`World::rework_asked`] asks, reworked as it scripts each rework: its
    /// commit pushed, its comment posted and whoever gave feedback asked to
    /// review it again.
    pub async fn reworked(&self, pr: Number) {
        self.rework_asked(pr);
        let branch = self.store.snapshot_pull(pr).await.head_ref;
        self.store
            .commit(LINEWRIGHT, &branch, &[RENAMED], RENAME)
            .await
            .unwrap();
        self.store.comment(pr, RENAMED_COMMENT).await.unwrap();
        self.store
            .request_review(pr, &[Login::new(ASKS_FOR_REWORK)])
            .await
            .unwrap();
    }

    /// [`World::ticket_in_review`]'s, after a maintainer asked for rework and
    /// the rework timed out, its branch keeping what
    /// [`rename_and_leave_edits`] committed and left as an [`UNFINISHED`]
    /// commit, each rework scripted as [`rework_timing_out_with_edits`]
    /// scripts it: the ticket, the pull request and its branch.
    pub async fn rework_timed_out_with_edits(&mut self) -> (Number, Number, String) {
        let (ticket, pr) = self.ticket_in_review().await;
        self.rework_asked(pr);
        rework_timing_out_with_edits(self);
        let branch = self.store.snapshot_pull(pr).await.head_ref;
        // One push of both commits, as the rework pushes them.
        let work = tempfile::tempdir().unwrap();
        let clone = work.path().join("clone");
        let identity = self.store.identity().await.unwrap();
        self.store
            .clone_repo(
                &clone,
                &branch,
                &[
                    ("user.name", &identity.name),
                    ("user.email", &identity.email),
                ],
            )
            .await
            .unwrap();
        commit_in(&clone, &[RENAMED], RENAME);
        commit_in(&clone, &LEFT_AFTER_RENAMING, UNFINISHED_MESSAGE);
        self.store
            .push(&clone, &branch, PushMode::Replace)
            .await
            .unwrap();
        self.store
            .comment(
                pr,
                &format!(
                    "{TIMED_OUT}{}\n\nAssign @factory to this pull request again to try again.",
                    kept_on(&branch)
                ),
            )
            .await
            .unwrap();
        self.store
            .add_label(pr, "linewright:stalled")
            .await
            .unwrap();
        self.store.unassign(pr).await.unwrap();
        (ticket, pr, branch)
    }
}

/// `tickets` as a numbered list, one per line.
fn in_order(tickets: &[FiledTicket]) -> String {
    tickets
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}. {} {}\n", i + 1, t.number, t.title))
        .collect()
}
