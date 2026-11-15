//! Capturing a run of a stage as a case (`docs/specs/benchmarks.md`, "Cases"
//! and "Capturing").

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::domain::{Number, Timestamp};
use crate::git::{self, CommitId};
use crate::input::Input;
use crate::reconcile::{Work, WorkKind};
use crate::runs::{RunId, RunOf, Runs};
use crate::store::{Properties, RecordedItem, RecordedProperties, Store, properties};

mod item;

pub use item::{CaseItem, CaseItemKind, CasePull, CasePullState, IssueState, Merge};

/// The directory, under the work directory, that holds the cases.
pub const CASES: &str = "cases";

/// A case's name: one or more lowercase letters, digits and hyphens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseName(String);

impl CaseName {
    /// The letters and digits of `run`'s ID joined by hyphens, such as
    /// `43-implement-2` for `43/implement-2`.
    pub fn suggested(run: &RunId) -> CaseName {
        let id = run.to_string().to_ascii_lowercase();
        let words: Vec<&str> = id
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        CaseName(words.join("-"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for CaseName {
    type Err = String;

    fn from_str(name: &str) -> Result<CaseName, String> {
        let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
        if name.is_empty() || !name.chars().all(allowed) {
            return Err(format!(
                "{name:?} is not a case's name: it is lowercase letters, digits and hyphens"
            ));
        }
        Ok(CaseName(name.to_string()))
    }
}

impl fmt::Display for CaseName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A part of a case (`docs/specs/benchmarks.md`, "Cases").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CasePart {
    Stage,
    Item,
    Start,
    Items,
    Repository,
    Properties,
    Criteria,
    Run,
}

impl fmt::Display for CasePart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CasePart::Stage => "stage",
            CasePart::Item => "item",
            CasePart::Start => "start",
            CasePart::Items => "items",
            CasePart::Repository => "repository",
            CasePart::Properties => "properties",
            CasePart::Criteria => "criteria",
            CasePart::Run => "run",
        })
    }
}

/// Why a run was not captured.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("there is no run {0}")]
    NoSuchRun(RunId),
    #[error(
        "the run {0} is {kind}: only a triage, design or implement run can be captured",
        kind = Uncapturable(&.0.of)
    )]
    NotCapturable(RunId),
    #[error("the run {0} recorded no input, so it cannot be captured")]
    NoInput(RunId),
    #[error("the run {run}'s record lacks the {part}, so it cannot be captured")]
    Lacks { run: RunId, part: CasePart },
    #[error("a case is already named {0}")]
    Taken(CaseName),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// How a capture that did not happen is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// There is no such run.
    NoSuchRun,
    /// The run cannot be captured, or not under the name asked.
    Refused,
    /// The capture failed.
    Failed,
}

impl CaptureError {
    pub fn refusal(&self) -> Refusal {
        match self {
            CaptureError::NoSuchRun(_) => Refusal::NoSuchRun,
            CaptureError::NotCapturable(_)
            | CaptureError::NoInput(_)
            | CaptureError::Lacks { .. }
            | CaptureError::Taken(_) => Refusal::Refused,
            CaptureError::Failed(_) => Refusal::Failed,
        }
    }

    /// Why the capture did not happen, a failure with all its causes.
    pub fn why(&self) -> String {
        match self {
            CaptureError::Failed(e) => format!("{e:#}"),
            CaptureError::NoSuchRun(_)
            | CaptureError::NotCapturable(_)
            | CaptureError::NoInput(_)
            | CaptureError::Lacks { .. }
            | CaptureError::Taken(_) => self.to_string(),
        }
    }
}

/// What a run that cannot be captured is of, as in "a rework run".
struct Uncapturable<'a>(&'a RunOf);

impl fmt::Display for Uncapturable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            RunOf::Work { stage, .. } => write!(f, "a {} run", stage.name()),
            RunOf::Automation { .. } => f.write_str("an automation's run"),
        }
    }
}

