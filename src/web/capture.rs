//! Capturing a run as a task from its page (`docs/specs/runs.md`, Run,
//! Capturing): the page's form, and the `POST` on the run's path that captures
//! it.

use std::fmt::Write as _;
use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tracing::{info, warn};

use super::html::{self, Html};
use super::write::{Refused, Write, alert};
use super::{benchmarks, run_path};
use crate::api::Sources;
use crate::benchmarks::{BenchmarkName, Benchmarks};
use crate::runs::RunId;
use crate::task::{
    self, BenchmarkChoice, CRITERIA_REQUIRED, CaptureError, Criteria, Refusal, TaskName, TaskStage,
};

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

    /// The dialog `id` of the capture (`docs/specs/web/design_system.md`,
    /// Navigation): a daisyUI `modal` holding the form, its benchmark the
    /// first of the run's stage or else a new one, its name filled in with
    /// the suggested one, then where it says what came of a capture. The
    /// page draws it outside every region a stream sends.
    pub(super) fn html(&self, sources: &Sources, id: &str) -> Html {
        let path = html::escape(&run_path(&self.run));
        let name = html::escape(TaskName::suggested(&self.run).as_str());
        let id = html::escape(id);
        html::built(format!(
            r##"<dialog id="{id}" class="modal" aria-labelledby="{id}-title"><div class="modal-box"><form class="capture" method="post" action="{path}" hx-post="{path}" hx-target="#capture-said" hx-swap="innerHTML"><fieldset class="fieldset gap-2"><legend id="{id}-title" class="fieldset-legend text-base">Add task</legend><label class="select select-sm w-full"><span class="label">benchmark</span>{select}</label><label class="new-benchmark-name input input-sm w-full"><span class="label">new benchmark</span><input name="new-benchmark" pattern="{pattern}" autocomplete="off" placeholder="its name"></label><label class="input input-sm w-full"><span class="label">name</span><input name="name" value="{name}" required pattern="{pattern}" autocomplete="off"></label><textarea class="textarea textarea-sm w-full" name="criteria" rows="4" required placeholder="criteria: what a good result is" aria-label="criteria"></textarea></fieldset><div class="modal-action items-center"><button class="btn btn-sm" type="button" command="close" commandfor="{id}">Cancel</button>{ready}</div></form><div id="capture-said" class="mt-2" role="status"></div></div></dialog>"##,
            select = benchmark_select(&self.benchmarks, None, ""),
            pattern = html::escape(task::NAME_PATTERN),
            ready = Offer::ready_html(sources),
        ))
    }

    /// The region of an offer's form that changes with whether a capture
    /// can run yet, by its id.
    pub(super) fn region(sources: &Sources) -> (&'static str, String) {
        (READY, Offer::ready_html(sources).to_string())
    }

    pub(super) fn ready_html(sources: &Sources) -> Html {
        let (disabled, note) = if polled(sources) {
            ("", String::new())
        } else {
            (
                " disabled",
                format!(r#"<span class="text-muted text-sm">{NOT_POLLED}</span>"#),
            )
        };
        html::built(format!(
            r#"<div id="{READY}" class="flex items-center gap-2"><button class="btn btn-sm btn-primary" type="submit"{disabled}>Add task</button>{note}</div>"#
        ))
    }
}

const READY: &str = "capture-ready";

const NEW_BENCHMARK_UNNAMED: &str = "the new benchmark needs a name";

/// Whether `serve` has polled, so that a capture can run.
pub(super) fn polled(sources: &Sources) -> bool {
    sources.served.borrow().is_some()
}

/// Why a capture cannot run yet.
pub(super) const NOT_POLLED: &str = "the capture can't run before Linewright's first poll";

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

/// The form's choice of a benchmark: each of `benchmarks`, with `selected`
/// chosen, or else the first, then a new one; with `attributes` added.
fn benchmark_select(
    benchmarks: &[BenchmarkName],
    selected: Option<&BenchmarkName>,
    attributes: &str,
) -> String {
    let mut options = String::new();
    for benchmark in benchmarks {
        let chosen = if Some(benchmark) == selected {
            " selected"
        } else {
            ""
        };
        let benchmark = html::escape(benchmark.as_str());
        let _ = write!(
            options,
            r#"<option value="{benchmark}"{chosen}>{benchmark}</option>"#
        );
    }
    format!(
        r#"<select id="{BENCHMARK_SELECT}" name="benchmark"{attributes}>{options}<option class="new-benchmark" value="">a new benchmark</option></select>"#
    )
}

const BENCHMARK_SELECT: &str = "capture-benchmark";

/// What the form sends.
#[derive(Deserialize)]
struct CaptureForm {
    /// Empty for a new benchmark.
    #[serde(default)]
    benchmark: String,
    #[serde(default, rename = "new-benchmark")]
    new_benchmark: String,
    #[serde(flatten)]
    task: TaskFields,
}

