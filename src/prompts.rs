use std::collections::BTreeSet;
use std::fmt::{self, Write};
use std::marker::PhantomData;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::automation::Automation;
use crate::credentials::Held;
use crate::domain::{CheckFailure, Comment, Issue, MergedPull, Number, Pull, UpdateMethod};
use crate::git::{BranchStart, Conflict, UnfinishedCommits};
use crate::reconcile::Feedback;
use crate::store::FoundSecrets;

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
    /// `conflict` is where the update the run starts with stopped, if it did.
    Rework {
        feedback: Feedback<'a>,
        conflict: Option<&'a Conflict>,
    },
    FixCi {
        failures: &'a [CheckFailure],
        conflict: Option<&'a Conflict>,
    },
    /// Only bring the branch up to date: the session resolves the conflict
    /// the update stopped on.
    Update(&'a Conflict),
}

/// Where the prompts are read from.
#[derive(Clone, Copy)]
pub enum Source<'a> {
    /// The prompts built into Linewright.
    Shipped,
    /// A directory laid out as `briefs/prompts/` is.
    Dir(&'a Path),
}

/// The values a template's placeholders are filled with.
trait Values {
    /// The placeholders, one per value.
    fn names() -> Vec<&'static str>;
    fn value(&self, name: &str) -> Option<&str>;
}

/// Declares the values of a template: a field per placeholder, after those of
/// `base`, if given, which it holds as `base`.
macro_rules! values {
    ($(#[$doc:meta])* $name:ident $(: $base:ident)? { $($field:ident),* $(,)? }) => {
        $(#[$doc])*
        struct $name {
            $(base: $base,)?
            $($field: String,)*
        }

        impl Values for $name {
            fn names() -> Vec<&'static str> {
                [$(<$base as Values>::names(),)? vec![$(stringify!($field)),*]].concat()
            }

            fn value(&self, name: &str) -> Option<&str> {
                match name {
                    $(stringify!($field) => Some(&self.$field),)*
                    _ => None$(.or_else(|| <$base as Values>::value(&self.base, name)))?,
                }
            }
        }
    };
}

values!(Facts {
    default_branch,
    spec_glob
});
values!(Triage: Facts {
    categories,
    issue,
    category_choices,
    author
});
values!(Tickets: Facts {
    number,
    title,
    author,
    body,
    spec_diff
});
values!(Implement: Facts {
    branch,
    holding,
    ticket
});
values!(
    /// An update's, and the start of a rework's and a fix's.
    PullRequest: Facts {
        branch,
        pr,
        pr_title,
        unfinished,
        conflict,
        description,
        ticket
    }
);
values!(Rework: PullRequest {
    feedback,
    conversation
});
values!(FixCi: PullRequest { failures });
values!(AutomationRun: Facts {
    max_issues,
    name,
    instructions
});
values!(DefaultBranch { default_branch });
values!(ExistingBranch {
    default_branch,
    unfinished
});
values!(Commits { commits });
values!(Stopped {
    default_branch,
    files
});
values!(FullLog { job, repo });
values!(FiledGaps { gaps });
values!(ParseError { error });
values!(LooseEnds { problem });
values!(HeldCredential {
    commit,
    variable,
    file
});
values!(PushedSecret { secrets });
values!(NoValues {});
values!(LateFeedback {
    feedback,
    conversation
});

impl Facts {
    fn of(repo: &Repo) -> Facts {
        Facts {
            default_branch: repo.default_branch.into(),
            spec_glob: repo.spec_glob.into(),
        }
    }
}

/// A prompt, or a part of one, from `briefs/prompts/`: Markdown with a
/// `{{name}}` placeholder for each of the values `V`.
struct Template<V> {
    text: String,
    values: PhantomData<fn(&V)>,
}

impl<V> Clone for Template<V> {
    fn clone(&self) -> Self {
        Template {
            text: self.text.clone(),
            values: PhantomData,
        }
    }
}

impl<V> fmt::Debug for Template<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Template").field(&self.text).finish()
    }
}

