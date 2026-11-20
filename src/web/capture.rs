//! Capturing a run as a task from its page (`docs/specs/runs.md`, Run,
//! Capturing): the making dialog that "Add task" opens, and the `POST` on the
//! run's path that captures it.

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use serde::Deserialize;
use tracing::{info, warn};

use super::benchmark_page::BenchmarkTab;
use super::html::{self, Html};
use super::making::{Answer, MadeShown, Making, capture_refused, no_such_run};
use super::write::Write;
use super::{benchmark_tab_path, benchmarks, run_path};
use crate::api::{Served, Sources};
use crate::benchmarks::{BenchmarkName, Benchmarks};
use crate::runs::RunId;
use crate::task::{
    self, BenchmarkChoice, CaptureError, Criteria, Refusal, TaskName, TaskStage, capturable_stage,
};

/// The id of the capture's dialog, which "Add task" opens.
pub(super) const DIALOG: &str = "capture";

/// The run page's offer to capture its run as a task.
pub(super) struct Offer {
    run: RunId,
    /// The benchmarks of the run's stage, by name.
    benchmarks: Vec<BenchmarkName>,
}

impl Offer {
    /// The offer for run `id`, if it can be captured. A run whose records
    /// do not read is logged and offers none.
    pub(super) async fn of(sources: &Sources, id: &RunId) -> Option<Offer> {
        let run = match task::capturable_run(&sources.runs, id).await {
            Ok(run) => run,
            Err(e) => {
                if e.refusal() == Refusal::Failed {
                    warn!(run = %id, error = e.why(), "reading whether a run can be captured failed");
                }
                return None;
            }
        };
        let benchmarks = match benchmarks_of(&benchmarks(sources), run.stage()).await {
            Ok(benchmarks) => benchmarks,
            Err(e) => {
                warn!(run = %id, error = format!("{e:#}"), "reading the benchmarks failed");
                return None;
            }
        };
        Some(Offer {
            run: run.id().clone(),
            benchmarks,
        })
    }

    /// The capture's dialog, [`DIALOG`], with its form as it starts. The page
    /// draws it outside every region a stream sends.
    pub(super) fn html(self) -> Html {
        html::rendered(&CaptureDialog {
            form: CaptureForm::blank(&self.run, self.benchmarks),
        })
    }
}

/// The names of the benchmarks of `stage`.
async fn benchmarks_of(
    benchmarks: &Benchmarks,
    stage: TaskStage,
) -> anyhow::Result<Vec<BenchmarkName>> {
    Ok(benchmarks
        .list()
        .await?
        .into_iter()
        .filter(|listed| listed.benchmark.stage == stage)
        .map(|listed| listed.benchmark.name)
        .collect())
}

/// The names of the benchmarks of `run`'s stage, none for a run of no such
/// stage or when they do not read, which is logged.
async fn benchmarks_of_run(sources: &Sources, run: &RunId) -> Vec<BenchmarkName> {
    let Some((stage, _)) = capturable_stage(&run.of) else {
        return vec![];
    };
    benchmarks_of(&benchmarks(sources), stage)
        .await
        .unwrap_or_else(|e| {
            warn!(run = %run, error = format!("{e:#}"), "reading the benchmarks failed");
            vec![]
        })
}

/// The capture's dialog (`docs/specs/runs.md`, Run, Capturing), holding
/// its form.
#[derive(Template)]
#[template(path = "capture_dialog.html")]
struct CaptureDialog {
    form: CaptureForm,
}

impl CaptureDialog {
    fn form(&self) -> Html {
        html::rendered(&self.form)
    }
}

/// The capture's fields, as the form sends them.
#[derive(Default, Deserialize)]
pub(super) struct CaptureFields {
    /// The benchmark chosen, empty for a new one.
    #[serde(default)]
    benchmark: String,
    /// The new benchmark's name.
    #[serde(default, rename = "new-benchmark")]
    new_benchmark: String,
    #[serde(flatten)]
    task: TaskFields,
}

/// What the capture's dialog's form holds: its fields, each under its label,
/// then the refusal, if there is one, then "Cancel" and "Add task".
#[derive(Template)]
#[template(path = "capture_form.html")]
pub(super) struct CaptureForm {
    /// Where it posts: the run's path.
    path: String,
    making: Making<CaptureFields>,
    /// The benchmarks it offers, then a new one.
    benchmarks: Vec<BenchmarkName>,
}

