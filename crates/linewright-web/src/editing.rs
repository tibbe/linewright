//! Changing a thing in the dialog that "Edit" opens
//! (`docs/specs/ui_patterns.md`, Editing): Edit benchmark, Edit task and Edit
//! configuration (`docs/specs/benchmarks.md`), each the dialog that makes a
//! thing, holding the thing's values as its page's stream last sent them, with
//! "Save" in place of the button that makes it.

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::warn;

use linewright_core::access::{Act, LevelMarker, Permit};
use linewright_core::actions::{
    self, AS_IS_KEEPS_ITS_NAME, Action, BenchmarkRunScreen, another_named, benchmark_taken,
    task_taken,
};
use linewright_core::api::Sources;
use linewright_core::benchmarks::{
    Benchmark, BenchmarkId, BenchmarkRunId, BenchmarkRunRecord, ConfigurationId, CriteriaAnswer,
    EditTaskError, RenameConfigurationError, RenameError, TaskAnswer, TaskChange, TaskId,
};
use linewright_core::name::Name;

use super::buttons::{ScreenButton, Verb};
use super::caller::Asking;
use super::capture::{Task, TaskField, TaskFields};
use super::html::{self, Html};
use super::making::{self, Answer, Field, FieldRefusals, Form, Making, Refusal};
use super::shell::Missing;
use super::write::{Refused, Write};
use super::{benchmark_path, benchmark_run_path, render, task_path};

/// A dialog that "Edit" opens, which its page draws outside every region a
/// stream sends, with the `edit_dialog` macro.
pub(super) struct EditDialog {
    pub(super) id: String,
    /// Where its form posts.
    pub(super) action: String,
    /// What its form holds as the page draws it.
    pub(super) form: Html,
}

impl EditDialog {
    /// The id of the [`EditValues`] its fields start from as it opens.
    pub(super) fn values_id(&self) -> String {
        values_id(&self.id)
    }
}

/// The id of the [`EditValues`] of the dialog `dialog`.
fn values_id(dialog: &str) -> String {
    format!("{dialog}-values")
}

/// The thing's values, by the name of the field that holds each, that a
/// dialog that "Edit" opens starts from as it opens: a hidden element its
/// page draws, which its page's stream keeps as the thing is now.
#[derive(Template)]
#[template(
    source = r#"<div id="{{ id }}" hidden{% for (field, value) in values %} data-{{ field }}="{{ value }}"{% endfor %}></div>"#,
    ext = "html"
)]
struct EditValues {
    id: String,
    values: Vec<(&'static str, String)>,
}

/// The [`EditValues`] of the dialog `dialog`: `values`, by the name of the
/// field that holds each.
pub(super) fn edit_values(dialog: &str, values: Vec<(&'static str, String)>) -> String {
    render(&EditValues {
        id: values_id(dialog),
        values,
    })
}

/// "Edit" among a screen's buttons, which opens the dialog `dialog`, for
/// `action`.
fn edit_button(dialog: &str, action: Action) -> ScreenButton {
    ScreenButton::Act {
        verb: Verb::Edit,
        dialog: dialog.into(),
        action,
    }
}

/// The form a dialog that "Edit" opens sends in `body`, as the dialog keeps
/// it, which is as it starts where it does not read; and the permit to take
/// it as `action` with the form, or else the status and why it is refused:
/// without a permit, such as for a request that did not come from the web
/// UI's own pages, or where it does not read.
pub(super) fn read_sent<L: LevelMarker, T: DeserializeOwned + Clone + Default>(
    action: Action,
    asking: &Asking,
    uri: &Uri,
    body: &[u8],
) -> (T, Permitted<L, T>) {
    const WRITE: Write = Write::Edit;
    let sent = WRITE.read_form::<T>(body);
    let kept = sent.as_ref().ok().cloned().unwrap_or_default();
    let checked = WRITE
        .permit_with(action, asking, uri, sent)
        .map_err(Refused::into_parts);
    (kept, checked)
}

/// The permit to take a change with the form that sends it, or else the
/// status and why it is refused.
pub(super) type Permitted<L, T> = Result<(Permit<L>, T), (StatusCode, String)>;

/// The refusal of a change that failed for `e`.
fn failed<F: Field>(e: &anyhow::Error) -> (StatusCode, Refusal<F>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Refusal::Whole(format!("{e:#}")),
    )
}

/// The refusal of a thing that does not exist, which the dialog shows
/// above its buttons (`docs/specs/ui_patterns.md`, Editing, A thing that
/// is gone).
pub(super) fn gone<F: Field>(missing: &Missing) -> (StatusCode, Refusal<F>) {
    (StatusCode::NOT_FOUND, Refusal::Whole(missing.to_string()))
}

/// The fields of a dialog that changes a name alone, as it sends them.
#[derive(Clone, Default, Deserialize)]
pub(super) struct NameFields {
    #[serde(default)]
    name: String,
}

impl Form for NameFields {
    type Field = NameField;
}

/// The field of a dialog that changes a name alone.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum NameField {
    Name,
}

