//! Capturing a run of a stage as a task (`docs/specs/benchmarks.md`, "Tasks"
//! and "Capturing").

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::benchmarks::{Benchmark, BenchmarkId, Benchmarks, ChosenBenchmark, KeptTask};
use crate::domain::{Number, Timestamp};
use crate::git::{self, CommitId};
use crate::input::Input;
use crate::name::Name;
use crate::reconcile::{Work, WorkKind};
use crate::runs::{RunId, RunOf, Runs};
use crate::store::{Properties, RecordedItem, RecordedProperties, Store, properties};

mod item;

pub use item::{IssueState, Merge, TaskItem, TaskItemKind, TaskPull, TaskPullState};

/// The directory, under the work directory, that holds the tasks.
pub const TASKS: &str = "tasks";

/// What a good result is, in the capturer's words: text holding something
/// other than white space, for criteria that hold only white space are no
/// criteria (`docs/specs/benchmarks.md`, "Tasks").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Criteria(String);

impl Criteria {
    /// `text` as criteria, or none when it holds only white space.
    pub fn parse(text: &str) -> Option<Criteria> {
        (!text.trim().is_empty()).then(|| Criteria(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Criteria {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Text as [`Criteria::parse`] takes it, refusing text that holds only
/// white space with [`CRITERIA_REQUIRED`].
impl<'de> serde::Deserialize<'de> for Criteria {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Criteria, D::Error> {
        let text = String::deserialize(deserializer)?;
        Criteria::parse(&text).ok_or_else(|| serde::de::Error::custom(CRITERIA_REQUIRED))
    }
}

/// Why a capture given blank criteria is refused.
pub const CRITERIA_REQUIRED: &str =
    "the criteria are required: what a good result is, not only white space";

/// A part of a task (`docs/specs/benchmarks.md`, "Tasks").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskPart {
    Stage,
    Item,
    Start,
    Items,
    Repository,
    Properties,
    Criteria,
    Run,
}

impl fmt::Display for TaskPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TaskPart::Stage => "stage",
            TaskPart::Item => "item",
            TaskPart::Start => "start",
            TaskPart::Items => "items",
            TaskPart::Repository => "repository",
            TaskPart::Properties => "properties",
            TaskPart::Criteria => "criteria",
            TaskPart::Run => "run",
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
    Lacks { run: RunId, part: TaskPart },
    #[error("a task named {task} already exists in {benchmark}")]
    Taken { task: Name, benchmark: Name },
    #[error("there is no benchmark {0}")]
    NoSuchBenchmark(BenchmarkId),
    #[error("a benchmark is already named {0}")]
    BenchmarkTaken(Name),
    #[error(
        "the benchmark {} holds tasks of {}, and the run {run} is of {stage}",
        benchmark.name,
        benchmark.stage
    )]
    OtherStage {
        run: RunId,
        stage: TaskStage,
        benchmark: Benchmark,
    },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// How a capture that did not happen is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// There is no such run, or no such benchmark.
    NoSuch,
    /// The run cannot be captured, or not under the name asked.
    Refused,
    /// The capture failed.
    Failed,
}

impl CaptureError {
    pub fn refusal(&self) -> Refusal {
        match self {
            CaptureError::NoSuchRun(_) | CaptureError::NoSuchBenchmark(_) => Refusal::NoSuch,
            CaptureError::NotCapturable(_)
            | CaptureError::NoInput(_)
            | CaptureError::Lacks { .. }
            | CaptureError::Taken { .. }
            | CaptureError::BenchmarkTaken(_)
            | CaptureError::OtherStage { .. } => Refusal::Refused,
            CaptureError::Failed(_) => Refusal::Failed,
        }
    }