impl CaptureForm {
    /// The form of the capture of `run` as it starts, offering `benchmarks`:
    /// the first of them chosen, or else a new one, and the name suggested
    /// for `run`.
    fn blank(run: &RunId, benchmarks: Vec<BenchmarkName>) -> CaptureForm {
        CaptureForm {
            path: run_path(run),
            making: Making {
                fields: CaptureFields {
                    benchmark: benchmarks
                        .first()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    new_benchmark: String::new(),
                    task: TaskFields {
                        name: TaskName::suggested(run).to_string(),
                        criteria: String::new(),
                    },
                },
                refusal: None,
            },
            benchmarks,
        }
    }

    /// It refused `sent` for `why`, answered with `status`.
    fn refused_as_sent(
        mut self,
        sent: CaptureFields,
        (status, why): (StatusCode, String),
    ) -> Answer<CaptureForm> {
        self.making = Making::refused(sent, why);
        Answer::Refused { status, form: self }
    }
}

/// What a capture's form sends of the task.
#[derive(Default, Deserialize)]
pub(super) struct TaskFields {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) criteria: String,
}

/// Captures run `id` as the task the form in `body` names, with the store
/// `serve` holds, and answers with the capture's form: as it starts once
/// captured, with what it made said under the screen's heading, or as sent
/// with why it was refused. Refused, writing nothing, unless the browser
/// shows the request came from the web UI's own pages.
pub(super) async fn capture(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Answer<CaptureForm> {
    const WRITE: Write = Write::Capture;
    let origin = WRITE.same_origin(&headers, &uri);
    let (sent, refused) = match WRITE.read_form::<CaptureFields>(&body) {
        Ok(sent) => (sent, origin.err()),
        Err(unread) => (
            CaptureFields::default(),
            Some(origin.err().unwrap_or(unread)),
        ),
    };
    let Ok(run) = id.parse::<RunId>() else {
        let form = CaptureForm {
            path: uri.path().to_string(),
            making: Making::blank(),
            benchmarks: vec![],
        };
        return form.refused_as_sent(sent, (StatusCode::NOT_FOUND, no_such_run(&id)));
    };
    let form = CaptureForm::blank(&run, benchmarks_of_run(&sources, &run).await);
    if let Some(refused) = refused {
        return form.refused_as_sent(sent, refused.into_parts());
    }
    match capture_sent(&sources, &run, &sent).await {
        Ok(captured) => Answer::Made {
            shown: MadeShown::Elsewhere(captured.said(&run)),
            blank: CaptureForm::blank(&run, with_made(form.benchmarks, &captured.into)),
        },
        Err(why) => form.refused_as_sent(sent, why),
    }
}

/// `benchmarks`, by name, with the one a capture `into` made among them.
fn with_made(mut benchmarks: Vec<BenchmarkName>, into: &BenchmarkChoice) -> Vec<BenchmarkName> {
    if let BenchmarkChoice::New(made) = into
        && let Err(at) = benchmarks.binary_search(made)
    {
        benchmarks.insert(at, made.clone());
    }
    benchmarks
}

/// Captures `run` into the benchmark `sent` chooses, as the task it names;
/// or else the status and the reason it was refused.
async fn capture_sent(
    sources: &Sources,
    run: &RunId,
    sent: &CaptureFields,
) -> Result<Captured, (StatusCode, String)> {
    let served = polled(sources).map_err(|not| not.refusal())?;
    let choice = match (sent.benchmark.as_str(), sent.new_benchmark.as_str()) {
        ("", "") => return Err(NotCaptured::NewBenchmarkUnnamed.refusal()),
        ("", new) => new
            .parse()
            .map(BenchmarkChoice::New)
            .map_err(|_| NotCaptured::NewBenchmarkMisnamed.refusal())?,
        (named, _) => named
            .parse()
            .map(BenchmarkChoice::Named)
            .map_err(|error| (StatusCode::UNPROCESSABLE_ENTITY, error))?,
    };
    capture_run(sources, &served, run, &choice, &sent.task)
        .await
        .map_err(|not| not.refusal())
}

/// A run captured as a task.
pub(super) struct Captured {
    /// The task's name.
    name: TaskName,
    /// The benchmark it was captured into.
    into: BenchmarkChoice,
}

impl Captured {
    /// What the screen says of `run` captured as it (`docs/specs/runs.md`,
    /// Run, Capturing, What was made), linking to the benchmark's Tasks.
    fn said(&self, run: &RunId) -> Html {
        let new = match self.into {
            BenchmarkChoice::Named(_) => "",
            BenchmarkChoice::New(_) => "new ",
        };
        let benchmark = self.into.benchmark();
        html::built(format!(
            r#"Captured run {} as the task {} in the {new}benchmark <a class="link" href="{}">{}</a>."#,
            html::escape(&run.to_string()),
            html::escape(self.name.as_str()),
            html::escape(&benchmark_tab_path(benchmark, BenchmarkTab::Tasks)),
            html::escape(benchmark.as_str()),
        ))
    }
}

/// Why a capture did not happen.
pub(super) enum NotCaptured {
    /// The task's name is not in the form of a name.
    Name,
    NoCriteria,
    /// `serve` has not polled since it started.
    NotPolled,
    /// A new benchmark was asked for with an empty name.
    NewBenchmarkUnnamed,
    /// A new benchmark's name is not in the form of a name.
    NewBenchmarkMisnamed,
    /// The capture was refused or failed, for `why`, redacted.
    Capture {
        error: CaptureError,
        why: String,
    },
}

impl NotCaptured {
    /// The status the capture that did not happen is answered with, and the
    /// screen's wording of why.
    pub(super) fn refusal(&self) -> (StatusCode, String) {
        (self.status(), capture_refused(self))
    }

    /// The status the capture that did not happen is answered with.
    fn status(&self) -> StatusCode {
        match self {
            NotCaptured::Name
            | NotCaptured::NoCriteria
            | NotCaptured::NewBenchmarkUnnamed
            | NotCaptured::NewBenchmarkMisnamed => StatusCode::UNPROCESSABLE_ENTITY,
            NotCaptured::NotPolled => StatusCode::SERVICE_UNAVAILABLE,
            NotCaptured::Capture { error, .. } => status_of(error),
        }
    }
}

/// What `serve` serves once it has polled, so that a capture can run; or
/// else why it cannot yet.
pub(super) fn polled(sources: &Sources) -> Result<Served, NotCaptured> {
    sources
        .served
        .borrow()
        .clone()
        .ok_or(NotCaptured::NotPolled)
}

/// Captures `run` as the task `task` names into the benchmark `choice`, with
/// the store `served` holds; or else says why it did not.
pub(super) async fn capture_run(
    sources: &Sources,
    served: &Served,
    run: &RunId,
    choice: &BenchmarkChoice,
    task: &TaskFields,
) -> Result<Captured, NotCaptured> {
    let name: TaskName = task.name.parse().map_err(|_| NotCaptured::Name)?;
    let criteria = Criteria::parse(&task.criteria).ok_or(NotCaptured::NoCriteria)?;
    let benchmarks = benchmarks(sources);
    let captured = match task::capturable(&sources.runs, &benchmarks, run, &name, choice).await {
        Ok(capturable) => task::capture(served.store(), capturable, &criteria, &benchmarks).await,
        Err(e) => Err(e),
    };
    match captured {
        Ok(dir) => {
            info!(run = %run, task = %dir.display(), benchmark = %choice.benchmark(), "captured a run as a task");
            Ok(Captured {
                name,
                into: choice.clone(),
            })
        }
        Err(error) => {
            let why = served.redaction().apply(&error.why());
            warn!(run = %run, task = %name, error = why, "a capture failed");
            Err(NotCaptured::Capture { error, why })
        }
    }
}

/// The status a capture that did not happen for `e` is answered with.
fn status_of(e: &CaptureError) -> StatusCode {
    match e.refusal() {
        Refusal::NoSuch => StatusCode::NOT_FOUND,
        Refusal::Refused => StatusCode::UNPROCESSABLE_ENTITY,
        Refusal::Failed => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