/// [`Template::read`] for the template at `$path` under `briefs/prompts/`,
/// which Linewright ships.
macro_rules! template {
    ($source:expr, $path:literal) => {
        Template::read(
            $source,
            $path,
            include_str!(concat!("../briefs/prompts/", $path)),
        )
    };
}

impl<V: Values> Template<V> {
    /// The template at `path` in `source`, which ships as `shipped`. A part
    /// or a follow-up is used without its trailing newline.
    fn read(source: Source, path: &str, shipped: &str) -> Result<Template<V>> {
        let text = match source {
            Source::Shipped => shipped.to_string(),
            Source::Dir(dir) => {
                let file = dir.join(path);
                std::fs::read_to_string(&file)
                    .with_context(|| format!("reading the brief {}", file.display()))?
            }
        };
        let text = if path.contains('/') {
            text.trim_end().to_string()
        } else {
            text
        };
        Template::parse(text).with_context(|| format!("reading the prompt {path}"))
    }

    /// Fails unless `text`'s placeholders are exactly the values'.
    fn parse(text: String) -> Result<Template<V>> {
        let names = V::names();
        let found: BTreeSet<&str> = placeholders(&text).map(|(_, name, _)| name).collect();
        let unknown: Vec<&str> = found
            .iter()
            .filter(|n| !names.contains(n))
            .copied()
            .collect();
        let missing: Vec<&str> = names
            .iter()
            .filter(|n| !found.contains(*n))
            .copied()
            .collect();
        if !unknown.is_empty() {
            bail!("it has unknown placeholders: {}", braced(&unknown));
        }
        if !missing.is_empty() {
            bail!("it lacks the placeholders {}", braced(&missing));
        }
        Ok(Template {
            text,
            values: PhantomData,
        })
    }

    /// Fills each placeholder with its value. A placeholder alone on its line
    /// that is filled with nothing takes the line with it, and the blank line
    /// after it, so an optional paragraph leaves no gap.
    fn fill(&self, values: &V) -> String {
        let value = |name: &str| {
            values
                .value(name)
                .expect("a parsed template has only its values' placeholders")
        };
        let mut filled = String::with_capacity(self.text.len());
        let mut lines = self.text.split_inclusive('\n').peekable();
        while let Some(line) = lines.next() {
            if let Some(name) = alone(line)
                && value(name).is_empty()
            {
                lines.next_if(|next| next.trim().is_empty());
                continue;
            }
            let mut rest = line;
            while let Some((start, name, end)) = placeholders(rest).next() {
                filled.push_str(&rest[..start]);
                filled.push_str(value(name));
                rest = &rest[end..];
            }
            filled.push_str(rest);
        }
        filled
    }
}

/// Each `{{name}}` in `text`: where it starts, its name and where it ends.
fn placeholders(text: &str) -> impl Iterator<Item = (usize, &str, usize)> {
    let mut from = 0;
    std::iter::from_fn(move || {
        while let Some(found) = text[from..].find("{{") {
            let start = from + found;
            from = start + 2;
            let name_len = text[from..]
                .find(|c: char| !(c.is_ascii_lowercase() || c == '_'))
                .unwrap_or(text.len() - from);
            if name_len > 0 && text[from + name_len..].starts_with("}}") {
                let end = from + name_len + 2;
                let name = &text[from..from + name_len];
                from = end;
                return Some((start, name, end));
            }
        }
        None
    })
}

/// The placeholder that is all of `line`, if one is.
fn alone(line: &str) -> Option<&str> {
    let line = line.trim_end_matches(['\n', '\r']);
    match placeholders(line).next() {
        Some((0, name, end)) if end == line.len() => Some(name),
        _ => None,
    }
}

