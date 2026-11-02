//! What is shown of the environment and its builds, by `environment`,
//! `status` and the server page (`docs/specs/monitoring.md`, environment).

use std::fmt::Write as _;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::kept::{self, BuildId, BuildRecord, Builds, Built};
use crate::domain::Timestamp;
use crate::phrase::Phrase;
use crate::status::{Fact, Paint};
use crate::transcript;

/// The environment as `status` shows it (`docs/specs/monitoring.md`,
/// status).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentShown {
    /// `None` while there is no environment yet.
    pub built: Option<BuiltShown>,
    /// When the build in progress started, if one is.
    pub building_since: Option<Timestamp>,
    /// When the latest build that ended failed, if it did.
    pub latest_failed: Option<Timestamp>,
}

/// What the environment was built from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltShown {
    /// The commit of the default branch.
    pub commit: String,
    /// When its build started.
    pub at: Timestamp,
}

/// Why the environment asks for attention (`docs/specs/design_system.md`,
/// Holds and problems).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    /// Its latest build failed, at this time.
    LatestFailed(Timestamp),
    /// There is none yet, and its first build is in progress, since then.
    FirstBuilding(Timestamp),
    /// There is none yet, and no build is in progress.
    NoneYet,
}

impl EnvironmentShown {
    /// Why it asks for attention, if it does: with no environment yet, or
    /// after a failed latest build.
    pub fn attention(&self) -> Option<Attention> {
        match (self.latest_failed, &self.built, self.building_since) {
            (Some(at), _, _) => Some(Attention::LatestFailed(at)),
            (None, Some(_), _) => None,
            (None, None, Some(since)) => Some(Attention::FirstBuilding(since)),
            (None, None, None) => Some(Attention::NoneYet),
        }
    }
}

/// What `environment` answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentAnswer {
    /// `None` without a setup command.
    pub environment: Option<EnvironmentBuilds>,
}

/// The environment and its builds, as `environment` shows them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentBuilds {
    #[serde(flatten)]
    pub shown: EnvironmentShown,
    /// The latest build that ended and, when it succeeded, the latest that
    /// failed, then the build in progress, if any.
    pub builds: Vec<BuildShown>,
}

/// A build, as the maintenance command's object is in `logs --json`, with
/// the commit it built from (`docs/specs/monitoring.md`, environment).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildShown {
    pub build: BuildId,
    #[serde(flatten)]
    pub(crate) record: BuildRecord,
    /// Of a build in progress, its output so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub so_far: Option<SoFar>,
}

/// A build's output in whole lines so far, which then ends `to` bytes in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoFar {
    pub output: String,
    pub to: u64,
}

impl BuildShown {
    /// Whether it had not ended as last read.
    pub fn in_progress(&self) -> bool {
        !self.record.has_ended()
    }

    /// When it ended: when it started, and as long after as it ran, if
    /// that is known.
    fn ended_at(&self) -> Timestamp {
        let ran = self
            .record
            .line
            .ended
            .as_ref()
            .map_or(0.0, |ended| ended.duration_seconds);
        let ran = Duration::try_from_secs_f64(ran).unwrap_or_default();
        self.record.at + chrono::TimeDelta::from_std(ran).unwrap_or_default()
    }
}

/// Whether the latest poll that read the repository's files gave a setup
/// command, as kept under `dir`; `None` before the first.
pub fn setup_given(dir: &Path) -> Option<bool> {
    Builds::new(dir.to_path_buf()).setup_given()
}

/// What is kept of the environment under `dir`, `setup_command` being
/// whether there is a setup command when no poll has said yet.
pub fn answer(dir: &Path, setup_command: bool) -> Result<EnvironmentAnswer> {
    let builds = Builds::new(dir.to_path_buf());
    if !builds.setup_given().unwrap_or(setup_command) {
        return Ok(EnvironmentAnswer { environment: None });
    }
    let building = builds.is_building()?;
    let mut shown = shown(building, builds.named(), builds.records()?);
    if let Some(build) = shown.builds.last_mut().filter(|build| build.in_progress()) {
        let mut following = FollowingBuild::start(dir, build.build, 0);
        let so_far = match following.read_output(false)? {
            Some(Followed::Output { text, to }) => SoFar { output: text, to },
            Some(Followed::Ended(_)) | None => SoFar {
                output: String::new(),
                to: 0,
            },
        };
        build.so_far = Some(so_far);
    }
    Ok(EnvironmentAnswer {
        environment: Some(shown),
    })
}