impl Field for NameField {
    fn required(&self) -> bool {
        match self {
            NameField::Name => true,
        }
    }
}

/// The name `sent`, or else its refusal, of a name that is no name
/// (`docs/specs/benchmarks.md`, Names and IDs).
fn sent_name(sent: &NameFields) -> Result<Name, (StatusCode, Refusal<NameField>)> {
    making::parse_name(&sent.name).map_err(|why| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Refusal::Fields(FieldRefusals::of(NameField::Name, why)),
        )
    })
}

/// What the form of a dialog that changes a name alone holds: the dialog
/// `id`, titled `title`.
#[derive(Template)]
#[template(path = "edit_name.html")]
pub(super) struct EditNameForm {
    id: String,
    title: &'static str,
    making: Making<NameFields>,
}

impl EditNameForm {
    /// The id of its field.
    fn field_id(&self) -> String {
        format!("{}-name", self.id)
    }
}

/// The id of the Edit benchmark dialog.
const EDIT_BENCHMARK: &str = "edit-benchmark";

/// The region of Benchmark's stream that holds the Edit benchmark dialog's
/// [`EditValues`].
pub(super) const EDIT_BENCHMARK_VALUES: &str = "edit-benchmark-values";

/// The form of the Edit benchmark dialog as `making` holds it.
fn benchmark_form(making: Making<NameFields>) -> EditNameForm {
    EditNameForm {
        id: EDIT_BENCHMARK.into(),
        title: "Edit benchmark",
        making,
    }
}

/// The Edit benchmark dialog of `benchmark` (`docs/specs/benchmarks.md`,
/// Edit benchmark), holding its name.
pub(super) fn edit_benchmark(benchmark: &Benchmark) -> EditDialog {
    EditDialog {
        id: EDIT_BENCHMARK.into(),
        action: format!("{}/edit", benchmark_path(benchmark.id)),
        form: html::rendered(&benchmark_form(Making::of(NameFields {
            name: benchmark.name.to_string(),
        }))),
    }
}

/// The values the Edit benchmark dialog of `benchmark` starts from: its
/// name as it is now.
pub(super) fn benchmark_values(benchmark: &Benchmark) -> String {
    render(&EditValues {
        id: values_id(EDIT_BENCHMARK),
        values: vec![("name", benchmark.name.to_string())],
    })
}

/// "Edit" among Benchmark's buttons, which opens [`edit_benchmark`].
pub(super) fn edit_benchmark_button() -> ScreenButton {
    edit_button(EDIT_BENCHMARK, Action::EditBenchmark)
}

