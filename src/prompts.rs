use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt::{self, Write};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, bail};

use crate::automation::Automation;
use crate::domain;
use crate::domain::{CheckFailure, Comment, Issue, MergedPull, Number, Parent, Pull, UpdateMethod};
use crate::git::{BranchStart, Conflict, UnfinishedCommits};
use crate::reconcile::{Feedback, WorkKind};
use crate::secrets::Held;
use crate::spec_gap::FiledGaps;
use crate::store::FoundSecrets;
use crate::work_settings::RunKind;

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
    /// Only for a ticket cut from a spec change.
    spec_diff: Option<String>,
}

impl<'a> Ticket<'a> {
    /// # Panics
    ///
    /// When it has a spec diff but its parent is no spec change's tracking
    /// issue, or the other way round.
    pub fn new(
        issue: &'a Issue,
        parent: Option<Cow<'a, Issue>>,
        filed_gaps: Vec<&'a Issue>,
        spec_diff: Option<String>,
    ) -> Ticket<'a> {
        let ticket = Ticket {
            issue,
            parent,
            filed_gaps,
            spec_diff,
        };
        assert_eq!(
            ticket.spec_diff.is_some(),
            ticket.cut_from().is_some(),
            "a ticket has a spec diff exactly when it was cut from a spec change"
        );
        ticket
    }

    /// The spec change whose tracking issue is its parent, if it is one.
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
    categories,
    issue,
    output
});
values!(
    /// Triage's result contract, from `outputs/triage.md`.
    TriageOutput {
        category_choices,
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
    feedback,
    conversation
});
values!(FixCi: PullRequest { failures });
values!(AutomationRun: Shared {
    max_issues,
    name,
    instructions,
    output
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
values!(FiledGapsPart { gaps });
values!(ResultError { error });
values!(LooseEnds { problem });
values!(HeldSecret {
    commit,
    variable,
    file
});
values!(PushedSecret { secrets });
values!(
    /// The attachments contract, from `parts/attachments.md`.
    Attachments { directory }
);
values!(AttachmentSecret { variable, file });
values!(AttachmentRefused { file, reason });
values!(NoValues {});
values!(LateFeedback {
    feedback,
    conversation
});

/// A prompt from `briefs/prompts/`, or a part of one from `src/prompts/`:
/// Markdown with a `{{name}}` placeholder for each of the values `V`.
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

/// [`Reader::read`] for the template at `$path` under `briefs/prompts/`,
/// which Linewright ships, sent to the runs `$readers`.
macro_rules! template {
    ($reader:expr, $path:literal, $readers:expr) => {
        $reader.read(
            $path,
            $readers,
            include_str!(concat!("../briefs/prompts/", $path)),
        )
    };
}

/// Linewright's own template at `$path` under `src/prompts/`, without its
/// trailing newline.
macro_rules! own_template {
    ($path:literal) => {
        Template::parse(
            include_str!(concat!("prompts/", $path))
                .trim_end()
                .to_string(),
        )
        .expect(concat!(
            "src/prompts/",
            $path,
            " has its values' placeholders"
        ))
    };
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

/// The prompts Linewright sends sessions, each from the first layer that has
/// it (docs/specs/operation.md, "Prompts").
#[derive(Clone, Debug)]
pub struct Prompts {
    triage: Template<Triage>,
    design: Template<Design>,
    implement: Template<Implement>,
    rework: Template<Rework>,
    fix_ci: Template<FixCi>,
    update: Template<PullRequest>,
    automation: Template<AutomationRun>,
    result_error: Template<ResultError>,
    loose_ends: Template<LooseEnds>,
    usage_limit_reset: Template<NoValues>,
    late_feedback: Template<LateFeedback>,
}

/// The parts Linewright builds its prompts from, and each first-message
/// prompt's result contract, filled into its `{{output}}`: Linewright's own,
/// which no layer overrides (docs/specs/operation.md, "Prompts").
struct OwnTemplates {
    facts: Template<Facts>,
    spec_change: Template<SpecChange>,
    cut_spec_change: Template<NoValues>,
    cut_issue: Template<NoValues>,
    new_branch: Template<DefaultBranch>,
    existing_branch: Template<ExistingBranch>,
    unfinished: Template<Commits>,
    unfinished_or_discard: Template<Commits>,
    conflict_rebase: Template<Stopped>,
    conflict_merge: Template<Stopped>,
    update_rebase: Template<Stopped>,
    update_merge: Template<Stopped>,
    full_log: Template<FullLog>,
    filed_gaps: Template<FiledGapsPart>,
    fold_unfinished: Template<Commits>,
    held_secret: Template<HeldSecret>,
    pushed_secret: Template<PushedSecret>,
    attachments: Template<Attachments>,
    attachment_secret: Template<AttachmentSecret>,
    attachment_refused: Template<AttachmentRefused>,
    triage_output: Template<TriageOutput>,
    design_output: Template<DesignOutput>,
    implement_output: Template<NoValues>,
    rework_output: Template<NoValues>,
    fix_ci_output: Template<NoValues>,
    update_output: Template<NoValues>,
    automation_output: Template<NoValues>,
}

static OWN_TEMPLATES: LazyLock<OwnTemplates> = LazyLock::new(|| OwnTemplates {
    facts: own_template!("parts/facts.md"),
    spec_change: own_template!("parts/spec-change.md"),
    cut_spec_change: own_template!("parts/cut-spec-change.md"),
    cut_issue: own_template!("parts/cut-issue.md"),
    new_branch: own_template!("parts/new-branch.md"),
    existing_branch: own_template!("parts/existing-branch.md"),
    unfinished: own_template!("parts/unfinished.md"),
    unfinished_or_discard: own_template!("parts/unfinished-or-discard.md"),
    conflict_rebase: own_template!("parts/conflict-rebase.md"),
    conflict_merge: own_template!("parts/conflict-merge.md"),
    update_rebase: own_template!("parts/update-rebase.md"),
    update_merge: own_template!("parts/update-merge.md"),
    full_log: own_template!("parts/full-log.md"),
    filed_gaps: own_template!("parts/filed-gaps.md"),
    fold_unfinished: own_template!("parts/fold-unfinished.md"),
    held_secret: own_template!("parts/held-credential.md"),
    pushed_secret: own_template!("parts/pushed-secret.md"),
    attachments: own_template!("parts/attachments.md"),
    attachment_secret: own_template!("parts/attachment-secret.md"),
    attachment_refused: own_template!("parts/attachment-refused.md"),
    triage_output: own_template!("outputs/triage.md"),
    design_output: own_template!("outputs/design.md"),
    implement_output: own_template!("outputs/implement.md"),
    rework_output: own_template!("outputs/rework.md"),
    fix_ci_output: own_template!("outputs/fix-ci.md"),
    update_output: own_template!("outputs/update.md"),
    automation_output: own_template!("outputs/automation.md"),
});

impl Prompts {
    /// The prompts, each from `lookup`'s text for its path under the prompts' root,
    /// or the text Linewright ships where it has none, and the paths whose
    /// text does not parse, with why, each of which takes the shipped text.
    pub fn read<'a>(
        lookup: &mut dyn FnMut(&'static str) -> Option<&'a str>,
    ) -> (Prompts, Vec<(&'static str, anyhow::Error)>) {
        let mut reader = Reader {
            lookup,
            unusable: Vec::new(),
            layout: Vec::new(),
        };
        let prompts = Prompts::build(&mut reader);
        (prompts, reader.unusable)
    }

    /// The path under the prompts' root of every prompt a layer may override,
    /// with the runs it is sent to: the table that says which runs a file holds
    /// (docs/specs/repository.md, "Linewright's files").
    fn layout() -> &'static [(&'static str, Readers)] {
        static LAYOUT: LazyLock<Vec<(&'static str, Readers)>> = LazyLock::new(|| {
            let mut reader = Reader {
                lookup: &mut |_| None,
                unusable: Vec::new(),
                layout: Vec::new(),
            };
            Prompts::build(&mut reader);
            reader.layout
        });
        &LAYOUT
    }

    fn build(reader: &mut Reader) -> Prompts {
        Prompts {
            triage: template!(reader, "triage.md", TRIAGE),
            design: template!(reader, "design.md", DESIGN),
            implement: template!(reader, "implement.md", IMPLEMENT),
            rework: template!(reader, "rework.md", REWORK),
            fix_ci: template!(reader, "fix-ci.md", FIX_CI),
            update: template!(reader, "update.md", UPDATE),
            automation: template!(reader, "automation.md", AUTOMATIONS),
            result_error: template!(reader, "follow-ups/result-error.md", Readers::Every),
            loose_ends: template!(reader, "follow-ups/loose-ends.md", BRANCH_WORK),
            usage_limit_reset: template!(reader, "follow-ups/usage-limit-reset.md", Readers::Every),
            late_feedback: template!(reader, "follow-ups/late-feedback.md", PULL_WORK),
        }
    }

    /// The path under the prompts' root of every prompt a layer may override.
    pub fn paths() -> Vec<&'static str> {
        Prompts::layout().iter().map(|(path, _)| *path).collect()
    }
}

