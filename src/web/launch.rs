//! The New benchmark run dialog (`docs/specs/benchmarks.md`, New benchmark
//! run): its form, which starts each file of the brief as the text a run of the
//! stage uses as the dialog opens, and the `POST` that launches what it sends:
//! a file whose text is not changed is not part of the configuration.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use tracing::{info, warn};

use super::benchmark_page::benchmark_for_making;
use super::benchmark_runs;
use super::html::{self, Html};
use super::making::{
    Answer, Field, FieldRefusals, FieldShown, Form, MadeShown, Making, NOT_A_NAME, REQUIRED,
    Refusal,
};
use super::write::Write;
use crate::api::Sources;
use crate::benchmarks::{BenchmarkName, ConfigurationName, LaunchRefusal};
use crate::config;
use crate::repo_files::FileError;
use crate::trial::Configuration;
use crate::work_settings::Effort;

/// The text of each file of the brief that a configuration may give, by its
/// path under `briefs/`, as a run of the stage would use it now: the
/// repository's layer as `serve` last read it, over the operator's and
/// Linewright's own. None before `serve`'s first poll.
fn starting_texts(sources: &Sources) -> BTreeMap<String, String> {
    sources
        .served
        .borrow()
        .as_ref()
        .map(|served| served.briefs().texts())
        .unwrap_or_default()
}

/// How many trials `configurations` configurations, `as-is` among them,
/// `tasks` tasks and `repetitions` repetitions make, in words.
fn trial_count(configurations: usize, tasks: usize, repetitions: u32) -> String {
    let repetitions = usize::try_from(repetitions).unwrap_or(usize::MAX);
    format!(
        "{} × {} × {}: {}",
        counted(configurations, "configuration", "configurations"),
        counted(tasks, "task", "tasks"),
        counted(repetitions, "repetition", "repetitions"),
        counted(
            configurations
                .saturating_mul(tasks)
                .saturating_mul(repetitions),
            "trial",
            "trials"
        ),
    )
}

/// `n` with the noun for one, `one`, or for any other number, `many`.
pub(super) fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The efforts a configuration may set.
fn efforts() -> [&'static str; 5] {
    Effort::ALL.map(Effort::name)
}

/// A configuration but `as-is` as the dialog sends it and shows it.
#[derive(Default)]
pub(super) struct ConfigurationFields {
    /// Its number in its fields' names, `cN.KEY`.
    n: u32,
    name: String,
    model: String,
    effort: String,
    session_timeout: String,
    /// Each file of the brief given, by its path under `briefs/`, with its
    /// text, in the order the reader chose them.
    briefs: Vec<(String, String)>,
    /// The text each of them started with in the dialog, by its path.
    started: BTreeMap<String, String>,
}

/// A field of a configuration but `as-is`, by the key of its name, `cN.KEY`.
#[derive(Clone, Copy)]
enum ConfigurationKey<'a> {
    Name,
    Model,
    Effort,
    SessionTimeout,
    /// The text of the file of the brief at this path under `briefs/`.
    Brief(&'a str),
    /// The text that file started with in the dialog.
    Started(&'a str),
}

impl ConfigurationKey<'_> {
    /// The number `N` and the key of `field`, named `cN.KEY`, if it is a
    /// configuration's.
    fn of(field: &str) -> Option<(u32, ConfigurationKey<'_>)> {
        let (number, key) = field.strip_prefix('c')?.split_once('.')?;
        let parsed: u32 = number.parse().ok()?;
        if parsed.to_string() != number {
            return None;
        }
        let key = match key {
            "name" => ConfigurationKey::Name,
            "model" => ConfigurationKey::Model,
            "effort" => ConfigurationKey::Effort,
            "session-timeout" => ConfigurationKey::SessionTimeout,
            _ => match (key.strip_prefix("briefs/"), key.strip_prefix("started/")) {
                (Some(path), _) => ConfigurationKey::Brief(path),
                (None, Some(path)) => ConfigurationKey::Started(path),
                (None, None) => return None,
            },
        };
        Some((parsed, key))
    }
}