/// What a capture's form sends of the task.
#[derive(Deserialize)]
pub(super) struct TaskFields {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) criteria: String,
}

/// Captures run `id` as the task the form in `body` names, with the store
/// `serve` holds, and answers with what the page says of it: where the task
/// was written or why it was not. Refused, writing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn capture(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    Write::Capture.same_origin(&headers, &uri)?;
    let run: RunId = id
        .parse()
        .map_err(|error: String| Write::Capture.refused(StatusCode::NOT_FOUND, &error))?;
    let form: CaptureForm = Write::Capture.read_form(&body)?;
    let choice = match (form.benchmark.as_str(), form.new_benchmark.as_str()) {
        ("", "") => Err(NEW_BENCHMARK_UNNAMED.to_string()),
        ("", new) => new.parse().map(BenchmarkChoice::New),
        (named, _) => named.parse().map(BenchmarkChoice::Named),
    }
    .map_err(|error| Write::Capture.refused(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    let captured = capture_run(&sources, &run, &choice, &form.task)
        .await
        .map_err(NotCaptured::refused)?;
    let mut answer = captured.said(&run, &choice).0;
    match benchmarks_of(&benchmarks(&sources), captured.stage).await {
        Ok(names) => {
            answer.push_str(&benchmark_select(
                &names,
                Some(choice.benchmark()),
                r#" hx-swap-oob="true""#,
            ));
        }
        Err(e) => warn!(error = format!("{e:#}"), "reading the benchmarks failed"),
    }
    Ok((StatusCode::OK, axum::response::Html(answer)).into_response())
}

/// A run captured as a task.
pub(super) struct Captured {
    /// The task's directory.
    dir: PathBuf,
    /// The run's stage.
    stage: TaskStage,
}

impl Captured {
    /// What the page says of `run` captured as it, into `choice`.
    pub(super) fn said(
        &self,
        run: &RunId,
        choice: &BenchmarkChoice,
    ) -> axum::response::Html<String> {
        let new = match choice {
            BenchmarkChoice::Named(_) => "",
            BenchmarkChoice::New(_) => "new ",
        };
        let said = format!(
            "Captured {} as the task <code>{}</code> in the {new}benchmark {}",
            html::escape(&run.to_string()),
            html::escape(&self.dir.display().to_string()),
            html::escape(choice.benchmark().as_str()),
        );
        alert("alert-success", &said)
    }
}

/// Why a capture did not happen.
pub(super) enum NotCaptured {
    /// The task's name is not in the form of a name, for the reason given.
    Name(String),
    NoCriteria,
    NotPolled,
    /// The capture was refused or failed, for `why`, redacted.
    Capture {
        error: CaptureError,
        why: String,
    },
}

impl NotCaptured {
    /// The status the capture that did not happen is answered with.
    pub(super) fn status(&self) -> StatusCode {
        match self {
            NotCaptured::Name(_) | NotCaptured::NoCriteria => StatusCode::UNPROCESSABLE_ENTITY,
            NotCaptured::NotPolled => StatusCode::SERVICE_UNAVAILABLE,
            NotCaptured::Capture { error, .. } => status_of(error),
        }
    }

    /// Why, in the capture's own words.
    pub(super) fn why(&self) -> &str {
        match self {
            NotCaptured::Name(why) | NotCaptured::Capture { why, .. } => why,
            NotCaptured::NoCriteria => CRITERIA_REQUIRED,
            NotCaptured::NotPolled => NOT_POLLED,
        }
    }

    /// It in the capture's own words, in an alert.
    fn refused(self) -> Refused {
        Write::Capture.refused(self.status(), self.why())
    }
}

/// Captures `run` as the task `task` names into the benchmark `choice`, with
/// the store `serve` holds; or else says why it did not.
pub(super) async fn capture_run(
    sources: &Sources,
    run: &RunId,
    choice: &BenchmarkChoice,
    task: &TaskFields,
) -> Result<Captured, NotCaptured> {
    let name: TaskName = task.name.parse().map_err(NotCaptured::Name)?;
    let criteria = Criteria::parse(&task.criteria).ok_or(NotCaptured::NoCriteria)?;
    let served = sources
        .served
        .borrow()
        .clone()
        .ok_or(NotCaptured::NotPolled)?;
    let benchmarks = benchmarks(sources);
    let captured = match task::capturable(&sources.runs, &benchmarks, run, &name, choice).await {
        Ok(capturable) => {
            let stage = capturable.stage();
            task::capture(served.store(), capturable, &criteria, &benchmarks)
                .await
                .map(|dir| Captured { dir, stage })
        }
        Err(e) => Err(e),
    };
    match captured {
        Ok(captured) => {
            info!(run = %run, task = %captured.dir.display(), benchmark = %choice.benchmark(), "captured a run as a task");
            Ok(captured)
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
