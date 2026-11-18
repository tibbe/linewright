//! Launching an experiment from the suite page (`docs/specs/design_system.md`,
//! The suite page, Launching): the form, which starts each file of the
//! brief as the text a run of the stage would use as the form opens, and the
//! `POST` that launches what it sends: a file whose text is not changed is
//! not part of the configuration.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tracing::{info, warn};

use super::html::{self, Html};
use super::write::{Refused, Write, alert};
use super::{experiment_path, experiments};
use crate::api::Sources;
use crate::benchmarks::{ConfigurationName, LaunchRefusal, SuiteName};
use crate::replay::Configuration;
use crate::work_settings::Effort;

/// The text of each file of the brief that a configuration may give, by its
/// path under `briefs/`, as a run of the stage would use it now: the
/// repository's layer as `serve` last read it, over the operator's and
/// Linewright's own. None before `serve`'s first poll.
pub(super) fn starting_texts(sources: &Sources) -> BTreeMap<String, String> {
    sources
        .served
        .borrow()
        .as_ref()
        .map(|served| served.briefs().texts())
        .unwrap_or_default()
}

/// How many replays `cases` cases under `configurations` configurations,
/// `as-is` among them, `repetitions` times each make, in words.
pub(super) fn replay_count(cases: usize, configurations: usize, repetitions: u32) -> String {
    let repetitions = usize::try_from(repetitions).unwrap_or(usize::MAX);
    format!(
        "{}: {}, {}, {}",
        counted(
            cases
                .saturating_mul(configurations)
                .saturating_mul(repetitions),
            "replay",
            "replays"
        ),
        counted(cases, "case", "cases"),
        counted(configurations, "configuration", "configurations"),
        counted(repetitions, "repetition", "repetitions"),
    )
}

/// `n` with the noun for one, `one`, or for any other number, `many`.
pub(super) fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The texts of the files of the brief as the form's script reads them: a
/// JSON object, safe to put in a `<script>` element, as it holds no `<`
/// (<https://html.spec.whatwg.org/multipage/scripting.html#restrictions-for-contents-of-script-elements>).
pub(super) fn texts_json(texts: &BTreeMap<String, String>) -> Html {
    let json = serde_json::to_string(texts).expect("strings serialize");
    html::built(json.replace('<', "\\u003c"))
}

/// The efforts a configuration may set.
pub(super) fn efforts() -> [&'static str; 5] {
    Effort::ALL.map(Effort::name)
}

/// A configuration as the form sends it, by its number in the form.
#[derive(Default)]
struct Fields {
    name: String,
    model: String,
    effort: String,
    session_timeout: String,
    /// Each file of the brief given, by its path under `briefs/`.
    briefs: BTreeMap<String, String>,
    /// The text each of them started with in the form, by its path.
    started: BTreeMap<String, String>,
}

/// What the form sends: the repetitions, and each configuration, in the
/// order the form numbers them.
struct LaunchForm {
    repetitions: String,
    configurations: BTreeMap<u32, Fields>,
}

const REPETITIONS: &str = "the repetitions are a whole number, 1 or more";

impl LaunchForm {
    /// The form of the fields `sent`, or why it does not read.
    fn read(sent: Vec<(String, String)>) -> Result<LaunchForm, String> {
        let mut form = LaunchForm {
            repetitions: String::new(),
            configurations: BTreeMap::new(),
        };
        for (field, value) in sent {
            if field == "repetitions" {
                form.repetitions = value;
                continue;
            }
            let unknown = || format!("the form has no field {field:?}");
            let (number, key) = field
                .strip_prefix('c')
                .and_then(|rest| rest.split_once('.'))
                .ok_or_else(unknown)?;
            let number: u32 = number
                .parse()
                .ok()
                .filter(|parsed: &u32| parsed.to_string() == number)
                .ok_or_else(unknown)?;
            let configuration = form.configurations.entry(number).or_default();
            match key {
                "name" => configuration.name = value,
                "model" => configuration.model = value,
                "effort" => configuration.effort = value,
                "session-timeout" => configuration.session_timeout = value,
                _ => match (key.strip_prefix("briefs/"), key.strip_prefix("started/")) {
                    (Some(path), _) => {
                        configuration
                            .briefs
                            .insert(path.to_string(), value.replace("\r\n", "\n"));
                    }
                    (None, Some(path)) => {
                        configuration
                            .started
                            .insert(path.to_string(), value.replace("\r\n", "\n"));
                    }
                    (None, None) => return Err(unknown()),
                },
            }
        }
        Ok(form)
    }
}

