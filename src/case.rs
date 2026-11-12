//! Capturing a run of a stage as a case (`docs/specs/benchmarks.md`, "Cases"
//! and "Capturing").

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;

use crate::domain::{Number, Timestamp};
use crate::input::Input;
use crate::reconcile::WorkKind;
use crate::redaction::Redaction;
use crate::runs::{RunId, RunOf, Runs};
use crate::store::Store;

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
            CaptureError::NotCapturable(_) | CaptureError::NoInput(_) | CaptureError::Taken(_) => {
                Refusal::Refused
            }
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
/// recorded its input.
#[derive(Debug)]
pub struct CapturableRun {
    run: RunId,
    stage: WorkKind,
    item: Number,
    input: Input,
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
    let input = runs
        .read_input(run)?
        .ok_or_else(|| CaptureError::NoInput(run.clone()))?;
    Ok(CapturableRun {
        run: run.clone(),
        stage,
        item,
        input,
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

/// Captures `capturable` as its case, with `criteria`, redacted by
/// `redaction`, as what a good result is, and its commits fetched from
/// `store`, and returns the case's directory. All or nothing: the case is
/// whole or absent, and of two captures under one name, one is refused as
/// [`CaptureError::Taken`].
pub async fn capture(
    store: &dyn Store,
    capturable: Capturable,
    criteria: Option<&str>,
    redaction: &Redaction,
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
    if let Some(criteria) = criteria {
        write("criteria.md", redaction.apply(criteria).as_bytes())?;
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
}