/// What the New benchmark run dialog sends: the repetitions as typed, and each
/// configuration but `as-is`, in the order the dialog shows them.
pub(super) struct LaunchFields {
    repetitions: String,
    configurations: Vec<ConfigurationFields>,
}

impl Default for LaunchFields {
    /// The fields as they start: one repetition, and `as-is` alone.
    fn default() -> LaunchFields {
        LaunchFields {
            repetitions: "1".into(),
            configurations: vec![],
        }
    }
}

impl Form for LaunchFields {
    type Field = LaunchField;
}

/// A field of the New benchmark run dialog.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum LaunchField {
    Repetitions,
    /// The field `field` of the configuration numbered `n` in its fields'
    /// names, `cN.KEY`.
    Configuration {
        n: u32,
        field: ConfigurationField,
    },
}

/// A field of a configuration but `as-is`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ConfigurationField {
    Name,
    Model,
    Effort,
    SessionTimeout,
    /// The box of the file of the brief at this path under `briefs/`.
    Brief(String),
}

impl Field for LaunchField {
    fn required(&self) -> bool {
        match self {
            LaunchField::Repetitions => true,
            LaunchField::Configuration { field, .. } => field.required(),
        }
    }
}

impl Field for ConfigurationField {
    fn required(&self) -> bool {
        match self {
            ConfigurationField::Name => true,
            ConfigurationField::Model
            | ConfigurationField::Effort
            | ConfigurationField::SessionTimeout
            | ConfigurationField::Brief(_) => false,
        }
    }
}

impl LaunchFields {
    /// The fields `sent`, by the names the dialog gives them: `repetitions`,
    /// and `cN.KEY` for a configuration's, ordered by `N`. A field of another
    /// name is left out, and why the form does not read is given with them.
    fn read(sent: Vec<(String, String)>) -> (LaunchFields, Option<String>) {
        let mut repetitions = String::new();
        let mut configurations: BTreeMap<u32, ConfigurationFields> = BTreeMap::new();
        let mut unread = None;
        for (field, value) in sent {
            if field == "repetitions" {
                repetitions = value;
                continue;
            }
            let Some((number, key)) = ConfigurationKey::of(&field) else {
                unread.get_or_insert_with(|| format!("The form has no field \"{field}\"."));
                continue;
            };
            let configuration =
                configurations
                    .entry(number)
                    .or_insert_with(|| ConfigurationFields {
                        n: number,
                        ..ConfigurationFields::default()
                    });
            match key {
                ConfigurationKey::Name => configuration.name = value,
                ConfigurationKey::Model => configuration.model = value,
                ConfigurationKey::Effort => configuration.effort = value,
                ConfigurationKey::SessionTimeout => configuration.session_timeout = value,
                ConfigurationKey::Brief(path) => {
                    let text = value.replace("\r\n", "\n");
                    match configuration
                        .briefs
                        .iter_mut()
                        .find(|(given, _)| *given == path)
                    {
                        Some((_, given)) => *given = text,
                        None => configuration.briefs.push((path.to_string(), text)),
                    }
                }
                ConfigurationKey::Started(path) => {
                    configuration
                        .started
                        .insert(path.to_string(), value.replace("\r\n", "\n"));
                }
            }
        }
        let fields = LaunchFields {
            repetitions,
            configurations: configurations.into_values().collect(),
        };
        (fields, unread)
    }