fn braced(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{{{{{n}}}}}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The prompts Linewright sends sessions, and the parts it builds them from.
#[derive(Clone, Debug)]
pub struct Prompts {
    triage: Template<Triage>,
    tickets: Template<Tickets>,
    implement: Template<Implement>,
    rework: Template<Rework>,
    fix_ci: Template<FixCi>,
    update: Template<PullRequest>,
    automation: Template<AutomationRun>,
    new_branch: Template<DefaultBranch>,
    existing_branch: Template<ExistingBranch>,
    unfinished: Template<Commits>,
    unfinished_or_discard: Template<Commits>,
    conflict_rebase: Template<Stopped>,
    conflict_merge: Template<Stopped>,
    update_rebase: Template<Stopped>,
    update_merge: Template<Stopped>,
    full_log: Template<FullLog>,
    filed_gaps: Template<FiledGaps>,
    fold_unfinished: Template<Commits>,
    held_credential: Template<HeldCredential>,
    pushed_secret: Template<PushedSecret>,
    parse_error: Template<ParseError>,
    loose_ends: Template<LooseEnds>,
    usage_limit_reset: Template<NoValues>,
    late_feedback: Template<LateFeedback>,
}

impl Prompts {
    pub fn read(source: Source) -> Result<Prompts> {
        Ok(Prompts {
            triage: template!(source, "triage.md")?,
            tickets: template!(source, "tickets.md")?,
            implement: template!(source, "implement.md")?,
            rework: template!(source, "rework.md")?,
            fix_ci: template!(source, "fix-ci.md")?,
            update: template!(source, "update.md")?,
            automation: template!(source, "automation.md")?,
            new_branch: template!(source, "parts/new-branch.md")?,
            existing_branch: template!(source, "parts/existing-branch.md")?,
            unfinished: template!(source, "parts/unfinished.md")?,
            unfinished_or_discard: template!(source, "parts/unfinished-or-discard.md")?,
            conflict_rebase: template!(source, "parts/conflict-rebase.md")?,
            conflict_merge: template!(source, "parts/conflict-merge.md")?,
            update_rebase: template!(source, "parts/update-rebase.md")?,
            update_merge: template!(source, "parts/update-merge.md")?,
            full_log: template!(source, "parts/full-log.md")?,
            filed_gaps: template!(source, "parts/filed-gaps.md")?,
            fold_unfinished: template!(source, "parts/fold-unfinished.md")?,
            held_credential: template!(source, "parts/held-credential.md")?,
            pushed_secret: template!(source, "parts/pushed-secret.md")?,
            parse_error: template!(source, "follow-ups/parse-error.md")?,
            loose_ends: template!(source, "follow-ups/loose-ends.md")?,
            usage_limit_reset: template!(source, "follow-ups/usage-limit-reset.md")?,
            late_feedback: template!(source, "follow-ups/late-feedback.md")?,
        })
    }
}

pub fn triage(prompts: &Prompts, repo: &Repo, issue: &Issue, categories: &[String]) -> String {
    let mut rendered = String::new();
    write_issue(&mut rendered, "The issue", issue);
    prompts.triage.fill(&Triage {
        base: Facts::of(repo),
        categories: categories
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", "),
        issue: rendered.trim_end().into(),
        category_choices: categories
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(" | "),
        author: issue.author.to_string(),
    })
}

pub fn tickets(prompts: &Prompts, repo: &Repo, pr: &MergedPull, spec_diff: &str) -> String {
    prompts.tickets.fill(&Tickets {
        base: Facts::of(repo),
        number: pr.number.to_string(),
        title: pr.title.clone(),
        author: pr.author.to_string(),
        body: or_none(&pr.body).into(),
        spec_diff: spec_diff.trim_end().into(),
    })
}

pub fn implement(
    prompts: &Prompts,
    repo: &Repo,
    ticket: &Ticket,
    branch: &str,
    start: &BranchStart,
) -> String {
    let holding = match start {
        BranchStart::New => prompts.new_branch.fill(&DefaultBranch {
            default_branch: repo.default_branch.into(),
        }),
        BranchStart::Existing { unfinished } => prompts.existing_branch.fill(&ExistingBranch {
            default_branch: repo.default_branch.into(),
            unfinished: unfinished_sentence(&prompts.unfinished_or_discard, unfinished.as_ref()),
        }),
    };
    prompts.implement.fill(&Implement {
        base: Facts::of(repo),
        branch: branch.into(),
        holding,
        ticket: render_ticket(prompts, ticket),
    })
}

