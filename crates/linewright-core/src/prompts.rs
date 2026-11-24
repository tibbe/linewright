use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt::{self, Write};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use anyhow::{Result, bail};

use crate::attachments::BadReference;
use crate::automation::Automation;
use crate::domain;
use crate::domain::{
    CheckFailure, Comment, Issue, MergeMethod, MergedPull, Number, Parent, Pull, UpdateMethod,
};
use crate::git::{self, BranchStart, Conflict, UnfinishedCommits};
use crate::reconcile::WorkKind;
use crate::results::{ContractError, Mismatch, ParseError};
use crate::secrets::Held;
use crate::spec_gap::FiledGaps;
use crate::store::FoundSecrets;
use crate::task::Criteria;
use crate::trial::scorer::TrialCopy;
use crate::work_settings::RunKind;
use crate::workflow_run::CompletedAttempt;
use crate::working_pull::Feedback;

mod list;

use list::{List, Placement};

pub struct Repo<'a> {
    pub name: &'a domain::Repo,
    pub default_branch: &'a str,
    pub spec_glob: &'a str,
}

pub struct Ticket<'a> {
    issue: &'a Issue,
    /// The issue it was cut from, whose comments hold its design
    /// (docs/specs/implementation.md, "What the implementer reads").
    parent: Option<Cow<'a, Issue>>,
    filed_gaps: Vec<&'a Issue>,
    /// Only for a spec change's issue or a ticket cut from one.
    spec_diff: Option<String>,
}

/// A ticket as read, which a spec change's issue, or a ticket cut from one,
/// is only once it has the spec change's spec diff.
pub enum TicketRead<'a> {
    Ready(Ticket<'a>),
    WantsSpecDiff(WantsSpecDiff<'a>),
}

/// A ticket that is a spec change's issue or was cut from one, without the
/// spec change's spec diff.
pub struct WantsSpecDiff<'a> {
    ticket: Ticket<'a>,
    spec_change: Number,
}

impl<'a> WantsSpecDiff<'a> {
    /// The spec change whose spec diff it wants.
    pub fn spec_change(&self) -> Number {
        self.spec_change
    }

    pub fn with(self, spec_diff: String) -> Ticket<'a> {
        Ticket {
            spec_diff: Some(spec_diff),
            ..self.ticket
        }
    }
}

impl<'a> Ticket<'a> {
    /// The ticket `issue` is, under `parent`, with `filed_gaps`, wanting its
    /// spec change's spec diff where it has one.
    pub fn read(
        issue: &'a Issue,
        parent: Option<Cow<'a, Issue>>,
        filed_gaps: Vec<&'a Issue>,
    ) -> TicketRead<'a> {
        let ticket = Ticket {
            issue,
            parent,
            filed_gaps,
            spec_diff: None,
        };
        match ticket.issue.spec_change().or(ticket.cut_from()) {
            None => TicketRead::Ready(ticket),
            Some(spec_change) => TicketRead::WantsSpecDiff(WantsSpecDiff {
                ticket,
                spec_change,
            }),
        }
    }

    /// The spec change whose issue is its parent, if it is one.
    fn cut_from(&self) -> Option<Number> {
        let parent = self.parent.as_deref()?;
        Parent::new(parent.number, parent.filed).spec_change
    }

    pub fn number(&self) -> Number {
        self.issue.number
    }

    pub fn filed_gaps(&self) -> FiledGaps<'_> {
        FiledGaps {
            item: self.issue.number,
            issues: &self.filed_gaps,
        }
    }
}

/// How a fix or a rework commits, for the merge method
/// (`docs/specs/pull_requests.md`, "Commits").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    /// The commits land as they are: a coherent series, corrections folded in.
    Series,
    /// The commits are squashed into one: new commits on top, none rewritten.
    OnTop,
}

