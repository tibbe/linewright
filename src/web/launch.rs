//! The New experiment dialog (`docs/specs/benchmarks.md`, New experiment):
//! its form, which starts each file of the brief as the text a run of the
//! stage uses as the dialog opens, and the `POST` that launches what it
//! sends: a file whose text is not changed is not part of the configuration.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use tracing::{info, warn};

use super::experiments;
use super::html::{self, Html};
use super::making::{
    Answer, Making, REPETITIONS, configuration_misnamed, configuration_unnamed,
    configuration_unusable, launch_refused,
};
use super::suite_page::suite_for_making;
use super::write::Write;
use crate::api::Sources;
use crate::benchmarks::{ConfigurationName, LaunchRefusal, SuiteName};
use crate::replay::Configuration;
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

/// How many replays `configurations` configurations, `as-is` among them,
/// `cases` cases and `repetitions` repetitions make, in words.
fn replay_count(configurations: usize, cases: usize, repetitions: u32) -> String {
    let repetitions = usize::try_from(repetitions).unwrap_or(usize::MAX);
    format!(
        "{} × {} × {}: {}",
        counted(configurations, "configuration", "configurations"),
        counted(cases, "case", "cases"),
        counted(repetitions, "repetition", "repetitions"),
        counted(
            configurations
                .saturating_mul(cases)
                .saturating_mul(repetitions),
            "replay",
            "replays"
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

/// What the New experiment dialog sends: the repetitions as typed, and each
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
            let configuration = configurations.entry(number).or_default();
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

    /// Each configuration but `as-is`, its files of the brief given over
    /// `starting`, or else the refusal of the first that cannot be used.
    fn configurations(
        &self,
        starting: &BTreeMap<String, String>,
    ) -> Result<Vec<(ConfigurationName, Configuration)>, String> {
        let mut configurations = Vec::with_capacity(self.configurations.len());
        for (added, fields) in self.configurations.iter().enumerate() {
            let typed = fields.name.trim();
            if typed.is_empty() {
                return Err(configuration_unnamed(added));
            }
            let name: ConfigurationName =
                typed.parse().map_err(|_| configuration_misnamed(typed))?;
            let configuration = Configuration::parse(fields.files(starting))
                .map_err(|errors| configuration_unusable(&name, &errors))?;
            configurations.push((name, configuration));
        }
        Ok(configurations)
    }

    /// The repetitions typed, while they are a whole number, 1 or more.
    fn repetitions(&self) -> Option<NonZeroU32> {
        self.repetitions.trim().parse().ok()
    }
}

impl ConfigurationFields {
    /// The configuration's files, by their paths under CONFIG, as
    /// [`Configuration::parse`] takes them: its settings, if it sets any,
    /// and each file of the brief whose text is not the one it started with
    /// in the dialog, or else `starting`'s.
    fn files(&self, starting: &BTreeMap<String, String>) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut settings = toml::Table::new();
        for (key, value) in [
            ("model", &self.model),
            ("effort", &self.effort),
            ("session-timeout", &self.session_timeout),
        ] {
            let value = value.trim();
            if !value.is_empty() {
                settings.insert(key.into(), toml::Value::String(value.into()));
            }
        }
        if !settings.is_empty() {
            let text = toml::to_string(&settings).expect("a table of strings serializes");
            files.insert("config.toml".to_string(), text.into_bytes());
        }
        for (path, text) in &self.briefs {
            let started = self.started.get(path).or_else(|| starting.get(path));
            if started != Some(text) {
                files.insert(format!("briefs/{path}"), text.clone().into_bytes());
            }
        }
        files
    }
}

/// What the New experiment dialog's form holds.
#[derive(Template)]
#[template(path = "new_experiment.html")]
pub(super) struct NewExperimentForm {
    /// The suite's name, as its path gives it.
    suite: String,
    /// How many cases the suite has.
    cases: usize,
    making: Making<LaunchFields>,
    /// The paths under `briefs/` of the files a configuration may give.
    paths: Vec<String>,
}

impl NewExperimentForm {
    /// The form as it starts, for suite `suite` with `cases` cases.
    pub(super) fn blank(sources: &Sources, suite: &SuiteName, cases: usize) -> NewExperimentForm {
        NewExperimentForm {
            suite: suite.to_string(),
            cases,
            making: Making::blank(),
            paths: starting_texts(sources).into_keys().collect(),
        }
    }

    fn title(&self) -> String {
        format!("New experiment on {}", self.suite)
    }

    fn count(&self) -> String {
        replay_count(
            self.making.fields.configurations.len() + 1,
            self.cases,
            self.making.fields.repetitions().map_or(1, NonZeroU32::get),
        )
    }

    /// The configuration `fields`, numbered `n` for its fields' names.
    fn configuration(&self, n: usize, fields: &ConfigurationFields) -> Html {
        self.configuration_box(n.to_string(), fields)
    }

    /// What "Add a configuration" adds, its number `__N__` for the script
    /// to fill in.
    pub(super) fn added(&self) -> Html {
        self.configuration_box(UNNUMBERED.into(), &ConfigurationFields::default())
    }

    fn configuration_box(&self, n: String, fields: &ConfigurationFields) -> Html {
        html::rendered(&ConfigurationBox {
            n,
            fields,
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
        })
    }

    /// It refused for `why`, answered with `status`.
    fn refused(mut self, (status, why): (StatusCode, String)) -> Answer<NewExperimentForm> {
        self.making.refusal = Some(why);
        Answer::Refused { status, form: self }
    }
}

/// A configuration but `as-is` in the New experiment dialog.
#[derive(Template)]
#[template(path = "new_experiment_configuration.html")]
struct ConfigurationBox<'a> {
    /// Its number in its fields' names.
    n: String,
    fields: &'a ConfigurationFields,
    paths: &'a [String],
    efforts: [&'static str; 5],
}

impl ConfigurationBox<'_> {
    /// The id of its field `key`.
    fn id(&self, key: &str) -> String {
        format!("new-experiment-c{}-{key}", self.n)
    }

    /// The box of its file of the brief at `path`, holding `text`.
    fn file(&self, path: &str, text: &str) -> Html {
        html::rendered(&BriefFileBox {
            n: &self.n,
            path,
            text,
            started: self.fields.started.get(path).map(String::as_str),
        })
    }
}