/// What `case.json` holds.
#[derive(Serialize)]
struct CaseFile<'a> {
    stage: WorkKind,
    item: Number,
    start: Timestamp,
    run: &'a RunId,
    api_version: &'a str,
}

/// The stage and item of a run of `of`, when it can be captured: a triage,
/// design or implement run.
pub fn capturable_stage(of: &RunOf) -> Option<(WorkKind, Number)> {
    match of {
        RunOf::Work { item, stage } => match stage {
            WorkKind::Triage | WorkKind::Design | WorkKind::Implement => Some((*stage, *item)),
            WorkKind::Rework | WorkKind::FixCi | WorkKind::Update => None,
        },
        RunOf::Automation { .. } => None,
    }
}

/// A run that can be captured: a triage, design or implement run that
/// recorded its input, with every part of it.
#[derive(Debug)]
pub struct CapturableRun {
    run: RunId,
    stage: WorkKind,
    item: Number,
    input: WholeInput,
}

/// A run's [`Input`] that lacks no part.
#[derive(Debug)]
struct WholeInput {
    api_version: String,
    start: Timestamp,
    items: Vec<RecordedItem>,
    heads: BTreeMap<String, CommitId>,
    properties: RecordedProperties,
}

impl CapturableRun {
    pub fn id(&self) -> &RunId {
        &self.run
    }
}

/// `run`, of those `runs` keeps, if it can be captured, or why not.
pub fn capturable_run(runs: &Runs, run: &RunId) -> Result<CapturableRun, CaptureError> {
    if runs.try_read_run(run)?.is_none() {
        return Err(CaptureError::NoSuchRun(run.clone()));
    }
    let (stage, item) =
        capturable_stage(&run.of).ok_or_else(|| CaptureError::NotCapturable(run.clone()))?;
    let Input {
        api_version,
        start,
        items,
        heads,
        properties,
    } = runs
        .read_input(run)?
        .ok_or_else(|| CaptureError::NoInput(run.clone()))?;
    let properties = properties.ok_or_else(|| CaptureError::Lacks {
        run: run.clone(),
        part: CasePart::Properties,
    })?;
    Ok(CapturableRun {
        run: run.clone(),
        stage,
        item,
        input: WholeInput {
            api_version,
            start,
            items,
            heads,
            properties,
        },
    })
}

/// A run that can be captured, with what it is to be captured as.
#[derive(Debug)]
pub struct Capturable {
    run: CapturableRun,
    name: CaseName,
    cases: PathBuf,
}

/// `run`, of those `runs` keeps, to capture as the case `name` in `cases`,
/// or why it cannot be: read without the store, so that a capture refused
/// is refused before reaching it.
pub fn capturable(
    runs: &Runs,
    cases: &Path,
    run: &RunId,
    name: &CaseName,
) -> Result<Capturable, CaptureError> {
    let run = capturable_run(runs, run)?;
    if cases.join(name.as_str()).symlink_metadata().is_ok() {
        return Err(CaptureError::Taken(name.clone()));
    }
    Ok(Capturable {
        run,
        name: name.clone(),
        cases: cases.to_path_buf(),
    })
}