impl Fields {
    /// The configuration's files, by their paths under CONFIG, as
    /// [`Configuration::parse`] takes them: its settings, if it sets any,
    /// and each file of the brief whose text is not the one it started with
    /// in the form, or else `starting`'s.
    fn files(self, starting: &BTreeMap<String, String>) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut settings = toml::Table::new();
        for (key, value) in [
            ("model", self.model),
            ("effort", self.effort),
            ("session-timeout", self.session_timeout),
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
        for (path, text) in self.briefs {
            let started = self.started.get(&path).or_else(|| starting.get(&path));
            if started != Some(&text) {
                files.insert(format!("briefs/{path}"), text.into_bytes());
            }
        }
        files
    }
}

/// Launches an experiment on suite `name` as the form in `body` asks, tells
/// `serve` so, and answers with what the page says of it. Refused,
/// launching nothing, unless the browser shows the request came from the
/// web UI's own pages.
pub(super) async fn launch(
    State(sources): State<Sources>,
    Path(suite): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::Launch;
    WRITE.same_origin(&headers, &uri)?;
    let unprocessable = |why: &str| WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, why);
    let suite: SuiteName = suite
        .parse()
        .map_err(|_| WRITE.refused(StatusCode::NOT_FOUND, &format!("there is no suite {suite}")))?;
    let sent: Vec<(String, String)> = WRITE.read_form(&body)?;
    let form =
        LaunchForm::read(sent).map_err(|why| WRITE.refused(StatusCode::BAD_REQUEST, &why))?;
    let repetitions: NonZeroU32 = form
        .repetitions
        .trim()
        .parse()
        .map_err(|_| unprocessable(REPETITIONS))?;
    let starting = starting_texts(&sources);
    let mut configurations = Vec::with_capacity(form.configurations.len());
    for fields in form.configurations.into_values() {
        let configuration_name: ConfigurationName = fields
            .name
            .trim()
            .parse()
            .map_err(|e: String| unprocessable(&e))?;
        let configuration = Configuration::parse(fields.files(&starting)).map_err(|errors| {
            let listed: Vec<String> = errors
                .iter()
                .map(|file| format!("{}: {}", file.path, file.error))
                .collect();
            unprocessable(&format!(
                "the configuration {configuration_name} cannot be used: {}",
                listed.join("; ")
            ))
        })?;
        configurations.push((configuration_name, configuration));
    }
    match experiments(&sources)
        .launch(&suite, configurations, repetitions, sources.clock.now())
        .await
    {
        Ok(launched) => {
            sources.experiments.changed();
            info!(experiment = %launched.id, suite = %suite, replays = launched.replays, "launched an experiment");
            let said = format!(
                r#"Launched <a class="link" href="{}">experiment {}</a> on the suite {}: {}"#,
                html::escape(&experiment_path(launched.id)),
                launched.id,
                html::escape(suite.as_str()),
                counted(launched.replays, "replay", "replays"),
            );
            Ok((StatusCode::OK, alert("alert-success", &said)).into_response())
        }
        Err(e @ LaunchRefusal::NoSuchSuite(_)) => {
            Err(WRITE.refused(StatusCode::NOT_FOUND, &e.to_string()))
        }
        Err(e @ (LaunchRefusal::NoCases(_) | LaunchRefusal::DuplicateConfiguration(_))) => {
            Err(unprocessable(&e.to_string()))
        }
        Err(LaunchRefusal::Failed(e)) => {
            let why = format!("{e:#}");
            warn!(suite = %suite, error = why, "launching an experiment failed");
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &why))
        }
    }
}