    /// The repetitions and each configuration but `as-is`, by its name, its
    /// files of the brief given over `starting`, or else every refusal of
    /// the fields (`docs/specs/benchmarks.md`, New benchmark run, Refusals).
    fn check(
        &self,
        starting: &BTreeMap<String, String>,
    ) -> Result<Launch, FieldRefusals<LaunchField>> {
        let mut refused = vec![];
        let names: Vec<Option<ConfigurationName>> = self
            .configurations
            .iter()
            .map(|fields| {
                let typed = fields.name.trim();
                let reason = if typed.is_empty() {
                    REQUIRED
                } else {
                    match typed.parse() {
                        Ok(name) => return Some(name),
                        Err(_) => NOT_A_NAME,
                    }
                };
                refused.push((fields.field(ConfigurationField::Name), reason.to_string()));
                None
            })
            .collect();
        let mut counts = BTreeMap::from([(ConfigurationName::as_is(), 1_usize)]);
        for name in names.iter().flatten() {
            *counts.entry(name.clone()).or_default() += 1;
        }
        let mut configurations = Vec::with_capacity(self.configurations.len());
        for (fields, name) in self.configurations.iter().zip(names) {
            if let Some(name) = &name
                && counts[name] > 1
            {
                refused.push((fields.field(ConfigurationField::Name), another_named(name)));
            }
            match fields.checked(starting) {
                Ok(configuration) => configurations.extend(name.map(|name| (name, configuration))),
                Err(errors) => refused.extend(errors),
            }
        }
        let refusals = FieldRefusals::collect(refused);
        match (self.repetitions(), refusals) {
            (Some(repetitions), None) => Ok((repetitions, configurations)),
            (Some(_), Some(refusals)) => Err(refusals),
            (None, refusals) => {
                let reason = if self.repetitions.trim().is_empty() {
                    REQUIRED
                } else {
                    REPETITIONS
                };
                Err(match refusals {
                    Some(refusals) => refusals.and(LaunchField::Repetitions, reason),
                    None => FieldRefusals::of(LaunchField::Repetitions, reason),
                })
            }
        }
    }

    /// The repetitions typed, while they are a whole number, 1 or more.
    fn repetitions(&self) -> Option<NonZeroU32> {
        let typed = self.repetitions.trim();
        if !typed.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        typed.parse().ok()
    }
}

/// What a launch that passes the form's checks launches: its repetitions, and
/// each configuration but `as-is`, by its name.
type Launch = (NonZeroU32, Vec<(ConfigurationName, Configuration)>);

impl ConfigurationFields {
    /// Its field `field`.
    fn field(&self, field: ConfigurationField) -> LaunchField {
        LaunchField::Configuration { n: self.n, field }
    }

    /// Its settings, each with its key and its field.
    fn settings(&self) -> [(ConfigurationField, &'static str, &str); 3] {
        [
            (ConfigurationField::Model, "model", &self.model),
            (ConfigurationField::Effort, "effort", &self.effort),
            (
                ConfigurationField::SessionTimeout,
                "session-timeout",
                &self.session_timeout,
            ),
        ]
    }

    /// Its configuration, its files of the brief given over `starting`, or
    /// else the refusal of each of its settings and files with an error,
    /// under its field: a setting checked on its own as a `config.toml` of
    /// that one key, and a file of the brief under its box, by its path as
    /// the box's label shows it. An error of its settings together, which
    /// none has on its own, goes under the first it sets.
    fn checked(
        &self,
        starting: &BTreeMap<String, String>,
    ) -> Result<Configuration, Vec<(LaunchField, String)>> {
        let mut refused: Vec<(LaunchField, String)> = self
            .settings()
            .into_iter()
            .filter_map(|(field, key, value)| {
                let value = value.trim();
                if value.is_empty() {
                    return None;
                }
                let table =
                    toml::Table::from_iter([(key.to_string(), toml::Value::String(value.into()))]);
                let text = toml::to_string(&table).expect("a table of strings serializes");
                let error = config::configuration_settings(&text).err()?;
                Some((
                    self.field(field),
                    format!("{SETTINGS} has an error: {error:#}"),
                ))
            })
            .collect();
        let errors = match Configuration::parse(self.files(starting)) {
            Ok(configuration) if refused.is_empty() => return Ok(configuration),
            Ok(_) => vec![],
            Err(errors) => errors,
        };
        let settings_refused = !refused.is_empty();
        for FileError { path, error } in errors {
            match path.strip_prefix(BRIEFS) {
                Some(under) => refused.push((
                    self.field(ConfigurationField::Brief(under.to_string())),
                    format!("{under} has an error: {error}"),
                )),
                None if settings_refused => {}
                None => {
                    let first_set = self
                        .settings()
                        .into_iter()
                        .find(|(_, _, value)| !value.trim().is_empty())
                        .map_or(ConfigurationField::Name, |(field, _, _)| field);
                    refused.push((
                        self.field(first_set),
                        format!("{path} has an error: {error}"),
                    ));
                }
            }
        }
        Err(refused)
    }

    /// The configuration's files, by their paths under CONFIG, as
    /// [`Configuration::parse`] takes them: its settings, if it sets any,
    /// and each file of the brief whose text is not the one it started with
    /// in the dialog, or else `starting`'s.
    fn files(&self, starting: &BTreeMap<String, String>) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut settings = toml::Table::new();
        for (_, key, value) in self.settings() {
            let value = value.trim();
            if !value.is_empty() {
                settings.insert(key.into(), toml::Value::String(value.into()));
            }
        }
        if !settings.is_empty() {
            let text = toml::to_string(&settings).expect("a table of strings serializes");
            files.insert(SETTINGS.to_string(), text.into_bytes());
        }
        for (path, text) in &self.briefs {
            let started = self.started.get(path).or_else(|| starting.get(path));
            if started != Some(text) {
                files.insert(format!("{BRIEFS}{path}"), text.clone().into_bytes());
            }
        }
        files
    }
}