pub fn pull_work(
    prompts: &Prompts,
    repo: &Repo,
    ticket: &Ticket,
    pr: &Pull,
    work: &PullTask,
    unfinished: Option<&UnfinishedCommits>,
) -> String {
    let pull_request = |conflict: String| PullRequest {
        base: Facts::of(repo),
        branch: pr.head_ref.clone(),
        pr: pr.number.to_string(),
        pr_title: pr.title.clone(),
        unfinished: unfinished_sentence(&prompts.unfinished, unfinished),
        conflict,
        description: or_none(&pr.body).into(),
        ticket: render_ticket(prompts, ticket),
    };
    let first_update = |conflict: Option<&Conflict>| {
        conflict
            .map(|c| stopped(&prompts.conflict_rebase, &prompts.conflict_merge, repo, c))
            .unwrap_or_default()
    };
    match work {
        PullTask::Rework { feedback, conflict } => {
            let (feedback, conversation) = render_feedback(pr, feedback);
            prompts.rework.fill(&Rework {
                base: pull_request(first_update(*conflict)),
                feedback,
                conversation,
            })
        }
        PullTask::FixCi { failures, conflict } => prompts.fix_ci.fill(&FixCi {
            base: pull_request(first_update(*conflict)),
            failures: render_failures(prompts, failures),
        }),
        PullTask::Update(conflict) => prompts.update.fill(&pull_request(stopped(
            &prompts.update_rebase,
            &prompts.update_merge,
            repo,
            conflict,
        ))),
    }
}

/// Where `conflict`'s update stopped and how to finish it, from the part for
/// its method.
fn stopped(
    rebase: &Template<Stopped>,
    merge: &Template<Stopped>,
    repo: &Repo,
    conflict: &Conflict,
) -> String {
    let part = match conflict.method {
        UpdateMethod::Rebase => rebase,
        UpdateMethod::Merge => merge,
    };
    part.fill(&Stopped {
        default_branch: repo.default_branch.into(),
        files: conflict
            .files
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", "),
    })
}

/// The sentence `part` says of the `unfinished` commits, led by a space to
/// follow the one before it, or nothing.
fn unfinished_sentence(part: &Template<Commits>, unfinished: Option<&UnfinishedCommits>) -> String {
    unfinished
        .map(|u| {
            format!(
                " {}",
                part.fill(&Commits {
                    commits: unfinished_commits(u)
                })
            )
        })
        .unwrap_or_default()
}

pub fn automation(prompts: &Prompts, repo: &Repo, automation: &Automation) -> String {
    prompts.automation.fill(&AutomationRun {
        base: Facts::of(repo),
        max_issues: automation.max_issues.to_string(),
        name: automation.name.to_string(),
        instructions: or_none(&automation.instructions).into(),
    })
}

pub fn parse_error(prompts: &Prompts, error: &str) -> String {
    prompts.parse_error.fill(&ParseError {
        error: error.into(),
    })
}

pub fn loose_ends(prompts: &Prompts, problem: &str) -> String {
    prompts.loose_ends.fill(&LooseEnds {
        problem: problem.into(),
    })
}

/// Names the unfinished `commits`, as in "the unfinished commit `abc1234`".
pub fn unfinished_commits(commits: &UnfinishedCommits) -> String {
    let first = format!("`{}`", commits.first);
    let Some((last, middle)) = commits.rest.split_last() else {
        return format!("the unfinished commit {first}");
    };
    let leading: Vec<String> = std::iter::once(first)
        .chain(middle.iter().map(|c| format!("`{c}`")))
        .collect();
    format!("the unfinished commits {} and `{last}`", leading.join(", "))
}