    /// Why the capture did not happen, a failure with all its causes.
    pub fn why(&self) -> String {
        match self {
            CaptureError::Failed(e) => format!("{e:#}"),
            CaptureError::NoSuchRun(_)
            | CaptureError::NoSuchBenchmark(_)
            | CaptureError::NotCapturable(_)
            | CaptureError::NoInput(_)
            | CaptureError::Lacks { .. }
            | CaptureError::Taken { .. }
            | CaptureError::BenchmarkTaken(_)
            | CaptureError::OtherStage { .. } => self.to_string(),
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

/// What `task.json` holds.
#[derive(Serialize)]
struct TaskFile<'a> {
    stage: WorkKind,
    item: Number,
    start: Timestamp,
    run: &'a RunId,
    api_version: &'a str,
}

/// The stage and item of a run of `of`, when it can be captured: a triage,
/// design or implement run.
pub fn capturable_stage(of: &RunOf) -> Option<(TaskStage, Number)> {
    match of {
        RunOf::Work { item, stage } => TaskStage::try_from(*stage).ok().map(|stage| (stage, *item)),
        RunOf::Automation { .. } => None,
    }
}

/// A run that can be captured: a triage, design or implement run that
/// recorded its input, with every part of it.
#[derive(Debug)]
pub struct CapturableRun {
    run: RunId,
    stage: TaskStage,
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

    pub fn stage(&self) -> TaskStage {
        self.stage
    }
}

/// `run`, of those `runs` keeps, if it can be captured, or why not.
pub async fn capturable_run(runs: &Runs, run: &RunId) -> Result<CapturableRun, CaptureError> {
    if runs.try_read_run(run).await?.is_none() {
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
        .read_input(run)
        .await?
        .ok_or_else(|| CaptureError::NoInput(run.clone()))?;
    let properties = properties.ok_or_else(|| CaptureError::Lacks {
        run: run.clone(),
        part: TaskPart::Properties,
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

/// The benchmark a capture puts its task into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BenchmarkChoice {
    /// A benchmark there is.
    Existing(BenchmarkId),
    /// A benchmark made, of the run's stage, as the task is captured into it.
    New(Name),
}

/// A run that can be captured, with what it is to be captured as.
#[derive(Debug)]
pub struct Capturable {
    run: CapturableRun,
    name: Name,
    benchmark: ChosenBenchmark,
}

impl Capturable {
    /// The stage of the run, and of the benchmark its task goes into.
    pub fn stage(&self) -> TaskStage {
        self.run.stage
    }
}

/// `run`, of those `runs` keeps, to capture as the task `name` into the
/// benchmark `choice` of `benchmarks`, or why it cannot be: read without the
/// store, so that a capture refused is refused before reaching it.
pub async fn capturable(
    runs: &Runs,
    benchmarks: &Benchmarks,
    run: &RunId,
    name: &Name,
    choice: &BenchmarkChoice,
) -> Result<Capturable, CaptureError> {
    let run = capturable_run(runs, run).await?;
    let benchmark = match choice {
        BenchmarkChoice::Existing(id) => {
            let existing = benchmarks
                .find(*id)
                .await?
                .ok_or(CaptureError::NoSuchBenchmark(*id))?;
            if run.stage != existing.stage {
                return Err(CaptureError::OtherStage {
                    stage: run.stage,
                    run: run.run,
                    benchmark: existing,
                });
            }
            if benchmarks.has_task(existing.id, name).await? {
                return Err(CaptureError::Taken {
                    task: name.clone(),
                    benchmark: existing.name,
                });
            }
            ChosenBenchmark::Existing(existing)
        }
        BenchmarkChoice::New(new) => {
            if benchmarks.named(new).await?.is_some() {
                return Err(CaptureError::BenchmarkTaken(new.clone()));
            }
            ChosenBenchmark::New {
                name: new.clone(),
                stage: run.stage,
            }
        }
    };
    Ok(Capturable {
        run,
        name: name.clone(),
        benchmark,
    })
}

/// Captures `capturable` as its task, with `criteria` as what a good result is,
/// and its commits fetched from `store`, into its benchmark of `benchmarks`,
/// making the benchmark when it is a new one, and gives the task kept. All or
/// nothing: the task's directory, its benchmark's record of it and a new
/// benchmark are all written or none is, and of two captures under one name
/// into one benchmark, one is refused as [`CaptureError::Taken`].
pub async fn capture(
    store: &dyn Store,
    capturable: Capturable,
    criteria: &Criteria,
    benchmarks: &Benchmarks,
) -> Result<KeptTask, CaptureError> {
    let Capturable {
        run:
            CapturableRun {
                run,
                stage,
                item,
                input,
            },
        name,
        benchmark,
    } = capturable;
    let tasks = benchmarks.tasks_dir();
    std::fs::create_dir_all(&tasks).context("creating the tasks' directory")?;
    let building = tempfile::Builder::new()
        .prefix(".capturing-")
        .tempdir_in(&tasks)
        .context("creating a directory to capture into")?;
    let write = |file: &str, contents: &[u8]| {
        std::fs::write(building.path().join(file), contents)
            .with_context(|| format!("writing the task's {file}"))
    };
    let task = TaskFile {
        stage: stage.kind(),
        item,
        start: input.start,
        run: &run,
        api_version: &input.api_version,
    };
    write(TASK_FILE, &json(&task)?)?;
    write("items.json", &json(&input.items)?)?;
    write("properties.json", &json(&input.properties)?)?;
    write(CRITERIA_FILE, criteria.as_str().as_bytes())?;
    store
        .fetch_commits(&building.path().join("repository.git"), &input.heads)
        .await?;
    let kept = benchmarks
        .keep_task(&benchmark, &name, &run, building.path())
        .await?;
    let _moved = building.keep();
    Ok(kept)
}

/// The stage of a task: one that can be captured.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
pub enum TaskStage {
    Triage,
    Design,
    Implement,
}

impl TryFrom<WorkKind> for TaskStage {
    type Error = WorkKind;

    /// The stage of a task of `kind`, or `kind` when there is none.
    fn try_from(kind: WorkKind) -> Result<TaskStage, WorkKind> {
        match kind {
            WorkKind::Triage => Ok(TaskStage::Triage),
            WorkKind::Design => Ok(TaskStage::Design),
            WorkKind::Implement => Ok(TaskStage::Implement),
            WorkKind::Rework | WorkKind::FixCi | WorkKind::Update => Err(kind),
        }
    }
}

impl TaskStage {
    /// The work of the stage on `issue`.
    pub fn work(self, issue: Number) -> Work {
        match self {
            TaskStage::Triage => Work::Triage { issue },
            TaskStage::Design => Work::DesignIssue { issue },
            TaskStage::Implement => Work::Implement { issue },
        }
    }

    pub fn kind(self) -> WorkKind {
        match self {
            TaskStage::Triage => WorkKind::Triage,
            TaskStage::Design => WorkKind::Design,
            TaskStage::Implement => WorkKind::Implement,
        }
    }

    pub const ALL: [TaskStage; 3] = [TaskStage::Triage, TaskStage::Design, TaskStage::Implement];

    pub fn name(self) -> &'static str {
        self.kind().name()
    }
}

impl std::str::FromStr for TaskStage {
    type Err = String;

    fn from_str(name: &str) -> Result<TaskStage, String> {
        TaskStage::ALL
            .into_iter()
            .find(|stage| stage.name() == name)
            .ok_or_else(|| {
                let names: Vec<&str> = TaskStage::ALL.map(TaskStage::name).to_vec();
                format!(
                    "{name:?} is not a stage of a benchmark: one of {}",
                    names.join(", ")
                )
            })
    }
}

impl fmt::Display for TaskStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a task cannot be opened: the part it lacks or cannot read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the task's {part}: {why}")]
pub struct TaskRefusal {
    pub part: TaskPart,
    pub why: String,
}

/// A task, opened: every part of it read, so that whatever its content, a
/// store can be seeded from it.
#[derive(Clone, Debug)]
pub struct Task {
    stage: TaskStage,
    item: Number,
    start: Timestamp,
    run: RunId,
    items: Vec<TaskItem>,
    properties: Properties,
    heads: BTreeMap<String, CommitId>,
    repository: PathBuf,
    criteria: Criteria,
}

impl Task {
    /// Opens the task in the directory `dir`, refusing it when it lacks a
    /// part or holds a part that does not read, such as blank criteria, a
    /// merge commit its repository lacks, or an item or default branch it
    /// lacks.
    pub async fn open(dir: &Path) -> Result<Task, TaskRefusal> {
        let dir = &std::path::absolute(dir)
            .map_err(|e| refusal(TaskPart::Stage, format!("{}: {e}", dir.display())))?;
        let file = read_task_file(dir)?;
        let repository = dir.join("repository.git");
        let heads = read_heads(&repository).await?;
        let recorded: RecordedProperties = read_json(dir, "properties.json", TaskPart::Properties)?;
        let properties = properties::read(&recorded)
            .map_err(|e| refusal(TaskPart::Properties, e.to_string()))?;
        let default_branch = &properties.settings.default_branch;
        if !heads.contains_key(default_branch) {
            return Err(refusal(
                TaskPart::Repository,
                format!("it lacks the default branch {default_branch}"),
            ));
        }
        let recorded_items: Vec<RecordedItem> = read_json(dir, "items.json", TaskPart::Items)?;
        let items = read_items(&recorded_items, &repository).await?;
        if !items.iter().any(|item| item.number == file.item) {
            return Err(refusal(
                TaskPart::Items,
                format!("they lack the task's item {}", file.item),
            ));
        }
        let criteria = read_criteria(dir)?;
        Ok(Task {
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

    pub fn stage(&self) -> TaskStage {
        self.stage
    }

    /// The issue the task's run was on.
    pub fn item(&self) -> Number {
        self.item
    }

    /// When Linewright decided to start the task's run.
    pub fn start(&self) -> Timestamp {
        self.start
    }

    /// The run the task was captured from.
    pub fn run(&self) -> &RunId {
        &self.run
    }

    pub fn items(&self) -> &[TaskItem] {
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

    /// What a good result is, in the capturer's words.
    pub fn criteria(&self) -> &Criteria {
        &self.criteria
    }
}

/// The criteria of the task in the directory `dir`, refused when they do
/// not read or hold only white space.
pub fn read_criteria(dir: &Path) -> Result<Criteria, TaskRefusal> {
    let criteria = std::fs::read(dir.join(CRITERIA_FILE))
        .map_err(|e| refusal(TaskPart::Criteria, format!("{CRITERIA_FILE}: {e}")))?;
    let criteria = String::from_utf8(criteria)
        .map_err(|_| refusal(TaskPart::Criteria, format!("{CRITERIA_FILE} is not UTF-8")))?;
    Criteria::parse(&criteria).ok_or_else(|| {
        refusal(
            TaskPart::Criteria,
            format!("{CRITERIA_FILE} holds only white space"),
        )
    })
}

/// Replaces the criteria of the task in the directory `dir` with `criteria`,
/// by a rename, so that a reader sees the old criteria or the new; gives
/// what the file held before, for [`ReplacedCriteria::put_back`].
pub(crate) fn replace_criteria(
    dir: &Path,
    criteria: &Criteria,
) -> anyhow::Result<ReplacedCriteria> {
    let place = dir.join(CRITERIA_FILE);
    let was = match std::fs::read(&place) {
        Ok(was) => Some(was),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => {
            return Err(anyhow::Error::from(e).context(format!("reading {}", place.display())));
        }
    };
    write_criteria(dir, criteria.as_str().as_bytes())?;
    Ok(ReplacedCriteria {
        dir: dir.to_path_buf(),
        was,
    })
}

/// Writes `contents` as the criteria file of the task in the directory
/// `dir`, with the mode of the file it replaces: a new file, written and
/// synced beside the task's directory so that a copy of the directory never
/// holds it, is renamed into place.
fn write_criteria(dir: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let place = dir.join(CRITERIA_FILE);
    let writing = || format!("writing {}", place.display());
    let beside = dir.parent().with_context(writing)?;
    let permissions = match std::fs::metadata(&place) {
        Ok(metadata) => metadata.permissions(),
        Err(_) => std::fs::Permissions::from_mode(0o644),
    };
    let mut file = tempfile::Builder::new()
        .prefix(".criteria-")
        .permissions(permissions)
        .tempfile_in(beside)
        .with_context(writing)?;
    file.write_all(contents).with_context(writing)?;
    file.as_file().sync_all().with_context(writing)?;
    file.persist(&place)
        .map_err(|e| e.error)
        .with_context(writing)?;
    std::fs::File::open(dir)
        .and_then(|dir| dir.sync_all())
        .with_context(writing)?;
    Ok(())
}

/// A task's criteria just replaced, with what its file held before, `None`
/// when it had none.
pub(crate) struct ReplacedCriteria {
    dir: PathBuf,
    was: Option<Vec<u8>>,
}

impl ReplacedCriteria {
    /// Puts back the criteria the task had before.
    pub(crate) fn put_back(self) -> anyhow::Result<()> {
        match self.was {
            Some(was) => write_criteria(&self.dir, &was),
            None => std::fs::remove_file(self.dir.join(CRITERIA_FILE))
                .context("removing the new criteria"),
        }
    }
}

/// What a task's directory gives of the task beside its criteria and its
/// repository: its stage, its item with the item's title as at the start, and
/// its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskFacts {
    pub stage: TaskStage,
    pub item: Number,
    pub title: String,
    /// When Linewright decided to start the task's run.
    pub start: Timestamp,
}

/// The facts of the task in the directory `dir`, refused when a part they
/// come from does not read.
pub fn read_facts(dir: &Path) -> Result<TaskFacts, TaskRefusal> {
    let file = read_task_file(dir)?;
    let items: Vec<RecordedItem> = read_json(dir, "items.json", TaskPart::Items)?;
    let item = items
        .iter()
        .find(|item| item.number == file.item)
        .ok_or_else(|| {
            refusal(
                TaskPart::Items,
                format!("they lack the task's item {}", file.item),
            )
        })?;
    Ok(TaskFacts {
        stage: file.stage,
        item: file.item,
        title: item::title(item).map_err(|why| refusal(TaskPart::Items, why))?,
        start: file.start,
    })
}

/// The file, in a task's directory, of its criteria.
pub(crate) const CRITERIA_FILE: &str = "criteria.md";

/// The file, in a task's directory, of its stage, repository and run.
const TASK_FILE: &str = "task.json";

fn refusal(part: TaskPart, why: String) -> TaskRefusal {
    TaskRefusal { part, why }
}

/// What `task.json` gives.
struct TaskFileRead {
    stage: TaskStage,
    item: Number,
    start: Timestamp,
    run: RunId,
}

fn read_task_file(dir: &Path) -> Result<TaskFileRead, TaskRefusal> {
    let file: serde_json::Map<String, Value> = read_json(dir, TASK_FILE, TaskPart::Stage)?;
    let stage = TaskStage::try_from(task_field::<WorkKind>(&file, TaskPart::Stage, "stage")?)
        .map_err(|kind| {
            refusal(
                TaskPart::Stage,
                format!("a task of {} cannot be run", kind.name()),
            )
        })?;
    Ok(TaskFileRead {
        stage,
        item: task_field(&file, TaskPart::Item, "item")?,
        start: task_field(&file, TaskPart::Start, "start")?,
        run: task_field::<String>(&file, TaskPart::Run, "run")?
            .parse()
            .map_err(|e| refusal(TaskPart::Run, format!("{TASK_FILE}'s run: {e}")))?,
    })
}

/// The field `name` of `task.json`, which holds `part`.
fn task_field<T: DeserializeOwned>(
    file: &serde_json::Map<String, Value>,
    part: TaskPart,
    name: &str,
) -> Result<T, TaskRefusal> {
    let value = file
        .get(name)
        .ok_or_else(|| refusal(part, format!("{TASK_FILE} gives no {name}")))?;
    T::deserialize(value).map_err(|e| refusal(part, format!("{TASK_FILE}'s {name}: {e}")))
}

/// The JSON file `name` in `dir`, which holds `part`.
fn read_json<T: DeserializeOwned>(
    dir: &Path,
    name: &str,
    part: TaskPart,
) -> Result<T, TaskRefusal> {
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
async fn read_heads(repository: &Path) -> Result<BTreeMap<String, CommitId>, TaskRefusal> {
    if !repository.is_dir() {
        return Err(refusal(
            TaskPart::Repository,
            "there is no repository.git".to_string(),
        ));
    }
    let unreadable = |e: anyhow::Error| refusal(TaskPart::Repository, format!("{e:#}"));
    let bare = git_in(repository, &["rev-parse", "--is-bare-repository"])
        .await
        .map_err(unreadable)?;
    if bare.trim() != "true" {
        return Err(refusal(
            TaskPart::Repository,
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
) -> Result<Vec<TaskItem>, TaskRefusal> {
    let unreadable = |why: String| refusal(TaskPart::Items, why);
    let mut merges = BTreeMap::new();
    for item in recorded {
        if let Some(merge_commit) = item::merge_commit(item).map_err(unreadable)? {
            let merge = merge_of(repository, &merge_commit)
                .await
                .map_err(|why| refusal(TaskPart::Repository, why))?;
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
    fn criteria_hold_something_other_than_white_space() {
        for text in ["A good result adds gadgets.\n", "  x  ", "\n\t-\n"] {
            assert_eq!(
                Criteria::parse(text).map(|c| c.as_str().to_string()),
                Some(text.into())
            );
        }
        for text in ["", " ", "\n\t \r\n", "\u{a0}\u{2003}"] {
            assert!(Criteria::parse(text).is_none(), "{text:?}");
        }
    }

    #[test_log::test]
    fn only_a_triage_design_or_implement_run_is_captured() {
        let stage_of = |id: &str| capturable_stage(&id.parse::<RunId>().unwrap().of);

        assert_eq!(stage_of("4/triage-1"), Some((TaskStage::Triage, Number(4))));
        assert_eq!(stage_of("4/design-2"), Some((TaskStage::Design, Number(4))));
        assert_eq!(
            stage_of("4/implement-1"),
            Some((TaskStage::Implement, Number(4)))
        );
        for id in ["9/rework-1", "9/fix-ci-1", "9/update-1", "hourly.md/1"] {
            assert_eq!(stage_of(id), None, "{id}");
        }
    }

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