/// What the New benchmark run dialog's form holds.
#[derive(Template)]
#[template(path = "new_benchmark_run.html")]
pub(super) struct NewBenchmarkRunForm {
    /// The benchmark's name, as its path gives it.
    benchmark: String,
    /// How many tasks the benchmark has.
    tasks: usize,
    making: Making<LaunchFields>,
    /// The paths under `briefs/` of the files a configuration may give.
    paths: Vec<String>,
}

impl NewBenchmarkRunForm {
    /// The form as it starts, for benchmark `benchmark` with `tasks` tasks.
    pub(super) fn blank(
        sources: &Sources,
        benchmark: &BenchmarkName,
        tasks: usize,
    ) -> NewBenchmarkRunForm {
        NewBenchmarkRunForm {
            benchmark: benchmark.to_string(),
            tasks,
            making: Making::blank(),
            paths: starting_texts(sources).into_keys().collect(),
        }
    }

    fn title(&self) -> String {
        format!("New benchmark run on {}", self.benchmark)
    }

    fn count(&self) -> String {
        trial_count(
            self.making.fields.configurations.len() + 1,
            self.tasks,
            self.making.fields.repetitions().map_or(1, NonZeroU32::get),
        )
    }

    /// The configuration `fields`.
    fn configuration(&self, fields: &ConfigurationFields) -> Html {
        self.configuration_box(Some(fields.n), fields)
    }

    /// What "Add a configuration" adds, its number `__N__` for the script
    /// to fill in.
    pub(super) fn added(&self) -> Html {
        self.configuration_box(None, &ConfigurationFields::default())
    }

    /// The box of the configuration `fields` numbered `n`, or else
    /// unnumbered.
    fn configuration_box(&self, n: Option<u32>, fields: &ConfigurationFields) -> Html {
        html::rendered(&ConfigurationBox {
            n,
            fields,
            making: &self.making,
            paths: &self.paths,
            efforts: efforts(),
        })
    }

    /// The box of a file of the brief that choosing it opens: its number
    /// `__N__`, its path `__PATH__`, and its text and the text it started
    /// with empty, for the script to fill in.
    pub(super) fn added_file() -> Html {
        html::rendered(&BriefFileBox {
            n: UNNUMBERED,
            path: UNCHOSEN,
            text: "",
            started: Some(""),
            shown: FieldShown::unrefused(&ConfigurationField::Brief(UNCHOSEN.into())),
        })
    }

    /// It refused for `refusal`, answered with `status`.
    fn refused(
        mut self,
        (status, refusal): (StatusCode, Refusal<LaunchField>),
    ) -> Answer<NewBenchmarkRunForm> {
        self.making.refusal = Some(refusal);
        Answer::Refused { status, form: self }
    }
}

/// A configuration but `as-is` in the New benchmark run dialog.
#[derive(Template)]
#[template(path = "new_benchmark_run_configuration.html")]
struct ConfigurationBox<'a> {
    /// Its number in its fields' names, or none while the script has yet to
    /// number it.
    n: Option<u32>,
    fields: &'a ConfigurationFields,
    making: &'a Making<LaunchFields>,
    paths: &'a [String],
    efforts: [&'static str; 5],
}