/// The environment and its builds as shown, from the build the state file
/// names, `built`, and the records of the builds kept, by number, the
/// latest being in progress if it has not ended and a build is `building`.
fn shown(
    building: bool,
    built: Option<Built>,
    mut records: Vec<(BuildId, BuildRecord)>,
) -> EnvironmentBuilds {
    let shown = |(id, record): (BuildId, BuildRecord)| BuildShown {
        build: id,
        record,
        so_far: None,
    };
    let in_progress = records
        .pop_if(|(_, record)| building && !record.has_ended())
        .map(shown);
    let mut ended = records
        .into_iter()
        .rev()
        .filter(|(_, record)| record.has_ended());
    let latest = ended.next().map(shown);
    let latest_failed = match &latest {
        Some(latest) if !latest.record.failed() => {
            ended.find(|(_, record)| record.failed()).map(shown)
        }
        Some(_) | None => None,
    };
    EnvironmentBuilds {
        shown: EnvironmentShown {
            built: built.map(|built| BuiltShown {
                commit: built.commit,
                at: built.at,
            }),
            building_since: in_progress.as_ref().map(|build| build.record.at),
            latest_failed: latest
                .as_ref()
                .filter(|latest| latest.record.failed())
                .map(BuildShown::ended_at),
        },
        builds: [latest, latest_failed, in_progress]
            .into_iter()
            .flatten()
            .collect(),
    }
}

/// The facts `status` and `environment` show of `environment`.
pub(crate) fn facts(environment: &EnvironmentShown) -> Vec<Fact> {
    let built = match &environment.built {
        Some(built) => Fact::Plain(
            Phrase::from("environment: built from ")
                .code(short(&built.commit))
                .text(" ")
                .time(built.at),
        ),
        None => Fact::Warning("environment: none yet".into()),
    };
    let building = environment.building_since.map(|since| {
        Fact::Plain(Phrase::from("environment build: in progress since ").time(since))
    });
    let failed = environment
        .latest_failed
        .map(|at| Fact::Problem(Phrase::from("environment build: the latest failed ").time(at)));
    [Some(built), building, failed]
        .into_iter()
        .flatten()
        .collect()
}

/// A commit as the environment's facts name it.
pub(crate) fn short(commit: &str) -> String {
    commit.chars().take(12).collect()
}

/// `answer` as `environment` prints it: the facts as `status` shows them,
/// then each build.
pub fn render(answer: &EnvironmentAnswer, colour: bool) -> String {
    let paint = Paint(colour);
    let Some(environment) = &answer.environment else {
        return "environment: there is no setup command\n".into();
    };
    let mut text = String::new();
    for fact in facts(&environment.shown) {
        let _ = writeln!(text, "{}", fact.render(paint));
    }
    for build in &environment.builds {
        text.push('\n');
        text.push_str(&render_build(build, colour));
    }
    text
}

/// A build as `environment` prints it: a line saying when it started and
/// from what, then its command's line and output as a transcript shows a
/// maintenance command's, its output whatever its status, and for a build
/// in progress its output so far.
pub fn render_build(build: &BuildShown, colour: bool) -> String {
    let mut text = format!(
        "{}\n{}",
        heading(build),
        render_line(build, "running", colour)
    );
    if let Some(ended) = &build.record.line.ended {
        text.push_str(&render_output(&ended.output));
    }
    if let Some(so_far) = &build.so_far {
        text.push_str(&render_output(&so_far.output));
    }
    text
}

/// The line that says when `build` started, and from what commit.
pub(crate) fn heading(build: &BuildShown) -> Phrase {
    let record = &build.record;
    let heading = Phrase::from(format!("build {}: started ", build.build)).time(record.at);
    match &record.commit {
        Some(commit) => heading.text(", from ").code(short(commit)),
        None => heading,
    }
}