/// The number of a configuration the script has yet to number.
const UNNUMBERED: &str = "__N__";

/// The path of a file of the brief the script has yet to fill in.
const UNCHOSEN: &str = "__PATH__";

/// The box of a file of the brief that a configuration gives.
#[derive(Template)]
#[template(path = "new_experiment_file.html")]
struct BriefFileBox<'a> {
    /// Its configuration's number in its fields' names.
    n: &'a str,
    /// Its path under `briefs/`.
    path: &'a str,
    text: &'a str,
    /// The text it started with in the dialog, if the dialog sent it.
    started: Option<&'a str>,
}

/// The texts of the files of the brief, by path, as a run of the stage uses
/// them now, as a region of the suite page's stream.
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

/// Launches an experiment on suite `suite_named` as the New experiment dialog's
/// form in `body` asks, tells `serve` so, and answers with the dialog's form:
/// as it starts once launched, or as sent with why it was refused. Refused,
/// launching nothing, unless the browser shows the request came from the web
/// UI's own pages.
pub(super) async fn launch(
    State(sources): State<Sources>,
    Path(suite_named): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Answer<NewExperimentForm> {
    const WRITE: Write = Write::Launch;
    let sent = WRITE.read_form::<Vec<(String, String)>>(&body);
    let (fields, unread) = match &sent {
        Ok(sent) => LaunchFields::read(sent.clone()),
        Err(_) => (LaunchFields::default(), None),
    };
    let starting = starting_texts(&sources);
    let mut form = NewExperimentForm {
        suite: suite_named,
        cases: 0,
        making: Making {
            fields,
            refusal: None,
        },
        paths: starting.keys().cloned().collect(),
    };
    let found = suite_for_making(&sources, &form.suite).await;
    if let Ok(shown) = &found {
        form.cases = shown.cases.len();
    }
    if let Err(refused) = WRITE.same_origin(&headers, &uri).and(sent) {
        return form.refused(refused.into_parts());
    }
    if let Some(why) = unread {
        return form.refused((StatusCode::BAD_REQUEST, why));
    }
    let suite = match found {
        Ok(shown) => shown.suite.name,
        Err(refused) => return form.refused(refused),
    };
    let unprocessable = |why: String| (StatusCode::UNPROCESSABLE_ENTITY, why);
    let Some(repetitions) = form.making.fields.repetitions() else {
        return form.refused(unprocessable(REPETITIONS.into()));
    };
    let configurations = match form.making.fields.configurations(&starting) {
        Ok(configurations) => configurations,
        Err(why) => return form.refused(unprocessable(why)),
    };
    match experiments(&sources)
        .launch(&suite, configurations, repetitions, sources.clock.now())
        .await
    {
        Ok(launched) => {
            sources.experiments.changed();
            info!(experiment = %launched.id, suite = %suite, replays = launched.replays, "launched an experiment");
            form.making = Making::blank();
            Answer::Made { blank: form }
        }
        Err(refusal) => {
            if let LaunchRefusal::Failed(e) = &refusal {
                warn!(suite = %suite, error = format!("{e:#}"), "launching an experiment failed");
            }
            form.refused(launch_refused(&refusal))
        }
    }
}