impl ConfigurationBox<'_> {
    /// Its number as its fields' names give it.
    fn number(&self) -> String {
        self.n.map_or(UNNUMBERED.into(), |n| n.to_string())
    }

    /// The id of its field `key`.
    fn id(&self, key: &str) -> String {
        format!("new-benchmark-run-c{}-{key}", self.number())
    }

    /// How it shows its field `field`.
    fn shown(&self, field: ConfigurationField) -> FieldShown {
        match self.n {
            Some(n) => self.making.field(&LaunchField::Configuration { n, field }),
            None => FieldShown::unrefused(&field),
        }
    }

    /// The box of its file of the brief at `path`, holding `text`.
    fn file(&self, path: &str, text: &str) -> Html {
        html::rendered(&BriefFileBox {
            n: &self.number(),
            path,
            text,
            started: self.fields.started.get(path).map(String::as_str),
            shown: self.shown(ConfigurationField::Brief(path.into())),
        })
    }
}

/// The number of a configuration the script has yet to number.
const UNNUMBERED: &str = "__N__";

/// The path of a file of the brief the script has yet to fill in.
const UNCHOSEN: &str = "__PATH__";

/// The box of a file of the brief that a configuration gives.
#[derive(Template)]
#[template(path = "new_benchmark_run_file.html")]
struct BriefFileBox<'a> {
    /// Its configuration's number in its fields' names.
    n: &'a str,
    /// Its path under `briefs/`.
    path: &'a str,
    text: &'a str,
    /// The text it started with in the dialog, if the dialog sent it.
    started: Option<&'a str>,
    shown: FieldShown,
}

impl BriefFileBox<'_> {
    /// The id of its box.
    fn id(&self) -> String {
        format!("new-benchmark-run-c{}-briefs/{}", self.n, self.path)
    }
}

/// The texts of the files of the brief, by path, as a run of the stage uses
/// them now, as a region of the benchmark page's stream.
#[derive(Template)]
#[template(
    source = r#"<div id="brief-texts" hidden data-texts="{{ json }}"></div>"#,
    ext = "html"
)]
pub(super) struct BriefTexts {
    json: String,
}

impl BriefTexts {
    pub(super) fn of(sources: &Sources) -> BriefTexts {
        BriefTexts {
            json: serde_json::to_string(&starting_texts(sources)).expect("strings serialize"),
        }
    }
}