/// Renames benchmark `id`, as the path gives it, to the name the form in
/// `body` sends, and answers with the dialog's form: holding the name saved,
/// or as sent with why it was refused (`docs/specs/benchmarks.md`, Edit
/// benchmark, Refusals). A benchmark that does not exist is refused above
/// the buttons, whatever the name. Refused, writing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn save_benchmark(
    State(sources): State<Sources>,
    asking: Asking,
    Path(id): Path<String>,
    uri: Uri,
    body: Bytes,
) -> Answer<EditNameForm> {
    let (fields, sent) = read_sent::<Act, NameFields>(Action::EditBenchmark, &asking, &uri, &body);
    let refuse = |(status, refusal)| Answer::Refused {
        status,
        form: benchmark_form(Making::refused(fields, refusal)),
    };
    let (permit, sent) = match sent {
        Ok(permitted) => permitted,
        Err((status, why)) => return refuse((status, Refusal::Whole(why))),
    };
    let no_such = || gone(&Missing::Benchmark(id.clone()));
    let Ok(benchmark) = id.parse::<BenchmarkId>() else {
        return refuse(no_such());
    };
    let name = match sent_name(&sent) {
        Ok(name) => name,
        Err(refused) => {
            let reading = permit.into();
            return refuse(
                match actions::benchmark_shown(&reading, &sources, benchmark).await {
                    Ok(None) => no_such(),
                    Ok(Some(_)) | Err(_) => refused,
                },
            );
        }
    };
    match actions::edit_benchmark(&permit, &sources, benchmark, &name).await {
        Ok(()) => Answer::Saved {
            form: benchmark_form(Making::of(NameFields {
                name: name.to_string(),
            })),
        },
        Err(RenameError::NoSuchBenchmark(_)) => refuse(no_such()),
        Err(RenameError::Taken(taken)) => refuse((
            StatusCode::UNPROCESSABLE_ENTITY,
            Refusal::Fields(FieldRefusals::of(NameField::Name, benchmark_taken(&taken))),
        )),
        Err(RenameError::Failed(e)) => refuse(failed(&e)),
    }
}

/// The id of the Edit task dialog.
const EDIT_TASK: &str = "edit-task";

/// The region of Task's stream that holds the Edit task dialog's
/// [`EditValues`].
pub(super) const EDIT_TASK_VALUES: &str = "edit-task-values";

/// What the Edit task dialog's form holds.
#[derive(Template)]
#[template(path = "edit_task.html")]
pub(super) struct EditTaskForm {
    making: Making<TaskFields>,
}

/// The Edit task dialog of `task` (`docs/specs/benchmarks.md`, Edit task),
/// holding its name and its criteria, or no criteria where they do not read.
pub(super) fn edit_task(task: &TaskAnswer) -> EditDialog {
    let fields = TaskFields {
        name: task.name.to_string(),
        criteria: match &task.criteria {
            CriteriaAnswer::Criteria(criteria) => criteria.clone(),
            CriteriaAnswer::Unreadable(_) => String::new(),
        },
    };
    EditDialog {
        id: EDIT_TASK.into(),
        action: format!("{}/edit", task_path(task.id)),
        form: html::rendered(&EditTaskForm {
            making: Making::of(fields),
        }),
    }
}

/// The values the Edit task dialog of `task` starts from: its name and its
/// criteria as they are now, but criteria that do not read.
pub(super) fn task_values(task: &TaskAnswer) -> String {
    let mut values = vec![("name", task.name.to_string())];
    if let CriteriaAnswer::Criteria(criteria) = &task.criteria {
        values.push(("criteria", criteria.clone()));
    }
    render(&EditValues {
        id: values_id(EDIT_TASK),
        values,
    })
}

/// "Edit" among Task's buttons, which opens [`edit_task`].
pub(super) fn edit_task_button() -> ScreenButton {
    edit_button(EDIT_TASK, Action::EditTask)
}