/// The runs the template at `path` under the prompts' root is sent to, or `None`
/// for a path that is no template's.
pub fn readers(path: &str) -> Option<Readers> {
    Prompts::layout()
        .iter()
        .find_map(|(p, readers)| (*p == path).then_some(*readers))
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
}

impl Readers {
    pub fn read_by(self, run: &RunKind) -> bool {
        match self {
            Readers::Every => true,
            Readers::Only(reads) => reads.iter().any(|reads| match (reads, run) {
                (Reads::Stage(kind), RunKind::Stage(run)) => kind == run,
                (Reads::Automations, RunKind::Automation(_)) => true,
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

/// Reads each template from its lookup, noting the runs it is sent to.
struct Reader<'l, 'a> {
    lookup: &'l mut dyn FnMut(&'static str) -> Option<&'a str>,
    unusable: Vec<(&'static str, anyhow::Error)>,
    layout: Vec<(&'static str, Readers)>,
}

impl Reader<'_, '_> {
    /// The template at `path`, which ships as `shipped`. A follow-up is used
    /// without its trailing newline.
    fn read<V: Values>(
        &mut self,
        path: &'static str,
        readers: Readers,
        shipped: &str,
    ) -> Template<V> {
        self.layout.push((path, readers));
        let parse = |text: &str| {
            let text = if path.contains('/') {
                text.trim_end()
            } else {
                text
            };
            Template::parse(text.to_string())
        };
        if let Some(text) = (self.lookup)(path) {
            match parse(text) {
                Ok(template) => return template,
                Err(error) => self.unusable.push((path, error)),
            }
        }
        parse(shipped).expect("the shipped prompts have their values' placeholders")
    }
}

/// The values every first-message prompt shares, its facts filled from
/// `parts/facts.md`.
fn shared(repo: &Repo) -> Shared {
    Shared {
        facts: OWN_TEMPLATES.facts.fill(&Facts {
            repo: repo.name.to_string(),
            default_branch: repo.default_branch.into(),
            spec_glob: repo.spec_glob.into(),
        }),
    }
}

pub fn triage(
    prompts: &Prompts,
    repo: &Repo,
    issue: &Issue,
    categories: &[String],
    attachments: &Path,
) -> String {
    let mut rendered = String::new();
    write_issue(&mut rendered, "The issue", issue);
    prompts.triage.fill(&Triage {
        base: shared(repo),
        categories: categories
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", "),
        issue: rendered.trim_end().into(),
        output: with_attachments(
            &OWN_TEMPLATES.triage_output.fill(&TriageOutput {
                category_choices: categories
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(" | "),
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

pub fn design_spec_change(
    prompts: &Prompts,
    repo: &Repo,
    pr: &MergedPull,
    spec_diff: &str,
    filed_gaps: &[&Issue],
    attachments: &Path,
) -> String {
    prompts.design.fill(&Design {
        base: shared(repo),
        kind: DesignOf::SpecChange.noun().into(),
        item: OWN_TEMPLATES.spec_change.fill(&SpecChange {
            default_branch: repo.default_branch.into(),
            number: pr.number.to_string(),
            title: pr.title.clone(),
            author: pr.author.to_string(),
            body: or_none(&pr.body).into(),
            spec_diff: spec_diff.trim_end().into(),
        }),
        filed_gaps: render_filed_gaps(filed_gaps),
        cutting: OWN_TEMPLATES.cut_spec_change.fill(&NoValues {}),
        output: design_output(DesignOf::SpecChange, attachments),
    })
}

/// The same prompt as a spec change's, with the issue and its comments as
/// what the item asks (docs/specs/design.md, "Designing").
pub fn design_issue(
    prompts: &Prompts,
    repo: &Repo,
    issue: &Issue,
    filed_gaps: &[&Issue],
    attachments: &Path,
) -> String {
    let mut item = String::new();
    write_issue(&mut item, "The issue", issue);
    prompts.design.fill(&Design {
        base: shared(repo),
        kind: DesignOf::Issue.noun().into(),
        item: item.trim_end().into(),
        filed_gaps: render_filed_gaps(filed_gaps),
        cutting: OWN_TEMPLATES.cut_issue.fill(&NoValues {}),
        output: design_output(DesignOf::Issue, attachments),
    })
}

fn design_output(of: DesignOf, attachments: &Path) -> String {
    with_attachments(
        &OWN_TEMPLATES.design_output.fill(&DesignOutput {
            kind: of.noun().into(),
        }),
        attachments,
    )
}

/// A stage's result contract `output`, followed by the attachments
/// contract naming the session's `attachments` directory
/// (docs/specs/runs.md, "Every prompt").
fn with_attachments(output: &str, attachments: &Path) -> String {
    let contract = OWN_TEMPLATES.attachments.fill(&Attachments {
        directory: attachments.display().to_string(),
    });
    format!("{output}\n\n{contract}")
}

pub fn implement(
    prompts: &Prompts,
    repo: &Repo,
    ticket: &Ticket,
    branch: &str,
    start: &BranchStart,
    attachments: &Path,
) -> String {
    let holding = match start {
        BranchStart::New => OWN_TEMPLATES.new_branch.fill(&DefaultBranch {
            default_branch: repo.default_branch.into(),
        }),
        BranchStart::Existing { unfinished } => {
            OWN_TEMPLATES.existing_branch.fill(&ExistingBranch {
                default_branch: repo.default_branch.into(),
                unfinished: unfinished_sentence(
                    &OWN_TEMPLATES.unfinished_or_discard,
                    unfinished.as_ref(),
                ),
            })
        }
    };
    prompts.implement.fill(&Implement {
        base: shared(repo),
        default_branch: repo.default_branch.into(),
        branch: branch.into(),
        holding,
        ticket: render_ticket(ticket),
        output: with_attachments(
            &OWN_TEMPLATES.implement_output.fill(&NoValues {}),
            attachments,
        ),
    })
}

pub fn pull_work(
    prompts: &Prompts,
    repo: &Repo,
    ticket: &Ticket,
    pr: &Pull,
    work: &PullTask,
    unfinished: Option<&UnfinishedCommits>,
    attachments: &Path,
) -> String {
    let pull_request = |conflict: String, output: &Template<NoValues>| PullRequest {
        base: shared(repo),
        branch: pr.head_ref.clone(),
        pr: pr.number.to_string(),
        pr_title: pr.title.clone(),
        unfinished: unfinished_sentence(&OWN_TEMPLATES.unfinished, unfinished),
        conflict,
        description: or_none(&pr.body).into(),
        ticket: render_ticket(ticket),
        output: with_attachments(&output.fill(&NoValues {}), attachments),
    };
    let first_update = |conflict: Option<&Conflict>| {
        conflict
            .map(|c| {
                stopped(
                    &OWN_TEMPLATES.conflict_rebase,
                    &OWN_TEMPLATES.conflict_merge,
                    repo,
                    c,
                )
            })
            .unwrap_or_default()
    };
    match work {
        PullTask::Rework { feedback, conflict } => {
            let (feedback, conversation) = render_feedback(pr, feedback);
            prompts.rework.fill(&Rework {
                base: pull_request(first_update(*conflict), &OWN_TEMPLATES.rework_output),
                feedback,
                conversation,
            })
        }
        PullTask::FixCi { failures, conflict } => prompts.fix_ci.fill(&FixCi {
            base: pull_request(first_update(*conflict), &OWN_TEMPLATES.fix_ci_output),
            failures: render_failures(failures),
        }),
        PullTask::Update(conflict) => prompts.update.fill(&pull_request(
            stopped(
                &OWN_TEMPLATES.update_rebase,
                &OWN_TEMPLATES.update_merge,
                repo,
                conflict,
            ),
            &OWN_TEMPLATES.update_output,
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

pub fn automation(
    prompts: &Prompts,
    repo: &Repo,
    automation: &Automation,
    attachments: &Path,
) -> String {
    prompts.automation.fill(&AutomationRun {
        base: shared(repo),
        max_issues: automation.max_issues.to_string(),
        name: automation.name.to_string(),
        instructions: or_none(&automation.instructions).into(),
        output: with_attachments(
            &OWN_TEMPLATES.automation_output.fill(&NoValues {}),
            attachments,
        ),
    })
}

pub fn result_error(prompts: &Prompts, error: &str) -> String {
    prompts.result_error.fill(&ResultError {
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
        &OWN_TEMPLATES.fold_unfinished.fill(&Commits {
            commits: unfinished_commits(unfinished),
        }),
    )
}

/// What a `done` answer is sent back with while its commits hold a
/// secret, as `held` says.
pub fn held_secret(prompts: &Prompts, held: &Held) -> String {
    loose_ends(
        prompts,
        &OWN_TEMPLATES.held_secret.fill(&HeldSecret {
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
        &OWN_TEMPLATES.pushed_secret.fill(&PushedSecret {
            secrets: secrets.to_string(),
        }),
    )
}

/// What a result is sent back with while the attachment `file` it refers to
/// holds the secret of `variable`.
pub fn attachment_secret(prompts: &Prompts, variable: &str, file: &Path) -> String {
    loose_ends(
        prompts,
        &OWN_TEMPLATES.attachment_secret.fill(&AttachmentSecret {
            variable: variable.into(),
            file: file.display().to_string(),
        }),
    )
}

/// What a result is sent back with while GitHub refuses the attachment
/// `file` it refers to, for `reason`.
pub fn attachment_refused(prompts: &Prompts, file: &Path, reason: &str) -> String {
    result_error(
        prompts,
        &OWN_TEMPLATES.attachment_refused.fill(&AttachmentRefused {
            file: file.display().to_string(),
            reason: reason.into(),
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
fn render_ticket(ticket: &Ticket) -> String {
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
    if let Some(spec_diff) = &ticket.spec_diff {
        let _ = writeln!(
            task,
            "#### The spec change's spec diff\n\n```diff\n{}\n```\n",
            spec_diff.trim_end()
        );
    }
    task.push_str(&render_filed_gaps(&ticket.filed_gaps));
    task.trim_end().to_string()
}

/// The spec gaps already filed for an item, or nothing when there are none.
fn render_filed_gaps(filed_gaps: &[&Issue]) -> String {
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
    OWN_TEMPLATES.filed_gaps.fill(&FiledGapsPart {
        gaps: gaps.trim_end().into(),
    })
}

fn render_failures(failures: &[CheckFailure]) -> String {
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
                OWN_TEMPLATES.full_log.fill(&FullLog {
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

    #[test]
    fn linewrights_own_templates_have_their_values_placeholders() {
        LazyLock::force(&OWN_TEMPLATES);
    }
}