/// Captures `capturable` as its case, with `criteria` as what a good result
/// is, and its commits fetched from `store`, and returns the case's
/// directory. All or nothing: the case is whole or absent, and of two
/// captures under one name, one is refused as [`CaptureError::Taken`].
pub async fn capture(
    store: &dyn Store,
    capturable: Capturable,
    criteria: Option<&str>,
) -> Result<PathBuf, CaptureError> {
    let Capturable {
        run:
            CapturableRun {
                run,
                stage,
                item,
                input,
            },
        name,
        cases,
    } = capturable;
    std::fs::create_dir_all(&cases).context("creating the cases' directory")?;
    let building = tempfile::Builder::new()
        .prefix(".capturing-")
        .tempdir_in(&cases)
        .context("creating a directory to capture into")?;
    let write = |file: &str, contents: &[u8]| {
        std::fs::write(building.path().join(file), contents)
            .with_context(|| format!("writing the case's {file}"))
    };
    let case = CaseFile {
        stage,
        item,
        start: input.start,
        run: &run,
        api_version: &input.api_version,
    };
    write("case.json", &json(&case)?)?;
    write("items.json", &json(&input.items)?)?;
    write("properties.json", &json(&input.properties)?)?;
    if let Some(criteria) = criteria {
        write("criteria.md", criteria.as_bytes())?;
    }
    store
        .fetch_commits(&building.path().join("repository.git"), &input.heads)
        .await?;
    let place = cases.join(name.as_str());
    match rustix::fs::renameat_with(
        rustix::fs::CWD,
        building.path(),
        rustix::fs::CWD,
        &place,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {
            let _moved = building.keep();
            Ok(place)
        }
        Err(e) if std::io::Error::from(e).kind() == ErrorKind::AlreadyExists => {
            Err(CaptureError::Taken(name))
        }
        Err(e) => Err(anyhow::Error::from(std::io::Error::from(e))
            .context(format!("moving the case into {}", place.display()))
            .into()),
    }
}

/// The stage of a case: one that can be captured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayStage {
    Triage,
    Design,
    Implement,
}

impl TryFrom<WorkKind> for ReplayStage {
    type Error = WorkKind;

    /// The stage of a case of `kind`, or `kind` when there is none.
    fn try_from(kind: WorkKind) -> Result<ReplayStage, WorkKind> {
        match kind {
            WorkKind::Triage => Ok(ReplayStage::Triage),
            WorkKind::Design => Ok(ReplayStage::Design),
            WorkKind::Implement => Ok(ReplayStage::Implement),
            WorkKind::Rework | WorkKind::FixCi | WorkKind::Update => Err(kind),
        }
    }
}

impl ReplayStage {
    /// The work of the stage on `issue`.
    pub fn work(self, issue: Number) -> Work {
        match self {
            ReplayStage::Triage => Work::Triage { issue },
            ReplayStage::Design => Work::DesignIssue { issue },
            ReplayStage::Implement => Work::Implement { issue },
        }
    }

    pub fn kind(self) -> WorkKind {
        match self {
            ReplayStage::Triage => WorkKind::Triage,
            ReplayStage::Design => WorkKind::Design,
            ReplayStage::Implement => WorkKind::Implement,
        }
    }
}

/// Why a case cannot be opened: the part it lacks or cannot read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the case's {part}: {why}")]
pub struct CaseRefusal {
    pub part: CasePart,
    pub why: String,
}

/// A case, opened: every part of it read, so that whatever its content, a
/// store can be seeded from it.
#[derive(Clone, Debug)]
pub struct Case {
    stage: ReplayStage,
    item: Number,
    start: Timestamp,
    run: RunId,
    items: Vec<CaseItem>,
    properties: Properties,
    heads: BTreeMap<String, CommitId>,
    repository: PathBuf,
    criteria: Option<String>,
}

impl Case {
    /// Opens the case in the directory `dir`, refusing it when it lacks a
    /// part other than its criteria or holds a part that does not read,
    /// such as a merge commit its repository lacks, or an item or default
    /// branch it lacks.
    pub async fn open(dir: &Path) -> Result<Case, CaseRefusal> {
        let dir = &std::path::absolute(dir)
            .map_err(|e| refusal(CasePart::Stage, format!("{}: {e}", dir.display())))?;
        let file = read_case_file(dir)?;
        let repository = dir.join("repository.git");
        let heads = read_heads(&repository).await?;
        let recorded: RecordedProperties = read_json(dir, "properties.json", CasePart::Properties)?;
        let properties = properties::read(&recorded)
            .map_err(|e| refusal(CasePart::Properties, e.to_string()))?;
        let default_branch = &properties.settings.default_branch;
        if !heads.contains_key(default_branch) {
            return Err(refusal(
                CasePart::Repository,
                format!("it lacks the default branch {default_branch}"),
            ));
        }
        let recorded_items: Vec<RecordedItem> = read_json(dir, "items.json", CasePart::Items)?;
        let items = read_items(&recorded_items, &repository).await?;
        if !items.iter().any(|item| item.number == file.item) {
            return Err(refusal(
                CasePart::Items,
                format!("they lack the case's item {}", file.item),
            ));
        }
        let criteria = match std::fs::read(dir.join("criteria.md")) {
            Ok(bytes) => Some(String::from_utf8(bytes).map_err(|_| {
                refusal(CasePart::Criteria, "criteria.md is not UTF-8".to_string())
            })?),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(refusal(CasePart::Criteria, format!("criteria.md: {e}"))),
        };
        Ok(Case {
            stage: file.stage,
            item: file.item,
            start: file.start,
            run: file.run,
            items,
            properties,
            heads,
            repository,
            criteria,
        })
    }