/// How a build has run, as its command's line says after its arrow.
pub(crate) enum Account {
    Running,
    Ended(String),
    Failed(String),
}

/// How `build` has run.
pub(crate) fn account(build: &BuildShown) -> Account {
    match (&build.record.error, &build.record.line.ended) {
        (Some(error), _) => Account::Failed(format!("failed: {error}")),
        (None, Some(ended)) if ended.status.failed() => Account::Failed(ended.account()),
        (None, Some(ended)) => Account::Ended(ended.account()),
        (None, None) => Account::Running,
    }
}

/// A build's command's line, as `environment` prints it: as the build
/// starts, and again once it ends; `unended` in place of its account while
/// its record says it has not.
pub fn render_line(build: &BuildShown, unended: &str, colour: bool) -> String {
    let paint = Paint(colour);
    let account = match account(build) {
        Account::Running => unended.into(),
        Account::Ended(account) => account,
        Account::Failed(account) => paint.red(account),
    };
    format!(
        "setup: {} → {account}\n",
        transcript::shown_command(&build.record.line.command)
    )
}

/// A build's output, or part of it, indented under its command's line.
pub fn render_output(output: &str) -> String {
    output
        .split_inclusive('\n')
        .map(|line| {
            if line.trim().is_empty() {
                "\n".to_string()
            } else {
                format!("  {}", line.trim_end_matches('\n')) + "\n"
            }
        })
        .collect()
}

/// How often a followed build's records are read again.
const FOLLOW_EVERY: Duration = Duration::from_millis(100);

/// What following a build gives next.
#[derive(Clone, Debug, PartialEq)]
pub enum Followed {
    /// More of its output, which then ends `to` bytes in.
    Output { text: String, to: u64 },
    /// It ended, or was abandoned, as its record says.
    Ended(BuildShown),
}

/// A build being followed, from its records.
pub struct FollowingBuild {
    builds: Builds,
    build: BuildId,
    output: PathBuf,
    /// How far into its output has been given.
    offset: u64,
    ended: bool,
}

impl FollowingBuild {
    /// Follows build `build` under `dir`, `offset` bytes into its output.
    pub fn start(dir: &Path, build: BuildId, offset: u64) -> FollowingBuild {
        let builds = Builds::new(dir.to_path_buf());
        let output = builds.build_dir(build).join(kept::OUTPUT);
        FollowingBuild {
            builds,
            build,
            output,
            offset,
            ended: false,
        }
    }

    /// Its output in whole lines as they come, then, once it has ended,
    /// the rest of its output, then its end; `None` after its end.
    pub async fn next(&mut self) -> Result<Option<Followed>> {
        if self.ended {
            return Ok(None);
        }
        loop {
            let record = BuildRecord::read(&self.builds.build_dir(self.build));
            let over = match &record {
                Some(record) => record.has_ended() || !self.builds.is_building()?,
                None => !self.builds.is_building()?,
            };
            if let Some(output) = self.read_output(over)? {
                return Ok(Some(output));
            }
            if over {
                self.ended = true;
                let record = record.context("the build followed has no record")?;
                return Ok(Some(Followed::Ended(BuildShown {
                    build: self.build,
                    record,
                    so_far: None,
                })));
            }
            tokio::time::sleep(FOLLOW_EVERY).await;
        }
    }