/// Launches a benchmark run on benchmark `benchmark_named` as the New benchmark
/// run dialog's form in `body` asks, tells `serve` so, and answers with the
/// dialog's form: as it starts once launched, or as sent with why it was
/// refused. Refused, launching nothing, unless the browser shows the request
/// came from the web UI's own pages.
pub(super) async fn launch(
    State(sources): State<Sources>,
    Path(benchmark_named): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Answer<NewBenchmarkRunForm> {
    const WRITE: Write = Write::Launch;
    let sent = WRITE.read_form::<Vec<(String, String)>>(&body);
    let (fields, unread) = match &sent {
        Ok(sent) => LaunchFields::read(sent.clone()),
        Err(_) => (LaunchFields::default(), None),
    };
    let starting = starting_texts(&sources);
    let mut form = NewBenchmarkRunForm {
        benchmark: benchmark_named,
        tasks: 0,
        making: Making::of(fields),
        paths: starting.keys().cloned().collect(),
    };
    let found = benchmark_for_making(&sources, &form.benchmark).await;
    if let Ok(shown) = &found {
        form.tasks = shown.tasks.len();
    }
    if let Err(refused) = WRITE.same_origin(&headers, &uri).and(sent) {
        let (status, why) = refused.into_parts();
        return form.refused((status, Refusal::Whole(why)));
    }
    if let Some(why) = unread {
        return form.refused((StatusCode::BAD_REQUEST, Refusal::Whole(why)));
    }
    let benchmark = match found {
        Ok(shown) => shown.benchmark.name,
        Err((status, why)) => return form.refused((status, Refusal::Whole(why))),
    };
    let (repetitions, configurations) = match form.making.fields.check(&starting) {
        Ok(launch) => launch,
        Err(refusals) => {
            return form.refused((StatusCode::UNPROCESSABLE_ENTITY, Refusal::Fields(refusals)));
        }
    };
    match benchmark_runs(&sources)
        .launch(&benchmark, configurations, repetitions, sources.clock.now())
        .await
    {
        Ok(launched) => {
            sources.benchmark_runs.changed();
            info!(benchmark_run = %launched.id, benchmark = %benchmark, trials = launched.trials, "launched a benchmark run");
            form.making = Making::blank();
            Answer::Made {
                blank: form,
                shown: MadeShown::OnScreen,
            }
        }
        Err(refusal) => {
            if let LaunchRefusal::Failed(e) = &refusal {
                warn!(benchmark = %benchmark, error = format!("{e:#}"), "launching a benchmark run failed");
            }
            let refused = launch_refused(&form.making.fields, &refusal);
            form.refused(refused)
        }
    }
}

/// The refusal of repetitions that are not a whole number, 1 or more.
const REPETITIONS: &str = "Repetitions are a whole number, 1 or more.";

/// The refusal of the name `name` that another configuration has.
fn another_named(name: &ConfigurationName) -> String {
    format!("Another configuration is named {name}.")
}

/// The path under a configuration of its settings.
const SETTINGS: &str = "config.toml";

/// The directory under a configuration of its files of the brief.
const BRIEFS: &str = "briefs/";

/// The status and the refusal of a benchmark run on `fields` that was not
/// launched, or why launching it failed: two configurations of one name
/// under each configuration of that name, and the rest of no field.
fn launch_refused(
    fields: &LaunchFields,
    refusal: &LaunchRefusal,
) -> (StatusCode, Refusal<LaunchField>) {
    let whole = |status, why: String| (status, Refusal::Whole(why));
    match refusal {
        LaunchRefusal::NoSuchBenchmark(_) => whole(StatusCode::NOT_FOUND, refusal.to_string()),
        LaunchRefusal::NoTasks(name) => whole(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("The benchmark {name} has no tasks."),
        ),
        LaunchRefusal::DuplicateConfiguration(name) => {
            let why = another_named(name);
            let named = fields
                .configurations
                .iter()
                .filter(|configuration| configuration.name.trim() == name.as_str())
                .map(|configuration| (configuration.field(ConfigurationField::Name), why.clone()));
            match FieldRefusals::collect(named) {
                Some(refusals) => (StatusCode::UNPROCESSABLE_ENTITY, Refusal::Fields(refusals)),
                None => whole(StatusCode::UNPROCESSABLE_ENTITY, why),
            }
        }
        LaunchRefusal::Failed(e) => whole(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fields of a launch sent as `sent`, by the names the dialog gives
    /// them.
    #[track_caller]
    fn sent(sent: &[(&str, &str)]) -> LaunchFields {
        let (fields, unread) = LaunchFields::read(
            sent.iter()
                .map(|(field, value)| (field.to_string(), value.to_string()))
                .collect(),
        );
        assert_eq!(unread, None);
        fields
    }

    /// The field `field` of the configuration numbered `n`.
    fn configuration_field(n: u32, field: ConfigurationField) -> LaunchField {
        LaunchField::Configuration { n, field }
    }

    /// The refusals of the launch sent as `fields`, over no starting texts.
    #[track_caller]
    fn refused(fields: &[(&str, &str)]) -> FieldRefusals<LaunchField> {
        match sent(fields).check(&BTreeMap::new()) {
            Ok(_) => panic!("{fields:?} is not refused"),
            Err(refusals) => refusals,
        }
    }

    const NAME_FORM: &str = "A name holds only lowercase letters, digits and hyphens, and \
                              starts with a letter or a digit.";

    /// `docs/specs/benchmarks.md`, New benchmark run, Refusals;
    /// `docs/specs/design_system.md`, Making, Required.
    #[test_log::test]
    fn repetitions_that_are_empty_are_required_and_any_other_but_a_whole_number_1_or_more_refused()
    {
        for (typed, reason) in [
            ("", "Required."),
            ("  ", "Required."),
            ("0", "Repetitions are a whole number, 1 or more."),
            ("1.5", "Repetitions are a whole number, 1 or more."),
            ("-2", "Repetitions are a whole number, 1 or more."),
            ("+3", "Repetitions are a whole number, 1 or more."),
            ("many", "Repetitions are a whole number, 1 or more."),
        ] {
            assert_eq!(
                refused(&[("repetitions", typed)]),
                FieldRefusals::of(LaunchField::Repetitions, reason),
                "{typed:?}"
            );
        }
    }

    /// `docs/specs/benchmarks.md`, New benchmark run, Refusals;
    /// `docs/specs/design_system.md`, Making, Required.
    #[test_log::test]
    fn a_name_that_is_empty_is_required_and_one_out_of_form_refused() {
        for (typed, reason) in [
            ("", "Required."),
            (" \t", "Required."),
            ("Short", NAME_FORM),
            ("short one", NAME_FORM),
        ] {
            assert_eq!(
                refused(&[("repetitions", "1"), ("c3.name", typed)]),
                FieldRefusals::of(configuration_field(3, ConfigurationField::Name), reason),
                "{typed:?}"
            );
        }
    }

    /// `docs/specs/benchmarks.md`, New benchmark run, Refusals: a name that
    /// another configuration has, `as-is` among them, under each such name.
    #[test_log::test]
    fn a_name_another_configuration_has_is_refused_under_each_configuration_of_that_name() {
        assert_eq!(
            refused(&[
                ("repetitions", "1"),
                ("c1.name", "short"),
                ("c2.name", "long"),
                ("c4.name", " short "),
                ("c5.name", "as-is"),
            ]),
            FieldRefusals::of(
                configuration_field(1, ConfigurationField::Name),
                "Another configuration is named short."
            )
            .and(
                configuration_field(4, ConfigurationField::Name),
                "Another configuration is named short."
            )
            .and(
                configuration_field(5, ConfigurationField::Name),
                "Another configuration is named as-is."
            )
        );
    }

    /// `docs/specs/benchmarks.md`, New benchmark run, Refusals: each setting
    /// is checked on its own, its error under it.
    #[test_log::test]
    fn a_setting_with_an_error_is_refused_under_it_as_config_toml() {
        let refusals = refused(&[
            ("repetitions", "1"),
            ("c1.name", "short"),
            ("c1.model", "haiku"),
            ("c1.effort", "lots"),
            ("c1.session-timeout", "soon"),
        ]);

        assert_eq!(
            refusals,
            FieldRefusals::of(
                configuration_field(1, ConfigurationField::Effort),
                "config.toml has an error: the setting `effort` is \"lots\": it is `low`, \
                 `medium`, `high`, `xhigh` or `max`"
            )
            .and(
                configuration_field(1, ConfigurationField::SessionTimeout),
                "config.toml has an error: the setting `session-timeout` is \"soon\": expected \
                 number at 0"
            )
        );
    }

    /// `docs/specs/benchmarks.md`, New benchmark run, Refusals: a file of
    /// the brief with an error, named by its path under `briefs/`, as its
    /// box's label shows it.
    #[test_log::test]
    fn a_file_of_the_brief_with_an_error_is_refused_under_its_box() {
        assert_eq!(
            refused(&[
                ("repetitions", "1"),
                ("c2.name", "short"),
                ("c2.briefs/prompts/implement.md", "Implement {{nothing}}.\n"),
            ]),
            FieldRefusals::of(
                configuration_field(2, ConfigurationField::Brief("prompts/implement.md".into())),
                "prompts/implement.md has an error: it has unknown placeholders: `{{nothing}}`"
            )
        );
    }

    /// `docs/specs/design_system.md`, Making, A refusal: each field refused
    /// shows its reason, all at once.
    #[test_log::test]
    fn every_refusal_of_the_form_comes_at_once() {
        let another_twin = "Another configuration is named twin.";
        assert_eq!(
            refused(&[
                ("repetitions", "0"),
                ("c1.name", ""),
                ("c2.name", "Short"),
                ("c3.name", "twin"),
                ("c3.briefs/prompts/implement.md", "Implement {{nothing}}.\n"),
                ("c6.name", "twin"),
                ("c6.effort", "lots"),
            ]),
            FieldRefusals::of(
                LaunchField::Repetitions,
                "Repetitions are a whole number, 1 or more."
            )
            .and(
                configuration_field(1, ConfigurationField::Name),
                "Required."
            )
            .and(configuration_field(2, ConfigurationField::Name), NAME_FORM)
            .and(
                configuration_field(3, ConfigurationField::Name),
                another_twin
            )
            .and(
                configuration_field(3, ConfigurationField::Brief("prompts/implement.md".into())),
                "prompts/implement.md has an error: it has unknown placeholders: `{{nothing}}`"
            )
            .and(
                configuration_field(6, ConfigurationField::Name),
                another_twin
            )
            .and(
                configuration_field(6, ConfigurationField::Effort),
                "config.toml has an error: the setting `effort` is \"lots\": it is `low`, \
                 `medium`, `high`, `xhigh` or `max`"
            )
        );
    }

    /// `docs/specs/benchmarks.md`, New benchmark run, Refusals: the
    /// domain's refusal of two configurations of one name goes under each
    /// configuration of that name, and a benchmark with no tasks above the
    /// buttons.
    #[test_log::test]
    fn the_domains_refusals_go_under_each_configuration_named_or_above_the_buttons() {
        let fields = sent(&[
            ("repetitions", "1"),
            ("c1.name", "short"),
            ("c2.name", "long"),
            ("c3.name", " short"),
        ]);

        let (status, refusal) = launch_refused(
            &fields,
            &LaunchRefusal::DuplicateConfiguration("short".parse().unwrap()),
        );
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let Refusal::Fields(refusals) = refusal else {
            panic!("refused as a whole");
        };
        assert_eq!(
            refusals,
            FieldRefusals::of(
                configuration_field(1, ConfigurationField::Name),
                "Another configuration is named short."
            )
            .and(
                configuration_field(3, ConfigurationField::Name),
                "Another configuration is named short."
            )
        );

        let (no_tasks_status, no_tasks) =
            launch_refused(&fields, &LaunchRefusal::NoTasks("board".parse().unwrap()));
        assert_eq!(no_tasks_status, StatusCode::UNPROCESSABLE_ENTITY);
        let Refusal::Whole(why) = no_tasks else {
            panic!("refused under a field");
        };
        assert_eq!(why, "The benchmark board has no tasks.");
    }

    /// `docs/specs/benchmarks.md`, New benchmark run: a launch that passes
    /// gives its repetitions and each configuration, by its name, with the
    /// files it gives: its settings, and each file of the brief whose text
    /// is not the one it started with.
    #[test_log::test]
    fn a_launch_that_passes_gives_its_repetitions_and_configurations() {
        let starting = BTreeMap::from([("prompts/fix-ci.md".to_string(), "Fix CI.\n".to_string())]);

        let (repetitions, configurations) = sent(&[
            ("repetitions", " 3 "),
            ("c1.name", " short "),
            ("c1.session-timeout", "30m"),
            ("c1.briefs/prompts/fix-ci.md", "Fix CI.\n"),
            ("c2.name", "careful"),
            ("c2.effort", "max"),
        ])
        .check(&starting)
        .unwrap_or_else(|refusals| panic!("{refusals:?}"));

        assert_eq!(repetitions.get(), 3);
        let kept: Vec<(String, Vec<(String, String)>)> = configurations
            .iter()
            .map(|(name, configuration)| {
                let dir = tempfile::tempdir().unwrap();
                configuration.keep(dir.path()).unwrap();
                let mut files: Vec<(String, String)> = std::fs::read_dir(dir.path())
                    .unwrap()
                    .map(|entry| {
                        let path = entry.unwrap().path();
                        (
                            path.file_name().unwrap().to_string_lossy().into_owned(),
                            std::fs::read_to_string(&path).unwrap(),
                        )
                    })
                    .collect();
                files.sort();
                (name.to_string(), files)
            })
            .collect();
        assert_eq!(
            kept,
            [
                (
                    "short".to_string(),
                    vec![(
                        "config.toml".to_string(),
                        "session-timeout = \"30m\"\n".to_string()
                    )]
                ),
                (
                    "careful".to_string(),
                    vec![("config.toml".to_string(), "effort = \"max\"\n".to_string())]
                ),
            ]
        );
    }
}