    pub fn stage(&self) -> ReplayStage {
        self.stage
    }

    /// The issue the case's run was on.
    pub fn item(&self) -> Number {
        self.item
    }

    /// When Linewright decided to start the case's run.
    pub fn start(&self) -> Timestamp {
        self.start
    }

    /// The run the case was captured from.
    pub fn run(&self) -> &RunId {
        &self.run
    }

    pub fn items(&self) -> &[CaseItem] {
        &self.items
    }

    pub fn properties(&self) -> &Properties {
        &self.properties
    }

    /// Each branch its repository holds, with its head, the default branch
    /// among them.
    pub fn heads(&self) -> &BTreeMap<String, CommitId> {
        &self.heads
    }

    /// Its bare repository.
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    pub fn criteria(&self) -> Option<&str> {
        self.criteria.as_deref()
    }
}

fn refusal(part: CasePart, why: String) -> CaseRefusal {
    CaseRefusal { part, why }
}

/// What `case.json` gives.
struct CaseFileRead {
    stage: ReplayStage,
    item: Number,
    start: Timestamp,
    run: RunId,
}

fn read_case_file(dir: &Path) -> Result<CaseFileRead, CaseRefusal> {
    let file: serde_json::Map<String, Value> = read_json(dir, "case.json", CasePart::Stage)?;
    let stage = ReplayStage::try_from(case_field::<WorkKind>(&file, CasePart::Stage, "stage")?)
        .map_err(|kind| {
            refusal(
                CasePart::Stage,
                format!("a case of {} cannot be replayed", kind.name()),
            )
        })?;
    Ok(CaseFileRead {
        stage,
        item: case_field(&file, CasePart::Item, "item")?,
        start: case_field(&file, CasePart::Start, "start")?,
        run: case_field::<String>(&file, CasePart::Run, "run")?
            .parse()
            .map_err(|e| refusal(CasePart::Run, format!("case.json's run: {e}")))?,
    })
}

/// The field `name` of `case.json`, which holds `part`.
fn case_field<T: DeserializeOwned>(
    file: &serde_json::Map<String, Value>,
    part: CasePart,
    name: &str,
) -> Result<T, CaseRefusal> {
    let value = file
        .get(name)
        .ok_or_else(|| refusal(part, format!("case.json gives no {name}")))?;
    T::deserialize(value).map_err(|e| refusal(part, format!("case.json's {name}: {e}")))
}

/// The JSON file `name` in `dir`, which holds `part`.
fn read_json<T: DeserializeOwned>(
    dir: &Path,
    name: &str,
    part: CasePart,
) -> Result<T, CaseRefusal> {
    let text = match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(refusal(part, format!("there is no {name}")));
        }
        Err(e) => return Err(refusal(part, format!("{name}: {e}"))),
    };
    serde_json::from_str(&text).map_err(|e| refusal(part, format!("{name}: {e}")))
}

/// Runs git on the bare repository `repository`, and on no repository
/// around it.
async fn git_in(repository: &Path, args: &[&str]) -> anyhow::Result<String> {
    let git_dir = format!("--git-dir={}", repository.display());
    let mut all = vec![git_dir.as_str()];
    all.extend(args);
    git::git(repository, &all).await
}