/// Changes task `id`, as the path gives it, to the name and criteria the
/// form in `body` sends, and answers with the dialog's form: holding what
/// was saved, or as sent with why it was refused (`docs/specs/benchmarks.md`,
/// Edit task, Refusals). A task that does not exist, such as one another
/// reader removed, is refused above the buttons, whatever the fields.
/// Refused, writing nothing, unless the browser shows the request came from
/// the web UI's own pages.
pub(super) async fn save_task(
    State(sources): State<Sources>,
    asking: Asking,
    Path(id): Path<String>,
    uri: Uri,
    body: Bytes,
) -> Answer<EditTaskForm> {
    let (fields, sent) = read_sent::<Act, TaskFields>(Action::EditTask, &asking, &uri, &body);
    let refuse = |(status, refusal)| Answer::Refused {
        status,
        form: EditTaskForm {
            making: Making::refused(fields, refusal),
        },
    };
    let (permit, sent) = match sent {
        Ok(permitted) => permitted,
        Err((status, why)) => return refuse((status, Refusal::Whole(why))),
    };
    let no_such = || gone(&Missing::Task(id.clone()));
    let Ok(task) = id.parse::<TaskId>() else {
        return refuse(no_such());
    };
    let reading = permit.clone().into();
    let benchmark = match actions::task_shown(&reading, &sources, task).await {
        Ok(Some(shown)) => shown.benchmark.id,
        Ok(None) => return refuse(no_such()),
        Err(e) => {
            warn!(error = format!("{e:#}"), "changing the task {task} failed");
            return refuse(failed(&e));
        }
    };
    let Task { name, criteria } = match sent.checked() {
        Ok(checked) => checked,
        Err(refusals) => {
            return refuse((StatusCode::UNPROCESSABLE_ENTITY, Refusal::Fields(refusals)));
        }
    };
    let saved = TaskFields {
        name: name.to_string(),
        criteria: criteria.as_str().to_string(),
    };
    let change = TaskChange::Both(name, criteria);
    match actions::edit_task(&permit, &sources, benchmark, task, &change).await {
        Ok(()) => Answer::Saved {
            form: EditTaskForm {
                making: Making::of(saved),
            },
        },
        Err(EditTaskError::NoSuchBenchmark(_) | EditTaskError::NoSuchTask { .. }) => {
            refuse(no_such())
        }
        Err(EditTaskError::Taken { task, benchmark }) => refuse((
            StatusCode::UNPROCESSABLE_ENTITY,
            Refusal::Fields(FieldRefusals::of(
                TaskField::Name,
                task_taken(&task, &benchmark),
            )),
        )),
        Err(EditTaskError::Failed(e)) => refuse(failed(&e)),
    }
}

/// The id of the Edit configuration dialog of `configuration`, or `None`
/// for `as-is`, whose name no one changes (`docs/specs/benchmarks.md`,
/// Benchmark runs, The configurations).
pub(super) fn edit_configuration_id(configuration: ConfigurationId) -> Option<String> {
    (configuration != ConfigurationId::AS_IS).then(|| configuration_dialog(configuration))
}

/// The id of the Edit configuration dialog of configuration
/// `configuration`, as a path gives it.
fn configuration_dialog(configuration: impl std::fmt::Display) -> String {
    format!("edit-configuration-{configuration}")
}

/// The region of Benchmark run's stream that holds the [`EditValues`] of
/// each Edit configuration dialog.
pub(super) const EDIT_CONFIGURATIONS_VALUES: &str = "edit-configurations-values";

/// The form of the Edit configuration dialog `id` as `making` holds it.
fn configuration_form(id: String, making: Making<NameFields>) -> EditNameForm {
    EditNameForm {
        id,
        title: "Edit configuration",
        making,
    }
}

/// Each configuration of `benchmark_run` but `as-is`: its ID, the id of
/// its Edit configuration dialog and its name.
fn editable(
    benchmark_run: &BenchmarkRunScreen,
) -> impl Iterator<Item = (ConfigurationId, String, &Name)> {
    benchmark_run
        .configurations
        .iter()
        .filter_map(|configuration| {
            edit_configuration_id(configuration.id)
                .map(|dialog| (configuration.id, dialog, &configuration.name))
        })
}

/// The Edit configuration dialog of each configuration of `benchmark_run`
/// but `as-is` (`docs/specs/benchmarks.md`, Edit configuration), holding its
/// name.
pub(super) fn edit_configurations(benchmark_run: &BenchmarkRunScreen) -> Vec<EditDialog> {
    editable(benchmark_run)
        .map(|(configuration, dialog, name)| EditDialog {
            action: format!(
                "{}/configurations/{configuration}",
                benchmark_run_path(benchmark_run.id)
            ),
            form: html::rendered(&configuration_form(
                dialog.clone(),
                Making::of(NameFields {
                    name: name.to_string(),
                }),
            )),
            id: dialog,
        })
        .collect()
}