impl History {
    /// The history for `method`: on top where it squashes, a series where
    /// the commits land as they are.
    pub fn for_method(method: MergeMethod) -> History {
        match method {
            MergeMethod::Merge | MergeMethod::Rebase => History::Series,
            MergeMethod::Squash => History::OnTop,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PullTask<'a> {
    /// `conflict` is where the update the run starts with stopped, if it did,
    /// and `history` how the session commits.
    Rework {
        feedback: Feedback<'a>,
        conflict: Option<&'a Conflict>,
        history: History,
    },
    FixCi {
        failures: &'a [CheckFailure],
        conflict: Option<&'a Conflict>,
        history: History,
    },
    /// Only bring the branch up to date: the session resolves the conflict
    /// the update stopped on.
    Update(&'a Conflict),
}

/// The values a template's placeholders are filled with.
trait Values {
    /// The placeholders, one per value.
    fn names() -> Vec<&'static str>;
    fn value(&self, name: &str) -> Option<Value<'_>>;
}

/// What a placeholder is filled with.
enum Value<'a> {
    Text(&'a str),
    List(&'a List),
}

/// Declares the values of a template: a field per placeholder, after those of
/// `base`, if given, which it holds as `base`. A field is text unless it is
/// declared a `List`, as in `items: List`.
macro_rules! values {
    (@type) => { String };
    (@type List) => { List };
    (@value $field:expr) => { Value::Text($field) };
    (@value $field:expr, List) => { Value::List($field) };
    ($(#[$doc:meta])* $name:ident $(: $base:ident)? { $($field:ident $(: $list:ident)?),* $(,)? }) => {
        $(#[$doc])*
        struct $name {
            $(base: $base,)?
            $($field: values!(@type $($list)?),)*
        }

        impl Values for $name {
            fn names() -> Vec<&'static str> {
                [$(<$base as Values>::names(),)? vec![$(stringify!($field)),*]].concat()
            }

            fn value(&self, name: &str) -> Option<Value<'_>> {
                match name {
                    $(stringify!($field) => Some(values!(@value &self.$field $(, $list)?)),)*
                    _ => None$(.or_else(|| <$base as Values>::value(&self.base, name)))?,
                }
            }
        }
    };
}

values!(
    /// The facts every first-message prompt shares, from `parts/facts.md`.
    Facts {
        repo,
        default_branch,
        spec_glob
    }
);
values!(
    /// The base of every first-message prompt.
    Shared { facts }
);
values!(Triage: Shared {
    categories: List,
    issue,
    output
});
values!(
    /// Triage's result contract, from `outputs/triage.md`.
    TriageOutput {
        category_choices: List,
        author
    }
);
values!(Design: Shared {
    kind,
    item,
    filed_gaps,
    cutting,
    output
});
values!(
    /// Design's result contract, from `outputs/design.md`.
    DesignOutput { kind }
);
values!(SpecChange {
    default_branch,
    number,
    title,
    author,
    body,
    spec_diff
});
values!(Implement: Shared {
    default_branch,
    branch,
    holding,
    ticket,
    output
});
values!(
    /// An update's, and the start of a rework's and a fix's.
    PullRequest: Shared {
        branch,
        pr,
        pr_title,
        unfinished,
        conflict,
        description,
        ticket,
        output
    }
);
values!(Rework: PullRequest {
    history,
    feedback,
    conversation
});
values!(FixCi: PullRequest { history, failures });
values!(AutomationRun: Shared {
    max_issues,
    name,
    instructions,
    trigger,
    output
});
values!(TriggerWorkflowRun {
    workflow,
    attempt,
    conclusion,
    branch,
    commit,
    link
});
values!(
    /// What the scorer of a trial is sent, from `scorer.md`.
    Scorer: Shared {
        stage,
        criteria,
        trial,
        output
    }
);
values!(
    /// Where the parts of a trial's output are, from
    /// `parts/trial-output.md`.
    TrialOutput {
        outcome,
        sessions,
        writes
    }
);
values!(DefaultBranch { default_branch });
values!(ExistingBranch {
    default_branch,
    unfinished
});
values!(Commits { commits: List });
values!(Stopped {
    default_branch,
    files: List
});
values!(FullLog { job, repo });
values!(FiledGapsPart { gaps });
values!(ResultError { error });
values!(LooseEnds { problem });
values!(HeldSecret {
    commit,
    variable,
    file
});
values!(PushedSecret { secrets: List });
values!(PushedSecretFound { kind, commit, path });
values!(
    /// The attachments contract, from `parts/attachments.md`.
    Attachments {
        directory,
        kinds: List,
        videos: List
    }
);
values!(AttachmentSecret { variable, file });
values!(AttachmentRefused { file, reason });
values!(AttachmentFile { file });
values!(AttachmentKind { file, kinds: List });
values!(SchemaMismatches { mismatches: List });
values!(MissingKey { key });
values!(Outcome {
    outcome,
    outcomes: List
});
values!(Outcomes { outcomes: List });
values!(At { at, error });
values!(Written { written });
values!(TicketId { id });
values!(Blocker { id, blocker });
values!(Category {
    category,
    categories: List
});
values!(Thread { thread });
values!(Status { status });
values!(NoValues {});
values!(LateFeedback {
    feedback,
    conversation
});

/// A prompt from `briefs/prompts/`, or a part of one from `briefs/parts/` or
/// `briefs/outputs/`: Markdown with a `{{name}}` placeholder for each of the
/// values `V`.
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

impl<V: Values> Template<V> {
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
    /// after it, so an optional paragraph leaves no gap. A list is a bullet
    /// line per item where its placeholder is alone on its line, ending in a
    /// line ending, and inline anywhere else.
    fn fill(&self, values: &V) -> String {
        let value = |name: &str| {
            values
                .value(name)
                .expect("a parsed template has only its values' placeholders")
        };
        let mut filled = String::with_capacity(self.text.len());
        let mut lines = self.text.split_inclusive('\n').peekable();
        while let Some(line) = lines.next() {
            if let Some((name, ending)) = alone(line) {
                match value(name) {
                    Value::Text("") => {
                        lines.next_if(|next| next.trim().is_empty());
                    }
                    Value::Text(text) => {
                        filled.push_str(text);
                        filled.push_str(ending);
                    }
                    Value::List(list) => {
                        filled.push_str(&list.render(Placement::Alone));
                        filled.push_str(if ending.is_empty() { "\n" } else { ending });
                    }
                }
                continue;
            }
            let mut rest = line;
            while let Some((start, name, end)) = placeholders(rest).next() {
                filled.push_str(&rest[..start]);
                match value(name) {
                    Value::Text(text) => filled.push_str(text),
                    Value::List(list) => filled.push_str(&list.render(Placement::Inline)),
                }
                rest = &rest[end..];
            }
            filled.push_str(rest);
        }
        filled
    }
}

/// Fails unless `text` has no placeholders.
pub fn without_placeholders(text: &str) -> Result<()> {
    Template::<NoValues>::parse(text.to_string()).map(drop)
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

/// The placeholder that is all of `line`, if one is, and the line's ending.
fn alone(line: &str) -> Option<(&str, &str)> {
    let text = line.trim_end_matches(['\n', '\r']);
    match placeholders(text).next() {
        Some((0, name, end)) if end == text.len() => Some((name, &line[end..])),
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

/// Declares [`Templates`] and [`TABLE`] from one row per file of the brief:
/// its field, its values, its path under `briefs/`, the runs that read it
/// and how its text ends.
macro_rules! templates {
    ($($(#[$attr:meta])* $field:ident: $values:ident = $path:literal, $readers:expr, $ending:ident;)*) => {
        /// Every file of the brief Linewright fills and sends sessions
        /// (docs/specs/operation.md, "Briefs").
        #[derive(Clone, Debug)]
        pub struct Templates {
            $($(#[$attr])* $field: Template<$values>,)*
        }

        /// Every file of [`Templates`], as Linewright ships it.
        const TABLE: &[File] = &[$(File {
            path: $path,
            readers: $readers,
            ending: Ending::$ending,
            shipped: include_str!(concat!("../../../briefs/", $path)),
        },)*];

        impl Templates {
            fn build(reader: &mut Reader, own: &Templates) -> Templates {
                Templates {
                    $($field: reader.read($path, &own.$field),)*
                }
            }

            /// The text of each file, by its path under `briefs/`, as it
            /// is filled: a file whose text is trimmed without its trailing
            /// white space.
            pub fn texts(&self) -> impl Iterator<Item = (&'static str, &str)> {
                [$(($path, self.$field.text.as_str()),)*].into_iter()
            }

            /// Every template from `text`'s text for its path and its
            /// values' names, or the paths whose text does not parse, with
            /// why.
            fn generate(
                text: &mut dyn FnMut(&'static str, &[&'static str]) -> String,
            ) -> Result<Templates, Vec<(&'static str, anyhow::Error)>> {
                let mut failed = Vec::new();
                $(let $field = generated::<$values>($path, text, &mut failed);)*
                if !failed.is_empty() {
                    return Err(failed);
                }
                Ok(Templates {
                    $($field: $field.expect("only a failed template is missing"),)*
                })
            }
        }
    };
}

templates! {
    triage: Triage = "prompts/triage.md", TRIAGE, AsWritten;
    design: Design = "prompts/design.md", DESIGN, AsWritten;
    implement: Implement = "prompts/implement.md", IMPLEMENT, AsWritten;
    rework: Rework = "prompts/rework.md", REWORK, AsWritten;
    fix_ci: FixCi = "prompts/fix-ci.md", FIX_CI, AsWritten;
    update: PullRequest = "prompts/update.md", UPDATE, AsWritten;
    automation: AutomationRun = "prompts/automation.md", AUTOMATIONS, AsWritten;
    scorer: Scorer = "prompts/scorer.md", SCORER, Trimmed;
    result_error: ResultError = "prompts/follow-ups/result-error.md", Readers::Every, Trimmed;
    loose_ends: LooseEnds = "prompts/follow-ups/loose-ends.md", ATTACHING, Trimmed;
    usage_limit_reset: NoValues = "prompts/follow-ups/usage-limit-reset.md", Readers::Every, Trimmed;
    late_feedback: LateFeedback = "prompts/follow-ups/late-feedback.md", PULL_WORK, Trimmed;
    facts: Facts = "parts/facts.md", Readers::Every, Trimmed;
    spec_change: SpecChange = "parts/spec-change.md", DESIGN, Trimmed;
    cut_issue: NoValues = "parts/cut-issue.md", DESIGN, Trimmed;
    new_branch: DefaultBranch = "parts/new-branch.md", IMPLEMENT, Trimmed;
    existing_branch: ExistingBranch = "parts/existing-branch.md", IMPLEMENT, Trimmed;
    unfinished: Commits = "parts/unfinished.md", PULL_WORK, Trimmed;
    unfinished_or_discard: Commits = "parts/unfinished-or-discard.md", IMPLEMENT, Trimmed;
    conflict_rebase: Stopped = "parts/conflict-rebase.md", FIXES, Trimmed;
    conflict_merge: Stopped = "parts/conflict-merge.md", FIXES, Trimmed;
    history_series: NoValues = "parts/history-series.md", FIXES, Trimmed;
    history_on_top: NoValues = "parts/history-on-top.md", FIXES, Trimmed;
    update_rebase: Stopped = "parts/update-rebase.md", UPDATE, Trimmed;
    update_merge: Stopped = "parts/update-merge.md", UPDATE, Trimmed;
    full_log: FullLog = "parts/full-log.md", FIX_CI, Trimmed;
    trigger_workflow_run: TriggerWorkflowRun = "parts/trigger-workflow-run.md", AUTOMATIONS, Trimmed;
    filed_gaps: FiledGapsPart = "parts/filed-gaps.md", TICKET_WORK, Trimmed;
    fold_unfinished: Commits = "parts/fold-unfinished.md", BRANCH_WORK, Trimmed;
    held_secret: HeldSecret = "parts/held-credential.md", BRANCH_WORK, Trimmed;
    pushed_secret: PushedSecret = "parts/pushed-secret.md", BRANCH_WORK, Trimmed;
    pushed_secret_found: PushedSecretFound = "parts/pushed-secret-found.md", BRANCH_WORK, Trimmed;
    attachments: Attachments = "parts/attachments.md", ATTACHING, Trimmed;
    attachment_secret: AttachmentSecret = "parts/attachment-secret.md", ATTACHING, Trimmed;
    attachment_refused: AttachmentRefused = "parts/attachment-refused.md", ATTACHING, Trimmed;
    trial_output: TrialOutput = "parts/trial-output.md", SCORER, Trimmed;
    result_no_json: NoValues = "parts/result-no-json.md", Readers::Every, Trimmed;
    result_not_json: ResultError = "parts/result-not-json.md", Readers::Every, Trimmed;
    result_schema: SchemaMismatches = "parts/result-schema.md", Readers::Every, Trimmed;
    result_missing: MissingKey = "parts/result-missing.md", Readers::Every, Trimmed;
    result_outcome: Outcome = "parts/result-outcome.md", OUTCOMES, Trimmed;
    result_outcome_missing: Outcomes = "parts/result-outcome-missing.md", OUTCOMES, Trimmed;
    result_at: At = "parts/result-at.md", Readers::Every, Trimmed;
    result_whole: ResultError = "parts/result-whole.md", Readers::Every, Trimmed;
    result_contract: ResultError = "parts/result-contract.md", Readers::Every, Trimmed;
    result_login: Written = "parts/result-login.md", TRIAGE, Trimmed;
    result_ticket_id: TicketId = "parts/result-ticket-id.md", DESIGN, Trimmed;
    result_ticket_blocker: Blocker = "parts/result-ticket-blocker.md", DESIGN, Trimmed;
    result_ticket_cycle: NoValues = "parts/result-ticket-cycle.md", DESIGN, Trimmed;
    result_category: Category = "parts/result-category.md", TRIAGE, Trimmed;
    result_thread: Thread = "parts/result-thread.md", PULL_WORK, Trimmed;
    attachment_not_a_file: AttachmentFile = "parts/attachment-not-a-file.md", ATTACHING, Trimmed;
    attachment_outside: AttachmentFile = "parts/attachment-outside.md", ATTACHING, Trimmed;
    attachment_invalid_url: AttachmentFile = "parts/attachment-invalid-url.md", ATTACHING, Trimmed;
    attachment_kind: AttachmentKind = "parts/attachment-kind.md", ATTACHING, Trimmed;
    rebase_in_progress: NoValues = "parts/rebase-in-progress.md", BRANCH_WORK, Trimmed;
    merge_in_progress: NoValues = "parts/merge-in-progress.md", BRANCH_WORK, Trimmed;
    uncommitted: Status = "parts/uncommitted.md", BRANCH_WORK, Trimmed;
    no_commits: NoValues = "parts/no-commits.md", IMPLEMENT, Trimmed;
    triage_output: TriageOutput = "outputs/triage.md", TRIAGE, Trimmed;
    design_output: DesignOutput = "outputs/design.md", DESIGN, Trimmed;
    implement_output: NoValues = "outputs/implement.md", IMPLEMENT, Trimmed;
    rework_output: NoValues = "outputs/rework.md", REWORK, Trimmed;
    fix_ci_output: NoValues = "outputs/fix-ci.md", FIX_CI, Trimmed;
    update_output: NoValues = "outputs/update.md", UPDATE, Trimmed;
    automation_output: NoValues = "outputs/automation.md", AUTOMATIONS, Trimmed;
    scorer_output: NoValues = "outputs/scorer.md", SCORER, Trimmed;
}

/// A file of the brief: its path under `briefs/`, the runs that read it, how
/// its text ends and its text as Linewright ships it.
struct File {
    path: &'static str,
    readers: Readers,
    ending: Ending,
    shipped: &'static str,
}

/// How much of a file's text is used.
#[derive(Clone, Copy)]
enum Ending {
    /// All of it: a stage's or an automation's first message, as written.
    AsWritten,
    /// All but its trailing newline, as it is filled into another or follows
    /// a first message.
    Trimmed,
}

impl File {
    /// `text` as this file's template, ending as the file does.
    fn parse<V: Values>(&self, text: &str) -> Result<Template<V>> {
        let text = match self.ending {
            Ending::AsWritten => text,
            Ending::Trimmed => text.trim_end(),
        };
        Template::parse(text.to_string())
    }
}

/// Linewright's own text of every file of [`Templates`], each of which
/// parses with its values: the layer under all others.
#[derive(Clone, Debug)]
pub struct OwnTemplates(Arc<Templates>);

impl OwnTemplates {
    /// The text Linewright ships, in `briefs/`.
    pub fn shipped() -> OwnTemplates {
        static SHIPPED: LazyLock<OwnTemplates> = LazyLock::new(|| {
            OwnTemplates::generate(|path, _| {
                file(path)
                    .expect("every template is in the table")
                    .shipped
                    .to_string()
            })
            .expect("the shipped templates have their values' placeholders")
        });
        SHIPPED.clone()
    }

    /// Each file's text from `text`, given its path under `briefs/` and the
    /// names of its placeholders in its values' order. Fails naming each file
    /// whose text does not parse.
    pub fn generate(
        mut text: impl FnMut(&'static str, &[&'static str]) -> String,
    ) -> Result<OwnTemplates> {
        match Templates::generate(&mut text) {
            Ok(templates) => Ok(OwnTemplates(Arc::new(templates))),
            Err(failed) => {
                let named: Vec<String> = failed
                    .iter()
                    .map(|(path, error)| format!("- {path}: {error:#}"))
                    .collect();
                bail!(
                    "the text of these files does not parse:\n{}",
                    named.join("\n")
                )
            }
        }
    }
}

/// The template at `path` from `text`, or `None`, its failure added to
/// `failed`, where it does not parse.
fn generated<V: Values>(
    path: &'static str,
    text: &mut dyn FnMut(&'static str, &[&'static str]) -> String,
    failed: &mut Vec<(&'static str, anyhow::Error)>,
) -> Option<Template<V>> {
    let file = file(path).expect("every template is in the table");
    match file.parse(&text(path, &V::names())) {
        Ok(template) => Some(template),
        Err(error) => {
            failed.push((path, error));
            None
        }
    }
}

impl Templates {
    /// The templates, each from `lookup`'s text for its path under
    /// `briefs/`, or `own`'s where it has none, and the paths whose text
    /// does not parse, with why, each of which takes `own`'s.
    pub fn read<'a>(
        own: &OwnTemplates,
        lookup: &mut dyn FnMut(&'static str) -> Option<&'a str>,
    ) -> (Templates, Vec<(&'static str, anyhow::Error)>) {
        let mut reader = Reader {
            lookup,
            unusable: Vec::new(),
        };
        let templates = Templates::build(&mut reader, &own.0);
        (templates, reader.unusable)
    }

    /// The path under `briefs/` of every template.
    pub fn paths() -> impl Iterator<Item = &'static str> {
        TABLE.iter().map(|file| file.path)
    }
}

/// The runs the template at `path` under `briefs/` is sent to, or `None` for
/// a path that is no template's: the table that says which runs a file holds
/// (docs/specs/repository.md, "Linewright's files").
pub fn readers(path: &str) -> Option<Readers> {
    file(path).map(|file| file.readers)
}

/// The file of [`TABLE`] at `path` under `briefs/`, if there is one.
fn file(path: &str) -> Option<&'static File> {
    TABLE.iter().find(|file| file.path == path)
}

/// Which runs a brief file is sent to, and so which runs a file that cannot
/// be used holds (docs/specs/repository.md, "Linewright's files").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readers {
    Every,
    Only(&'static [Reads]),
}

/// A kind of run that reads a brief file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    Stage(WorkKind),
    Automations,
    /// The scorer of a trial (docs/specs/benchmarks.md, "Scoring").
    Scorer,
}

impl Readers {
    pub fn read_by(self, run: &RunKind) -> bool {
        match self {
            Readers::Every => true,
            Readers::Only(reads) => reads.iter().any(|reads| match (reads, run) {
                (Reads::Stage(kind), RunKind::Stage(run)) => kind == run,
                (Reads::Automations, RunKind::Automation(_)) => true,
                (Reads::Scorer, RunKind::Scorer) => true,
                _ => false,
            }),
        }
    }
}

const TRIAGE: Readers = Readers::Only(&[Reads::Stage(WorkKind::Triage)]);
const DESIGN: Readers = Readers::Only(&[Reads::Stage(WorkKind::Design)]);
const IMPLEMENT: Readers = Readers::Only(&[Reads::Stage(WorkKind::Implement)]);
const REWORK: Readers = Readers::Only(&[Reads::Stage(WorkKind::Rework)]);
const FIX_CI: Readers = Readers::Only(&[Reads::Stage(WorkKind::FixCi)]);
const UPDATE: Readers = Readers::Only(&[Reads::Stage(WorkKind::Update)]);
const AUTOMATIONS: Readers = Readers::Only(&[Reads::Automations]);
const SCORER: Readers = Readers::Only(&[Reads::Scorer]);
/// The runs that rework a pull request or fix its CI.
const FIXES: Readers = Readers::Only(&[
    Reads::Stage(WorkKind::Rework),
    Reads::Stage(WorkKind::FixCi),
]);
const PULL_WORK: Readers = Readers::Only(&[
    Reads::Stage(WorkKind::Rework),
    Reads::Stage(WorkKind::FixCi),
    Reads::Stage(WorkKind::Update),
]);
/// The runs that work on a ticket's branch.
const BRANCH_WORK: Readers = Readers::Only(&[
    Reads::Stage(WorkKind::Implement),
    Reads::Stage(WorkKind::Rework),
    Reads::Stage(WorkKind::FixCi),
    Reads::Stage(WorkKind::Update),
]);
/// The runs that are sent the spec gaps filed for their item: design's, and
/// those that work on a ticket's branch.
const TICKET_WORK: Readers = Readers::Only(&[
    Reads::Stage(WorkKind::Design),
    Reads::Stage(WorkKind::Implement),
    Reads::Stage(WorkKind::Rework),
    Reads::Stage(WorkKind::FixCi),
    Reads::Stage(WorkKind::Update),
]);
/// The runs whose result schema has a branch for each outcome, which a
/// result with none of them is sent back naming.
const OUTCOMES: Readers = Readers::Only(&[
    Reads::Stage(WorkKind::Design),
    Reads::Stage(WorkKind::Implement),
    Reads::Stage(WorkKind::Rework),
    Reads::Stage(WorkKind::FixCi),
    Reads::Stage(WorkKind::Update),
    Reads::Scorer,
]);
/// The runs whose result may attach files: every stage's and every
/// automation's.
const ATTACHING: Readers = Readers::Only(&[
    Reads::Stage(WorkKind::Triage),
    Reads::Stage(WorkKind::Design),
    Reads::Stage(WorkKind::Implement),
    Reads::Stage(WorkKind::Rework),
    Reads::Stage(WorkKind::FixCi),
    Reads::Stage(WorkKind::Update),
    Reads::Automations,
]);

/// The templates read from a lookup, and the paths whose text could not be
/// used, with why.
struct Reader<'l, 'a> {
    lookup: &'l mut dyn FnMut(&'static str) -> Option<&'a str>,
    unusable: Vec<(&'static str, anyhow::Error)>,
}

impl Reader<'_, '_> {
    /// The template at `path` under `briefs/`, one of [`TABLE`]'s, or `own`
    /// where the lookup has none that parses.
    fn read<V: Values>(&mut self, path: &'static str, own: &Template<V>) -> Template<V> {
        let file = file(path).expect("every template is in the table");
        if let Some(text) = (self.lookup)(path) {
            match file.parse(text) {
                Ok(template) => return template,
                Err(error) => self.unusable.push((path, error)),
            }
        }
        own.clone()
    }
}

/// The values every first-message prompt shares, its facts filled from
/// `parts/facts.md`.
fn shared(templates: &Templates, repo: &Repo) -> Shared {
    Shared {
        facts: templates.facts.fill(&Facts {
            repo: repo.name.to_string(),
            default_branch: repo.default_branch.into(),
            spec_glob: repo.spec_glob.into(),
        }),
    }
}

pub fn triage(
    templates: &Templates,
    repo: &Repo,
    issue: &Issue,
    categories: &[String],
    attachments: &Path,
) -> String {
    let mut rendered = String::new();
    write_issue(&mut rendered, "The issue", issue);
    templates.triage.fill(&Triage {
        base: shared(templates, repo),
        categories: listed(categories, code, CATEGORIES_NEVER_EMPTY),
        issue: rendered.trim_end().into(),
        output: with_attachments(
            templates,
            &templates.triage_output.fill(&TriageOutput {
                category_choices: listed(categories, str::to_string, CATEGORIES_NEVER_EMPTY),
                author: issue.author.to_string(),
            }),
            attachments,
        ),
    })
}

/// What a design is of, as its prompt and its hand-backs name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesignOf {
    SpecChange,
    Issue,
}

impl DesignOf {
    pub fn noun(self) -> &'static str {
        match self {
            DesignOf::SpecChange => "spec change",
            DesignOf::Issue => "issue",
        }
    }
}

/// A spec change, merged, and its spec diff.
pub struct SpecChangeAndDiff {
    pub spec_change: MergedPull,
    pub diff: String,
}

/// The design prompt, with the issue and its comments as what the item
/// asks, followed for a spec change's issue by its `spec_change`
/// (docs/specs/design.md, "The session").
pub fn design_issue(
    templates: &Templates,
    repo: &Repo,
    issue: &Issue,
    spec_change: Option<&SpecChangeAndDiff>,
    filed_gaps: &[&Issue],
    attachments: &Path,
) -> String {
    let mut item = String::new();
    write_issue(&mut item, "The issue", issue);
    let of = match spec_change {
        None => DesignOf::Issue,
        Some(spec_change) => {
            item.push_str(&render_spec_change(
                templates,
                repo,
                &spec_change.spec_change,
                &spec_change.diff,
            ));
            DesignOf::SpecChange
        }
    };
    templates.design.fill(&Design {
        base: shared(templates, repo),
        kind: of.noun().into(),
        item: item.trim_end().into(),
        filed_gaps: render_filed_gaps(templates, filed_gaps),
        cutting: templates.cut_issue.fill(&NoValues {}),
        output: design_output(templates, of, attachments),
    })
}

fn render_spec_change(
    templates: &Templates,
    repo: &Repo,
    spec_change: &MergedPull,
    diff: &str,
) -> String {
    templates.spec_change.fill(&SpecChange {
        default_branch: repo.default_branch.into(),
        number: spec_change.number.to_string(),
        title: spec_change.title.clone(),
        author: spec_change.author.to_string(),
        body: or_none(&spec_change.body).into(),
        spec_diff: diff.trim_end().into(),
    })
}

fn design_output(templates: &Templates, of: DesignOf, attachments: &Path) -> String {
    with_attachments(
        templates,
        &templates.design_output.fill(&DesignOutput {
            kind: of.noun().into(),
        }),
        attachments,
    )
}

/// A stage's result contract `output`, followed by the attachments
/// contract naming the session's `attachments` directory and the kinds of
/// file Linewright attaches (docs/specs/runs.md, "Every prompt").
fn with_attachments(templates: &Templates, output: &str, attachments: &Path) -> String {
    let contract = templates.attachments.fill(&Attachments {
        directory: attachments.display().to_string(),
        kinds: extensions(crate::attachments::attachable()),
        videos: extensions(crate::attachments::videos()),
    });
    format!("{output}\n\n{contract}")
}

pub fn implement(
    templates: &Templates,
    repo: &Repo,
    ticket: &Ticket,
    branch: &str,
    start: &BranchStart,
    attachments: &Path,
) -> String {
    let holding = match start {
        BranchStart::New => templates.new_branch.fill(&DefaultBranch {
            default_branch: repo.default_branch.into(),
        }),
        BranchStart::Existing { unfinished } => templates.existing_branch.fill(&ExistingBranch {
            default_branch: repo.default_branch.into(),
            unfinished: unfinished_sentence(&templates.unfinished_or_discard, unfinished.as_ref()),
        }),
    };
    templates.implement.fill(&Implement {
        base: shared(templates, repo),
        default_branch: repo.default_branch.into(),
        branch: branch.into(),
        holding,
        ticket: render_ticket(templates, ticket),
        output: with_attachments(
            templates,
            &templates.implement_output.fill(&NoValues {}),
            attachments,
        ),
    })
}

pub fn pull_work(
    templates: &Templates,
    repo: &Repo,
    ticket: &Ticket,
    pr: &Pull,
    work: &PullTask,
    unfinished: Option<&UnfinishedCommits>,
    attachments: &Path,
) -> String {
    let pull_request = |conflict: String, output: &Template<NoValues>| PullRequest {
        base: shared(templates, repo),
        branch: pr.head_ref.clone(),
        pr: pr.number.to_string(),
        pr_title: pr.title.clone(),
        unfinished: unfinished_sentence(&templates.unfinished, unfinished),
        conflict,
        description: or_none(&pr.body).into(),
        ticket: render_ticket(templates, ticket),
        output: with_attachments(templates, &output.fill(&NoValues {}), attachments),
    };
    let first_update = |conflict: Option<&Conflict>| {
        conflict
            .map(|c| {
                stopped(
                    &templates.conflict_rebase,
                    &templates.conflict_merge,
                    repo,
                    c,
                )
            })
            .unwrap_or_default()
    };
    let history = |history: &History| {
        match history {
            History::Series => &templates.history_series,
            History::OnTop => &templates.history_on_top,
        }
        .fill(&NoValues {})
    };
    match work {
        PullTask::Rework {
            feedback,
            conflict,
            history: how,
        } => {
            let (feedback, conversation) = render_feedback(pr, feedback);
            templates.rework.fill(&Rework {
                base: pull_request(first_update(*conflict), &templates.rework_output),
                history: history(how),
                feedback,
                conversation,
            })
        }
        PullTask::FixCi {
            failures,
            conflict,
            history: how,
        } => templates.fix_ci.fill(&FixCi {
            base: pull_request(first_update(*conflict), &templates.fix_ci_output),
            history: history(how),
            failures: render_failures(templates, failures),
        }),
        PullTask::Update(conflict) => templates.update.fill(&pull_request(
            stopped(
                &templates.update_rebase,
                &templates.update_merge,
                repo,
                conflict,
            ),
            &templates.update_output,
        )),
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
        files: listed(
            &conflict.files,
            code,
            "an update stops on conflicts in at least one file",
        ),
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

/// The attempt of a workflow run that started an automation's run
/// (`docs/specs/automations.md`, "Workflow runs").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkflowRunAttempt<'a> {
    /// The name of the workflow run's workflow.
    pub workflow: &'a str,
    pub attempt: u32,
    /// The conclusion GitHub gives the attempt, such as `failure`.
    pub conclusion: &'a str,
    /// The workflow run's head branch, which a run for a fork's commit lacks.
    pub branch: Option<&'a str>,
    pub commit: &'a str,
    pub link: &'a str,
}

impl<'a> From<&'a CompletedAttempt> for WorkflowRunAttempt<'a> {
    fn from(attempt: &'a CompletedAttempt) -> WorkflowRunAttempt<'a> {
        WorkflowRunAttempt {
            workflow: &attempt.workflow,
            attempt: attempt.id.attempt.get(),
            conclusion: attempt.conclusion.wire(),
            branch: attempt.branch.as_deref(),
            commit: &attempt.commit,
            link: &attempt.link,
        }
    }
}

/// The prompt of a run of `automation`, which `trigger` started unless its
/// schedule or a person did.
pub fn automation(
    templates: &Templates,
    repo: &Repo,
    automation: &Automation,
    trigger: Option<WorkflowRunAttempt>,
    attachments: &Path,
) -> String {
    let trigger = trigger.map(|attempt| {
        templates.trigger_workflow_run.fill(&TriggerWorkflowRun {
            workflow: attempt.workflow.into(),
            attempt: attempt.attempt.to_string(),
            conclusion: attempt.conclusion.into(),
            branch: attempt
                .branch
                .map_or_else(|| "none".into(), |branch| format!("`{branch}`")),
            commit: attempt.commit.into(),
            link: attempt.link.into(),
        })
    });
    templates.automation.fill(&AutomationRun {
        base: shared(templates, repo),
        max_issues: automation.max_issues.to_string(),
        name: automation.name.to_string(),
        instructions: or_none(&automation.instructions).into(),
        trigger: trigger.unwrap_or_default(),
        output: with_attachments(
            templates,
            &templates.automation_output.fill(&NoValues {}),
            attachments,
        ),
    })
}

/// What the scorer of a trial of a task of `stage` in `repo` is sent: the
/// task's `criteria`, and where the parts of the trial's output are in the
/// scorer's copy `trial` (`docs/specs/benchmarks.md`, Scoring).
/// Its result contract has nothing on attachments.
pub fn scorer(
    templates: &Templates,
    repo: &Repo,
    stage: WorkKind,
    criteria: &Criteria,
    trial: &TrialCopy,
) -> String {
    templates.scorer.fill(&Scorer {
        base: shared(templates, repo),
        stage: stage.name().into(),
        criteria: criteria.as_str().trim_end().into(),
        trial: templates.trial_output.fill(&TrialOutput {
            outcome: trial.outcome().display().to_string(),
            sessions: trial.sessions().display().to_string(),
            writes: trial.writes().display().to_string(),
        }),
        output: templates.scorer_output.fill(&NoValues {}),
    })
}

/// Why a result is sent back to its session to be answered again
/// (docs/specs/runs.md, "Contract").
#[derive(Debug)]
pub enum SentBack<'a> {
    /// The final message holds no result Linewright can use.
    Result(&'a ParseError),
    /// The result refers to no file Linewright can attach.
    Reference(&'a BadReference),
    /// GitHub refuses the attachment `file` for `reason`.
    AttachmentRefused { file: &'a Path, reason: &'a str },
    /// A reply is to `thread`, which is not one of the pull request's.
    UnknownThread { thread: &'a str },
}

/// What is still wrong with a session's work when it answers `done`
/// (docs/specs/runs.md, "Contract").
#[derive(Debug)]
pub enum LooseEnd<'a> {
    /// A rebase or merge in progress, or uncommitted changes.
    Git(&'a git::LooseEnd),
    /// The branch has no commits.
    NoCommits,
    /// The branch holds unfinished commits.
    Unfinished(&'a UnfinishedCommits),
    /// Its commits hold a secret, as `held` says.
    HeldSecret(&'a Held),
    /// GitHub's push protection refuses its push, finding the secrets.
    PushedSecret(&'a FoundSecrets),
    /// The attachment that `reference`, as its syntax reads it, refers to
    /// holds the secret of `variable`.
    AttachmentSecret {
        variable: &'a str,
        reference: &'a str,
    },
}

/// What a session is sent back with when its result can't be used, for
/// the reason `sent_back`.
pub fn result_error(templates: &Templates, sent_back: &SentBack) -> String {
    let error = match sent_back {
        SentBack::Result(error) => parse_error(templates, error),
        SentBack::Reference(reference) => bad_reference(templates, reference),
        SentBack::AttachmentRefused { file, reason } => {
            templates.attachment_refused.fill(&AttachmentRefused {
                file: file.display().to_string(),
                reason: (*reason).into(),
            })
        }
        SentBack::UnknownThread { thread } => templates.result_thread.fill(&Thread {
            thread: (*thread).into(),
        }),
    };
    templates.result_error.fill(&ResultError { error })
}

fn parse_error(templates: &Templates, error: &ParseError) -> String {
    match error {
        ParseError::NoJson => templates.result_no_json.fill(&NoValues {}),
        ParseError::NotJson { error } => templates.result_not_json.fill(&ResultError {
            error: error.clone(),
        }),
        ParseError::Schema(mismatches) => templates.result_schema.fill(&SchemaMismatches {
            mismatches: {
                let (first, rest) = mismatches.split();
                List::new(
                    mismatch(templates, first),
                    rest.iter().map(|m| mismatch(templates, m)),
                )
            },
        }),
        ParseError::Contract(error) => templates.result_contract.fill(&ResultError {
            error: contract_error(templates, error),
        }),
        ParseError::Category {
            category,
            categories,
        } => templates.result_category.fill(&Category {
            category: format!("{category:?}"),
            categories: listed(categories, quoted, CATEGORIES_NEVER_EMPTY),
        }),
    }
}

fn mismatch(templates: &Templates, mismatch: &Mismatch) -> String {
    match mismatch {
        Mismatch::Missing { key } => templates
            .result_missing
            .fill(&MissingKey { key: key.clone() }),
        Mismatch::Outcome {
            outcome: Some(outcome),
            outcomes,
        } => templates.result_outcome.fill(&Outcome {
            outcome: outcome.to_string(),
            outcomes: listed(outcomes, quoted, OUTCOMES_NEVER_EMPTY),
        }),
        Mismatch::Outcome {
            outcome: None,
            outcomes,
        } => templates.result_outcome_missing.fill(&Outcomes {
            outcomes: listed(outcomes, quoted, OUTCOMES_NEVER_EMPTY),
        }),
        Mismatch::At { at, error } => templates.result_at.fill(&At {
            at: at.clone(),
            error: error.clone(),
        }),
        Mismatch::Whole { error } => templates.result_whole.fill(&ResultError {
            error: error.clone(),
        }),
    }
}

fn contract_error(templates: &Templates, error: &ContractError) -> String {
    match error {
        ContractError::Serde { error } => error.clone(),
        ContractError::Login { written } => templates.result_login.fill(&Written {
            written: format!("{written:?}"),
        }),
        ContractError::DuplicateId { id } => templates.result_ticket_id.fill(&TicketId {
            id: format!("{id:?}"),
        }),
        ContractError::UnknownBlocker { id, blocker } => {
            templates.result_ticket_blocker.fill(&Blocker {
                id: format!("{id:?}"),
                blocker: format!("{blocker:?}"),
            })
        }
        ContractError::Cycle => templates.result_ticket_cycle.fill(&NoValues {}),
    }
}

fn bad_reference(templates: &Templates, reference: &BadReference) -> String {
    match reference {
        BadReference::InvalidUrl { reference } => {
            templates.attachment_invalid_url.fill(&AttachmentFile {
                file: reference.clone(),
            })
        }
        BadReference::NotAFile { reference } => {
            templates.attachment_not_a_file.fill(&AttachmentFile {
                file: reference.clone(),
            })
        }
        BadReference::Outside { reference } => templates.attachment_outside.fill(&AttachmentFile {
            file: reference.clone(),
        }),
        BadReference::Kind { reference } => templates.attachment_kind.fill(&AttachmentKind {
            file: reference.clone(),
            kinds: extensions(crate::attachments::attachable()),
        }),
    }
}

/// Why a result's outcomes, the branches of its schema's `oneOf`, are never
/// empty.
const OUTCOMES_NEVER_EMPTY: &str = "a result that names none of its outcomes has a `oneOf` of them";

/// Why a repository's categories are never empty.
const CATEGORIES_NEVER_EMPTY: &str =
    "the categories are the enabled issue types, or the labels where there are none";

/// Each of `values` as `as_item` writes it, in a list.
///
/// # Panics
///
/// When `values` is empty, with `never_empty`, why they never are.
fn listed(values: &[String], as_item: fn(&str) -> String, never_empty: &str) -> List {
    List::of(values.iter().map(|v| as_item(v))).expect(never_empty)
}

/// The `first` and `rest` extensions, each with its dot as code, in a list,
/// as in "`.mp4`, `.mov`, `.webm`".
fn extensions((first, rest): (&str, impl Iterator<Item = &'static str>)) -> List {
    let as_item = |extension: &str| format!("`.{extension}`");
    List::new(as_item(first), rest.map(as_item))
}

/// `value` quoted, as in `"bug"`.
fn quoted(value: &str) -> String {
    format!("{value:?}")
}

/// `value` as code, as in `` `bug` ``.
fn code(value: &str) -> String {
    format!("`{value}`")
}

/// What a session is sent back with when it answers `done` while its work
/// is not finished, for `problem`.
pub fn loose_ends(templates: &Templates, problem: &LooseEnd) -> String {
    let problem = match problem {
        LooseEnd::Git(git::LooseEnd::RebaseInProgress) => {
            templates.rebase_in_progress.fill(&NoValues {})
        }
        LooseEnd::Git(git::LooseEnd::MergeInProgress) => {
            templates.merge_in_progress.fill(&NoValues {})
        }
        LooseEnd::Git(git::LooseEnd::Uncommitted { status }) => {
            templates.uncommitted.fill(&Status {
                status: status.clone(),
            })
        }
        LooseEnd::NoCommits => templates.no_commits.fill(&NoValues {}),
        LooseEnd::Unfinished(unfinished) => templates.fold_unfinished.fill(&Commits {
            commits: unfinished_commits(unfinished),
        }),
        LooseEnd::HeldSecret(held) => templates.held_secret.fill(&HeldSecret {
            commit: held.commit.clone(),
            variable: held.variable.clone(),
            file: held.file.clone(),
        }),
        LooseEnd::PushedSecret(secrets) => templates.pushed_secret.fill(&PushedSecret {
            secrets: pushed_secrets(templates, secrets),
        }),
        LooseEnd::AttachmentSecret {
            variable,
            reference,
        } => templates.attachment_secret.fill(&AttachmentSecret {
            variable: (*variable).into(),
            file: (*reference).into(),
        }),
    };
    templates.loose_ends.fill(&LooseEnds { problem })
}

/// Each of the `secrets` push protection found, from
/// `parts/pushed-secret-found.md`, in a list.
fn pushed_secrets(templates: &Templates, secrets: &FoundSecrets) -> List {
    let found = secrets.iter().map(|secret| {
        templates.pushed_secret_found.fill(&PushedSecretFound {
            kind: secret.kind.clone(),
            commit: secret.commit.clone(),
            path: secret.path.clone(),
        })
    });
    List::of(found).expect("push protection found at least one secret")
}

/// The unfinished `commits`, each as code.
fn unfinished_commits(commits: &UnfinishedCommits) -> List {
    List::new(code(&commits.first), commits.rest.iter().map(|c| code(c)))
}

pub fn usage_limit_reset(templates: &Templates) -> String {
    templates.usage_limit_reset.fill(&NoValues {})
}

pub fn late_feedback(templates: &Templates, pr: &Pull, feedback: &Feedback) -> String {
    let (feedback, conversation) = render_feedback(pr, feedback);
    templates.late_feedback.fill(&LateFeedback {
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
    write_comments(task, "Comments", &issue.comments);
}

/// The `comments` under `heading`, or nothing when there are none.
fn write_comments(task: &mut String, heading: &str, comments: &[Comment]) {
    if comments.is_empty() {
        return;
    }
    let _ = writeln!(task, "#### {heading}\n");
    for comment in comments {
        write_comment(task, comment);
    }
}

/// The ticket as a task shows it, without trailing whitespace.
fn render_ticket(templates: &Templates, ticket: &Ticket) -> String {
    let mut task = String::new();
    write_issue(&mut task, "The ticket", ticket.issue);
    if let Some(parent) = ticket.parent.as_deref() {
        let _ = match ticket.cut_from() {
            Some(spec_change) => writeln!(
                task,
                "### Parent issue\n\n{}, the tracking issue of the spec change {spec_change}.\n",
                parent.number
            ),
            None => writeln!(task, "### Parent issue\n\n{}.\n", parent.number),
        };
        write_comments(&mut task, "The parent issue's comments", &parent.comments);
    }
    if let Some(spec_change) = ticket.issue.spec_change() {
        let _ = writeln!(
            task,
            "### The spec change\n\nThis issue was opened for the spec change {spec_change}.\n"
        );
    }
    if let Some(spec_diff) = &ticket.spec_diff {
        let _ = writeln!(
            task,
            "#### The spec change's spec diff\n\n```diff\n{}\n```\n",
            spec_diff.trim_end()
        );
    }
    task.push_str(&render_filed_gaps(templates, &ticket.filed_gaps));
    task.trim_end().to_string()
}

/// The spec gaps already filed for an item, or nothing when there are none.
fn render_filed_gaps(templates: &Templates, filed_gaps: &[&Issue]) -> String {
    if filed_gaps.is_empty() {
        return String::new();
    }
    let mut gaps = String::new();
    for gap in filed_gaps {
        let _ = writeln!(
            gaps,
            "#### {} {}\n\n{}\n",
            gap.number,
            gap.title,
            gap.body.trim()
        );
    }
    templates.filed_gaps.fill(&FiledGapsPart {
        gaps: gaps.trim_end().into(),
    })
}

fn render_failures(templates: &Templates, failures: &[CheckFailure]) -> String {
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
                templates.full_log.fill(&FullLog {
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

#[cfg(test)]
mod tests {
    use super::*;

    values!(Listed { title, items: List });

    fn filled(text: &str, items: &[&str]) -> String {
        let template = Template::<Listed>::parse(text.into()).unwrap();
        template.fill(&Listed {
            title: "Items".into(),
            items: List::of(items.iter().map(std::string::ToString::to_string)).unwrap(),
        })
    }

    #[test_log::test]
    fn a_list_within_a_line_is_its_one_item() {
        assert_eq!(filled("{{title}}: {{items}}.\n", &["`a`"]), "Items: `a`.\n");
    }

    #[test_log::test]
    fn a_list_within_a_line_is_its_items_separated_by_commas() {
        assert_eq!(
            filled("{{title}}: {{items}}.\n", &["`a`", "`b`", "`c`"]),
            "Items: `a`, `b`, `c`.\n"
        );
        assert_eq!(
            filled("{{title}}: {{items}}.\n", &["a", "b, c", "d"]),
            "Items: a, b, c, d.\n"
        );
    }

    #[test_log::test]
    fn a_list_alone_on_its_line_is_a_bullet_line_for_its_one_item() {
        assert_eq!(
            filled("{{title}}:\n\n{{items}}\n\nMore.\n", &["a"]),
            "Items:\n\n- a\n\nMore.\n"
        );
    }

    #[test_log::test]
    fn a_list_alone_on_its_line_is_a_bullet_line_per_item() {
        assert_eq!(
            filled("{{title}}:\n\n{{items}}\n\nMore.\n", &["a", "b, c"]),
            "Items:\n\n- a\n- b, c\n\nMore.\n"
        );
    }

    #[test_log::test]
    fn a_list_alone_on_the_last_line_ends_with_a_line_ending() {
        assert_eq!(
            filled("{{title}}:\n\n{{items}}", &["a", "b"]),
            "Items:\n\n- a\n- b\n"
        );
    }

    #[test_log::test]
    fn an_items_further_lines_are_indented_under_its_bullet() {
        assert_eq!(
            filled("{{title}}:\n{{items}}\n", &["a\nmore", "b"]),
            "Items:\n- a\n  more\n- b\n"
        );
    }

    #[test_log::test]
    fn an_items_own_line_ending_leaves_no_blank_line_under_its_bullet() {
        assert_eq!(
            filled("{{title}}:\n{{items}}\n", &["a:\n- b\n", "c"]),
            "Items:\n- a:\n  - b\n- c\n"
        );
    }
}
