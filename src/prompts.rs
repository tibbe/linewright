use std::fmt::Write;

use crate::briefs::Briefs;
use crate::domain::{CheckFailure, Comment, Issue, MergedPull, Number, Pull, UpdateMethod};
use crate::git::Conflict;
use crate::reconcile::Feedback;
use crate::workflow::Workflow;

pub struct Repo<'a> {
    pub default_branch: &'a str,
    pub spec_glob: &'a str,
}

pub struct Ticket<'a> {
    issue: &'a Issue,
    filed_gaps: Vec<&'a Issue>,
    /// Only for a ticket cut from a spec change.
    spec_diff: Option<String>,
}

impl<'a> Ticket<'a> {
    /// # Panics
    ///
    /// When it has a spec diff but was cut from no spec change, or the other
    /// way round.
    pub fn new(
        issue: &'a Issue,
        filed_gaps: Vec<&'a Issue>,
        spec_diff: Option<String>,
    ) -> Ticket<'a> {
        assert_eq!(
            spec_diff.is_some(),
            issue.cut_from().is_some(),
            "a ticket has a spec diff exactly when it was cut from a spec change"
        );
        Ticket {
            issue,
            filed_gaps,
            spec_diff,
        }
    }

    pub fn number(&self) -> Number {
        self.issue.number
    }

    pub fn filed_gaps(&self) -> &[&'a Issue] {
        &self.filed_gaps
    }
}

pub enum PullTask<'a> {
    Rework(Feedback<'a>),
    FixCi(&'a [CheckFailure]),
    /// Only bring the branch up to date: the session resolves the conflict.
    Update,
}

const TRIAGE: &str = "/linewright:triage";
const TO_TICKETS: &str = "/linewright:to-tickets";
const IMPLEMENT: &str = "/linewright:implement";

pub fn triage(repo: &Repo, issue: &Issue, categories: &[String]) -> String {
    let mut task = String::new();
    task_facts(&mut task, repo);
    let _ = writeln!(
        task,
        "- Categories to choose from: {}\n",
        categories
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    write_issue(&mut task, "The issue", issue);
    let contract = format!(
        r#"{{
  "outcome": "ready-for-agent" | "needs-info" | "ready-for-human",
  "category": {},
  "comment": "<the Markdown comment to post on the issue>"
}}"#,
        categories
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    compose(TRIAGE, &task, &contract)
}

pub fn tickets(repo: &Repo, pr: &MergedPull, spec_diff: &str) -> String {
    let mut task = String::new();
    task_facts(&mut task, repo);
    let _ = writeln!(
        task,
        "\n### The spec change: {} {}\n\nMerged by its author @{} into `{}`.\n\n{}\n",
        pr.number,
        pr.title,
        pr.author,
        repo.default_branch,
        or_none(&pr.body),
    );
    write_spec_diff(&mut task, "#### Spec diff", spec_diff);
    let contract = r#"{
  "tickets": [
    {
      "id": "<a short id of your choosing, unique among the tickets>",
      "title": "<the ticket's title>",
      "body": "<the ticket's Markdown body>",
      "blocked_by": ["<ids of the tickets that must be merged first>"]
    }
  ],
  "comment": "<Markdown summary of the breakdown, or why no tickets are needed>"
}

`tickets` is empty when the code already does what the spec change asks."#;
    compose(TO_TICKETS, &task, contract)
}

pub fn implement(repo: &Repo, ticket: &Ticket, branch: &str) -> String {
    let mut task = String::new();
    task_facts(&mut task, repo);
    let _ = writeln!(
        task,
        "- Branch: `{branch}`, fresh from `{}`. The fixed point for review is `origin/{}`.\n",
        repo.default_branch, repo.default_branch
    );
    write_ticket(&mut task, ticket);
    compose(IMPLEMENT, &task, IMPLEMENT_CONTRACT)
}

const IMPLEMENT_CONTRACT: &str = r#"One of:

{ "outcome": "done", "pr_title": "<the pull request title, per the linewright:pr skill>", "pr_body": "<the pull request body, per the linewright:pr skill, without a closing reference to the ticket: Linewright adds it>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "gave-up", "reason": "<why the work cannot be done>" }

where SPEC_GAP is { "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }
and `spec_gaps` lists the gaps that did not block the ticket (often none)."#;

pub fn pull_work(
    repo: &Repo,
    ticket: &Ticket,
    pr: &Pull,
    work: &PullTask,
    conflict: Option<&Conflict>,
) -> String {
    let mut task = String::new();
    task_facts(&mut task, repo);
    let _ = writeln!(
        task,
        "- Branch: `{}`, the head of pull request {} \"{}\". The fixed point for review is `origin/{}`.\n",
        pr.head_ref, pr.number, pr.title, repo.default_branch
    );
    if let Some(conflict) = conflict {
        let (verb, finish) = match conflict.method {
            UpdateMethod::Rebase => (
                "A rebase onto",
                "`GIT_EDITOR=true git rebase --continue`, resolving any later commit it stops on the same way",
            ),
            UpdateMethod::Merge => ("A merge of", "`git commit`"),
        };
        let _ = writeln!(
            task,
            "**First, finish bringing the branch up to date.** {verb} `origin/{}` stopped on conflicts in {}. Resolve them keeping both sides' intent, run the checks, and finish with {finish}. Don't abort it.\n",
            repo.default_branch,
            conflict
                .files
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    match work {
        PullTask::Rework(feedback) => write_feedback(&mut task, pr, feedback),
        PullTask::FixCi(failures) => {
            task.push_str("### Failing CI\n\nThese checks fail on the pull request's head. Make them pass.\n\n");
            for failure in *failures {
                let _ = writeln!(
                    task,
                    "#### `{}`\n\n{}\n",
                    failure.name,
                    or_none(&failure.summary)
                );
                let Some(log) = &failure.log else { continue };
                let _ = writeln!(task, "```\n{}\n```\n", log.step_output.trim_end());
                if let Some(full) = &log.full {
                    let _ = writeln!(
                        task,
                        "The full log: `gh run view --job {} --repo {} --log`\n",
                        full.job, full.repo
                    );
                }
            }
        }
        PullTask::Update => {}
    }
    let _ = writeln!(
        task,
        "### The pull request's description\n\n{}\n",
        or_none(&pr.body)
    );
    write_ticket(&mut task, ticket);
    let contract = r#"One of:

{ "outcome": "done", "comment": "<Markdown summary of what you changed, for the pull request>", "pr_body": "<only when your change made the description out of date: the whole body rewritten, per the linewright:pr skill>", "replies": [REPLY], "spec_gaps": [SPEC_GAP] }
{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "gave-up", "reason": "<why the work cannot be done>" }

where REPLY is { "thread": "<thread id>", "body": "<your reply in that thread>", "resolve": <true to resolve it> }
and SPEC_GAP is { "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }.
Reply in every thread with new feedback. Resolve only threads a bot opened: people resolve their own.
Leave `pr_body` out when the description still fits."#;
    compose(IMPLEMENT, &task, contract)
}

pub fn workflow(briefs: &Briefs, repo: &Repo, workflow: &Workflow) -> String {
    let mut task = String::new();
    task_facts(&mut task, repo);
    let _ = writeln!(
        task,
        "- Issues: at most {}.\n\n### The workflow `{}`\n\n{}",
        workflow.max_issues,
        workflow.name,
        or_none(&workflow.instructions)
    );
    let contract = r#"{
  "summary": "<Markdown: what you did and found>",
  "issues": [ISSUE]
}

where ISSUE is { "title": "<issue title>", "body": "<the issue's Markdown body>", "label": "ready-for-agent" | "ready-for-human" }
and `issues` lists the issues to file (often none)."#;
    compose(&briefs.workflow, &task, contract)
}

pub fn parse_error(error: &str) -> String {
    format!(
        "Your final message could not be used: {error}\n\nAnswer again with only the result JSON object."
    )
}

pub fn unfinished(problem: &str) -> String {
    format!(
        "Your work is not finished: {problem}\n\nFix that, then answer again with only the result JSON object."
    )
}

pub fn usage_limit_reset() -> String {
    "The usage limit that stopped you has reset. Carry on where you stopped, and answer as your task says.".into()
}

pub fn late_feedback(pr: &Pull, feedback: &Feedback) -> String {
    let mut text = String::from(
        "New feedback arrived on the pull request while you worked. Address it as well, commit, and answer again with only the result JSON object, covering all your work and replying in every thread with new feedback.\n\n",
    );
    write_feedback(&mut text, pr, feedback);
    text
}

fn compose(brief: &str, task: &str, contract: &str) -> String {
    let mut prompt = String::from(brief.trim());
    let _ = write!(
        prompt,
        "\n\n---\n\n# Your task\n\n{}\n\n---\n\n# Result\n\nEnd your session with a final message that is a single JSON object and nothing else:\n\n```\n{}\n```\n",
        task.trim(),
        contract.trim()
    );
    prompt
}

fn task_facts(task: &mut String, repo: &Repo) {
    let _ = writeln!(
        task,
        "- Default branch: `{}`\n- Spec: the tracked files matching `{}`\n- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.",
        repo.default_branch, repo.spec_glob
    );
}

fn write_issue(task: &mut String, heading: &str, issue: &Issue) {
    let _ = writeln!(
        task,
        "### {heading}: {} {}\n\nOpened by @{}. Labels: {}.\n\n{}\n",
        issue.number,
        issue.title,
        issue.author,
        if issue.labels.is_empty() {
            "none".to_string()
        } else {
            issue.labels.join(", ")
        },
        or_none(&issue.body)
    );
    if !issue.comments.is_empty() {
        let _ = writeln!(task, "#### Comments\n");
        for comment in &issue.comments {
            write_comment(task, comment);
        }
    }
}

fn write_ticket(task: &mut String, ticket: &Ticket) {
    write_issue(task, "The ticket", ticket.issue);
    if let Some(parent) = ticket.issue.parent {
        let _ = match parent.spec_change {
            Some(spec_change) => writeln!(
                task,
                "### Parent issue\n\n{}, the tracking issue of the spec change {spec_change}.\n",
                parent.number
            ),
            None => writeln!(task, "### Parent issue\n\n{}.\n", parent.number),
        };
    }
    if let Some(spec_diff) = &ticket.spec_diff {
        write_spec_diff(task, "#### The spec change's spec diff", spec_diff);
    }
    write_filed_gaps(task, &ticket.filed_gaps);
}

fn write_spec_diff(task: &mut String, heading: &str, spec_diff: &str) {
    let _ = writeln!(
        task,
        "{heading}\n\n```diff\n{}\n```\n",
        spec_diff.trim_end()
    );
}

fn write_filed_gaps(task: &mut String, gaps: &[&Issue]) {
    if gaps.is_empty() {
        return;
    }
    task.push_str(
        "### Spec gaps already filed\n\nThese are open issues for the maintainer. Don't list them in `spec_gaps` again.\n\n",
    );
    for gap in gaps {
        let _ = writeln!(
            task,
            "#### {} {}\n\n{}\n",
            gap.number,
            gap.title,
            gap.body.trim()
        );
    }
}

fn write_feedback(task: &mut String, pr: &Pull, feedback: &Feedback) {
    task.push_str("### Feedback to address\n\n");
    for comment in &feedback.comments {
        write_comment(task, comment);
    }
    for review in &feedback.reviews {
        let _ = writeln!(
            task,
            "**Review by @{}** ({:?}, {}):\n\n{}\n",
            review.author,
            review.state,
            review.submitted_at.format("%Y-%m-%d %H:%M UTC"),
            or_none(&review.body)
        );
    }
    for (thread, new) in &feedback.threads {
        let _ = writeln!(
            task,
            "**Thread `{}` on `{}`**, new comments:\n",
            thread.id, thread.path
        );
        for comment in new {
            write_comment(task, comment);
        }
    }
    task.push_str("### The pull request's conversation so far\n\n");
    for comment in &pr.comments {
        write_comment(task, comment);
    }
    for thread in pr.threads.iter().filter(|t| !t.resolved) {
        let _ = writeln!(
            task,
            "#### Open thread `{}` on `{}`\n",
            thread.id, thread.path
        );
        for comment in &thread.comments {
            write_comment(task, comment);
        }
    }
}

fn write_comment(task: &mut String, comment: &Comment) {
    let _ = writeln!(
        task,
        "**@{}** ({}):\n\n{}\n",
        comment.author,
        comment.created_at.format("%Y-%m-%d %H:%M UTC"),
        comment.body.trim()
    );
}

fn or_none(text: &str) -> &str {
    if text.trim().is_empty() {
        "_(empty)_"
    } else {
        text.trim()
    }
}