/// The branches of the bare repository `repository`, with their heads.
async fn read_heads(repository: &Path) -> Result<BTreeMap<String, CommitId>, CaseRefusal> {
    if !repository.is_dir() {
        return Err(refusal(
            CasePart::Repository,
            "there is no repository.git".to_string(),
        ));
    }
    let unreadable = |e: anyhow::Error| refusal(CasePart::Repository, format!("{e:#}"));
    let bare = git_in(repository, &["rev-parse", "--is-bare-repository"])
        .await
        .map_err(unreadable)?;
    if bare.trim() != "true" {
        return Err(refusal(
            CasePart::Repository,
            "repository.git is not a bare repository".to_string(),
        ));
    }
    let refs = git_in(
        repository,
        &[
            "for-each-ref",
            "--format=%(refname:lstrip=2) %(objectname)",
            "refs/heads",
        ],
    )
    .await
    .map_err(unreadable)?;
    Ok(refs
        .lines()
        .filter_map(|line| line.rsplit_once(' '))
        .map(|(branch, head)| (branch.to_string(), CommitId::from(head)))
        .collect())
}

/// `recorded` read, each merge read from `repository`.
async fn read_items(
    recorded: &[RecordedItem],
    repository: &Path,
) -> Result<Vec<CaseItem>, CaseRefusal> {
    let unreadable = |why: String| refusal(CasePart::Items, why);
    let mut merges = BTreeMap::new();
    for item in recorded {
        if let Some(merge_commit) = item::merge_commit(item).map_err(unreadable)? {
            let merge = merge_of(repository, &merge_commit)
                .await
                .map_err(|why| refusal(CasePart::Repository, why))?;
            merges.insert(item.number, merge);
        }
    }
    let held: BTreeSet<Number> = recorded.iter().map(|item| item.number).collect();
    if held.len() != recorded.len() {
        return Err(unreadable("they hold an item twice".to_string()));
    }
    recorded
        .iter()
        .map(|item| item::read(item, &held, &merges).map_err(unreadable))
        .collect()
}

/// What `merge_commit` did to the branch it merged into, read from
/// `repository`, which must hold it on a branch.
async fn merge_of(repository: &Path, merge_commit: &item::MergeCommit) -> Result<Merge, String> {
    let after = &merge_commit.commit;
    let lacks = |e: anyhow::Error| format!("it lacks the merge commit {after}: {e:#}");
    let on_branches = git_in(
        repository,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "--contains",
            after,
            "refs/heads",
        ],
    )
    .await
    .map_err(lacks)?;
    if on_branches.trim().is_empty() {
        return Err(format!("no branch of it holds the merge commit {after}"));
    }
    let count = merge_commit.commits.max(1).saturating_add(1).to_string();
    let log = git_in(
        repository,
        &[
            "log",
            "--first-parent",
            "--format=%H %ct %P",
            "-n",
            &count,
            after,
            "--",
        ],
    )
    .await
    .map_err(lacks)?;
    let before = merged_onto(&log, merge_commit.commits)?;
    let range = format!("{before}..{after}");
    let files = git_in(repository, &["diff", "--name-only", &range, "--"])
        .await
        .map_err(lacks)?;
    Ok(Merge {
        before,
        after: after.clone(),
        commits: merge_commit.commits,
        files: files.lines().map(str::to_string).collect(),
    })
}

/// A commit as `git log --format='%H %ct %P'` gives it.
struct Logged<'a> {
    time: &'a str,
    parents: Vec<&'a str>,
}