/// The values each Edit configuration dialog of `benchmark_run` starts
/// from, the name of its configuration as it is now, in the region
/// [`EDIT_CONFIGURATIONS_VALUES`].
pub(super) fn configuration_values(benchmark_run: &BenchmarkRunScreen) -> String {
    let values: String = editable(benchmark_run)
        .map(|(_, dialog, name)| {
            render(&EditValues {
                id: values_id(&dialog),
                values: vec![("name", name.to_string())],
            })
        })
        .collect();
    format!(r#"<div id="{EDIT_CONFIGURATIONS_VALUES}" hidden>{values}</div>"#)
}

/// Why the configuration `configuration` of the benchmark run `record`,
/// which a path names `id`, cannot be renamed whatever the name: there is no
/// such benchmark run, no such configuration, or it is `as-is`.
fn unrenamable(
    record: Option<&BenchmarkRunRecord>,
    id: &str,
    configuration: ConfigurationId,
) -> Option<(StatusCode, Refusal<NameField>)> {
    match record {
        None => Some(gone(&Missing::BenchmarkRun(id.into()))),
        Some(record) if configuration.place() >= record.configurations.len() => {
            Some(gone(&Missing::Configuration(configuration.to_string())))
        }
        Some(_) if configuration == ConfigurationId::AS_IS => Some((
            StatusCode::UNPROCESSABLE_ENTITY,
            Refusal::Whole(AS_IS_KEEPS_ITS_NAME.into()),
        )),
        Some(_) => None,
    }
}

/// Renames configuration `configuration` of benchmark run `id`, both as the
/// path gives them, to the name the form in `body` sends, while the
/// benchmark run runs or after it ends, and answers with the dialog's form:
/// holding the name saved, or as sent with why it was refused
/// (`docs/specs/benchmarks.md`, Edit configuration, Refusals). `as-is`, and a
/// benchmark run or configuration that does not exist, are refused above
/// the buttons, whatever the name. Refused, writing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn save_configuration(
    State(sources): State<Sources>,
    asking: Asking,
    Path((id, configuration)): Path<(String, String)>,
    uri: Uri,
    body: Bytes,
) -> Answer<EditNameForm> {
    let (fields, sent) =
        read_sent::<Act, NameFields>(Action::EditConfiguration, &asking, &uri, &body);
    let parsed = configuration.parse::<ConfigurationId>();
    let dialog = parsed.as_ref().map_or_else(
        |_| configuration_dialog(&configuration),
        configuration_dialog,
    );
    let form = |making| configuration_form(dialog.clone(), making);
    let refuse = |(status, refusal)| Answer::Refused {
        status,
        form: form(Making::refused(fields, refusal)),
    };
    let (permit, sent) = match sent {
        Ok(permitted) => permitted,
        Err((status, why)) => return refuse((status, Refusal::Whole(why))),
    };
    let Ok(benchmark_run) = id.parse::<BenchmarkRunId>() else {
        return refuse(gone(&Missing::BenchmarkRun(id)));
    };
    let Ok(parsed) = parsed else {
        return refuse(gone(&Missing::Configuration(configuration)));
    };
    let name = match sent_name(&sent) {
        Ok(name) => name,
        Err(refused) => {
            let reading = permit.into();
            return refuse(
                match actions::benchmark_run_record(&reading, &sources, benchmark_run).await {
                    Ok(record) => unrenamable(record.as_ref(), &id, parsed).unwrap_or(refused),
                    Err(_) => refused,
                },
            );
        }
    };
    match actions::edit_configuration(&permit, &sources, benchmark_run, parsed, &name).await {
        Ok(()) => Answer::Saved {
            form: form(Making::of(NameFields {
                name: name.to_string(),
            })),
        },
        Err(RenameConfigurationError::NoSuchBenchmarkRun(_)) => {
            refuse(gone(&Missing::BenchmarkRun(id)))
        }
        Err(RenameConfigurationError::NoSuchConfiguration { .. }) => {
            refuse(gone(&Missing::Configuration(parsed.to_string())))
        }
        Err(RenameConfigurationError::AsIs) => refuse((
            StatusCode::UNPROCESSABLE_ENTITY,
            Refusal::Whole(AS_IS_KEEPS_ITS_NAME.into()),
        )),
        Err(RenameConfigurationError::Taken(taken)) => refuse((
            StatusCode::UNPROCESSABLE_ENTITY,
            Refusal::Fields(FieldRefusals::of(NameField::Name, another_named(&taken))),
        )),
        Err(RenameConfigurationError::Failed(e)) => refuse(failed(&e)),
    }
}