/// What a `done` answer is sent back with while the branch still holds the
/// `unfinished` commits.
pub fn fold_unfinished(prompts: &Prompts, unfinished: &UnfinishedCommits) -> String {
    loose_ends(
        prompts,
        &prompts.fold_unfinished.fill(&Commits {
            commits: unfinished_commits(unfinished),
        }),
    )
}

/// What a `done` answer is sent back with while its commits hold a
/// credential, as `held` says.
pub fn held_credential(prompts: &Prompts, held: &Held) -> String {
    loose_ends(
        prompts,
        &prompts.held_credential.fill(&HeldCredential {
            commit: held.commit.clone(),
            variable: held.variable.clone(),
            file: held.file.clone(),
        }),
    )
}

/// What a `done` answer is sent back with while GitHub's push protection
/// refuses its push, finding `secrets`.
pub fn pushed_secret(prompts: &Prompts, secrets: &FoundSecrets) -> String {
    loose_ends(
        prompts,
        &prompts.pushed_secret.fill(&PushedSecret {
            secrets: secrets.to_string(),
        }),
    )
}

pub fn usage_limit_reset(prompts: &Prompts) -> String {
    prompts.usage_limit_reset.fill(&NoValues {})
}

pub fn late_feedback(prompts: &Prompts, pr: &Pull, feedback: &Feedback) -> String {
    let (feedback, conversation) = render_feedback(pr, feedback);
    prompts.late_feedback.fill(&LateFeedback {
        feedback,
        conversation,
    })
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

/// The ticket as a task shows it, without trailing whitespace.
fn render_ticket(prompts: &Prompts, ticket: &Ticket) -> String {
    let mut task = String::new();
    write_issue(&mut task, "The ticket", ticket.issue);
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
        let _ = writeln!(
            task,
            "#### The spec change's spec diff\n\n```diff\n{}\n```\n",
            spec_diff.trim_end()
        );
    }
    if !ticket.filed_gaps.is_empty() {
        let mut gaps = String::new();
        for gap in &ticket.filed_gaps {
            let _ = writeln!(
                gaps,
                "#### {} {}\n\n{}\n",
                gap.number,
                gap.title,
                gap.body.trim()
            );
        }
        task.push_str(&prompts.filed_gaps.fill(&FiledGaps {
            gaps: gaps.trim_end().into(),
        }));
    }
    task.trim_end().to_string()
}

fn render_failures(prompts: &Prompts, failures: &[CheckFailure]) -> String {
    let mut task = String::new();
    for failure in failures {
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
                "{}\n",
                prompts.full_log.fill(&FullLog {
                    job: full.job.to_string(),
                    repo: full.repo.clone(),
                })
            );
        }
    }
    task.trim_end().to_string()
}

/// The `feedback` to address and the pull request's conversation so far, each
/// without trailing whitespace.
fn render_feedback(pr: &Pull, feedback: &Feedback) -> (String, String) {
    let mut new = String::new();
    for comment in &feedback.comments {
        write_comment(&mut new, comment);
    }
    for review in &feedback.reviews {
        let _ = writeln!(
            new,
            "**Review by @{}** ({:?}, {}):\n\n{}\n",
            review.author,
            review.state,
            review.submitted_at.format("%Y-%m-%d %H:%M UTC"),
            or_none(&review.body)
        );
    }
    for (thread, comments) in &feedback.threads {
        let _ = writeln!(
            new,
            "**Thread `{}` on `{}`**, new comments:\n",
            thread.id, thread.path
        );
        for comment in comments {
            write_comment(&mut new, comment);
        }
    }
    let mut conversation = String::new();
    for comment in &pr.comments {
        write_comment(&mut conversation, comment);
    }
    for thread in pr.threads.iter().filter(|t| !t.resolved) {
        let _ = writeln!(
            conversation,
            "#### Open thread `{}` on `{}`\n",
            thread.id, thread.path
        );
        for comment in &thread.comments {
            write_comment(&mut conversation, comment);
        }
    }
    (
        new.trim_end().to_string(),
        conversation.trim_end().to_string(),
    )
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