/// The commit a pull request of `commits` commits merged onto, from `log`,
/// the first-parent history of its merge commit as `git log
/// --format='%H %ct %P'` gives it, newest first: a merge commit merged onto
/// its first parent; a squash added one commit; and a rebase added the
/// pull request's commits, at most `commits`, which GitHub commits at one
/// time
/// (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github>).
fn merged_onto(log: &str, commits: u64) -> Result<CommitId, String> {
    let logged: Vec<Logged> = log
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let _commit = fields.next();
            Logged {
                time: fields.next().unwrap_or_default(),
                parents: fields.collect(),
            }
        })
        .collect();
    let Some(merge) = logged.first() else {
        return Err("git gave no history of the merge commit".to_string());
    };
    let one_parent = |commit: &Logged| commit.parents.len() == 1;
    let mut added: usize = 1;
    if one_parent(merge) {
        while logged.get(added).is_some_and(|commit| {
            one_parent(commit)
                && commit.time == merge.time
                && u64::try_from(added).is_ok_and(|added| added < commits)
        }) {
            added += 1;
        }
    }
    logged[added - 1]
        .parents
        .first()
        .map(|&parent| CommitId::from(parent))
        .ok_or_else(|| "the merge commit has no parent".to_string())
}

fn json(value: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    let mut json = serde_json::to_vec_pretty(value)?;
    json.push(b'\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn a_name_is_lowercase_letters_digits_and_hyphens() {
        for name in ["gadgets", "43-implement-2", "-", "7"] {
            assert_eq!(
                name.parse::<CaseName>().map(|n| n.to_string()),
                Ok(name.into())
            );
        }
        for name in [
            "",
            "Gadgets",
            "gadgets_1",
            "gadgets 1",
            "../gadgets",
            "gädgets",
        ] {
            assert!(name.parse::<CaseName>().is_err(), "{name:?}");
        }
    }

    #[test_log::test]
    fn only_a_triage_design_or_implement_run_is_captured() {
        let stage_of = |id: &str| capturable_stage(&id.parse::<RunId>().unwrap().of);

        assert_eq!(stage_of("4/triage-1"), Some((WorkKind::Triage, Number(4))));
        assert_eq!(stage_of("4/design-2"), Some((WorkKind::Design, Number(4))));
        assert_eq!(
            stage_of("4/implement-1"),
            Some((WorkKind::Implement, Number(4)))
        );
        for id in ["9/rework-1", "9/fix-ci-1", "9/update-1", "hourly/1"] {
            assert_eq!(stage_of(id), None, "{id}");
        }
    }

    #[test_log::test]
    fn the_suggested_name_is_the_run_ids_letters_and_digits_joined_by_hyphens() {
        let suggested = |id: &str| CaseName::suggested(&id.parse().unwrap()).to_string();

        assert_eq!(suggested("43/implement-2"), "43-implement-2");
        assert_eq!(suggested("812/design-1"), "812-design-1");
    }

    #[track_caller]
    fn onto(log: &str, commits: u64) -> Result<String, String> {
        merged_onto(log, commits).map(|commit| commit.to_string())
    }

    #[test_log::test]
    fn a_merge_commit_merged_onto_its_first_parent() {
        assert_eq!(onto("m 100 a p\na 90 z\n", 2), Ok("a".into()));
    }

    #[test_log::test]
    fn a_squash_of_several_commits_merged_onto_its_parent() {
        assert_eq!(onto("s 100 a\na 90 z\nz 80 y\n", 3), Ok("a".into()));
    }

    #[test_log::test]
    fn a_rebase_merged_onto_the_parent_of_the_commits_committed_with_its_merge() {
        assert_eq!(onto("r2 100 r1\nr1 100 a\na 90 z\n", 2), Ok("a".into()));
        assert_eq!(
            onto("r2 100 r1\nr1 100 a\na 90 z\nz 80 y\n", 3),
            Ok("a".into()),
            "a commit the rebase dropped"
        );
    }

    #[test_log::test]
    fn a_rebase_added_no_more_than_the_pull_requests_commits() {
        assert_eq!(onto("r2 100 r1\nr1 100 a\na 100 z\n", 2), Ok("a".into()));
    }

    #[test_log::test]
    fn a_root_commit_merged_onto_nothing() {
        assert!(onto("r 100\n", 1).is_err());
    }
}