    /// Its output past what was given, up to its last whole line unless
    /// `all`, if there is any.
    fn read_output(&mut self, all: bool) -> Result<Option<Followed>> {
        let mut file = match std::fs::File::open(&self.output) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("opening the build's output"),
        };
        file.seek(SeekFrom::Start(self.offset))
            .context("reading the build's output")?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .context("reading the build's output")?;
        let taken = if all {
            bytes.len()
        } else {
            bytes
                .iter()
                .rposition(|&byte| byte == b'\n')
                .map_or(0, |i| i + 1)
        };
        if taken == 0 {
            return Ok(None);
        }
        self.offset += taken as u64;
        Ok(Some(Followed::Output {
            text: String::from_utf8_lossy(&bytes[..taken]).into_owned(),
            to: self.offset,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::lifecycle::Fingerprint;
    use crate::transcript::{MaintenanceEnded, MaintenanceLine, MaintenanceStatus};

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// A build's record, started at `started`, ended a minute later with
    /// `status`, if any.
    fn record(started: &str, status: Option<MaintenanceStatus>) -> BuildRecord {
        BuildRecord {
            line: MaintenanceLine {
                command: "npm ci".into(),
                ended: status.map(|status| MaintenanceEnded {
                    status,
                    duration_seconds: 60.0,
                    output: String::new(),
                }),
            },
            commit: Some("0123456789abcdef".into()),
            at: at(started),
            error: None,
        }
    }

    fn built(id: u64, started: &str) -> Built {
        Built {
            id: BuildId(id),
            commit: "0123456789abcdef".into(),
            at: at(started),
            fingerprint: Fingerprint::of::<&str, &str>("npm ci", []),
        }
    }

    const OK: Option<MaintenanceStatus> = Some(MaintenanceStatus::Exited(0));
    const FAILED: Option<MaintenanceStatus> = Some(MaintenanceStatus::Exited(1));

    /// The numbers of the builds shown, in order.
    fn numbers(shown: &EnvironmentBuilds) -> Vec<String> {
        shown
            .builds
            .iter()
            .map(|build| build.build.to_string())
            .collect()
    }

    /// `docs/specs/monitoring.md`, environment.
    #[test_log::test]
    fn the_latest_build_shows_then_the_latest_that_failed_when_it_succeeded_then_the_one_in_progress()
     {
        let shown = shown(
            true,
            Some(built(3, "2026-10-01T12:03:00Z")),
            vec![
                (BuildId(1), record("2026-10-01T12:01:00Z", FAILED)),
                (BuildId(2), record("2026-10-01T12:02:00Z", FAILED)),
                (BuildId(3), record("2026-10-01T12:03:00Z", OK)),
                (BuildId(4), record("2026-10-01T12:04:00Z", None)),
            ],
        );

        assert_eq!(numbers(&shown), ["3", "2", "4"]);
        assert_eq!(
            shown.shown,
            EnvironmentShown {
                built: Some(BuiltShown {
                    commit: "0123456789abcdef".into(),
                    at: at("2026-10-01T12:03:00Z"),
                }),
                building_since: Some(at("2026-10-01T12:04:00Z")),
                latest_failed: None,
            }
        );
    }

    #[test_log::test]
    fn a_latest_build_that_failed_shows_alone_with_when_it_ended() {
        let shown = shown(
            false,
            Some(built(1, "2026-10-01T12:01:00Z")),
            vec![
                (BuildId(1), record("2026-10-01T12:01:00Z", OK)),
                (BuildId(2), record("2026-10-01T12:02:00Z", FAILED)),
            ],
        );

        assert_eq!(numbers(&shown), ["2"]);
        assert_eq!(
            shown.shown.attention(),
            Some(Attention::LatestFailed(at("2026-10-01T12:03:00Z")))
        );
    }

    #[test_log::test]
    fn a_build_that_has_not_ended_with_none_holding_the_lock_was_abandoned_and_does_not_show() {
        let shown = shown(
            false,
            None,
            vec![(BuildId(1), record("2026-10-01T12:01:00Z", None))],
        );

        assert_eq!(numbers(&shown), Vec::<String>::new());
        assert_eq!(shown.shown.attention(), Some(Attention::NoneYet));
    }

    #[test_log::test]
    fn a_first_build_in_progress_asks_for_attention_and_one_beside_an_environment_does_not() {
        let first = shown(
            true,
            None,
            vec![(BuildId(1), record("2026-10-01T12:01:00Z", None))],
        );
        let rebuilding = shown(
            true,
            Some(built(1, "2026-10-01T12:01:00Z")),
            vec![
                (BuildId(1), record("2026-10-01T12:01:00Z", OK)),
                (BuildId(2), record("2026-10-01T12:02:00Z", None)),
            ],
        );

        assert_eq!(
            first.shown.attention(),
            Some(Attention::FirstBuilding(at("2026-10-01T12:01:00Z")))
        );
        assert_eq!(rebuilding.shown.attention(), None);
    }
}
