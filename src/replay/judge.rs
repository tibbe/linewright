//! Judging a replay's output against the criteria of its case, in a
//! session of the judge's own (`docs/specs/benchmarks.md`, Judging).

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use super::{BUILD_LOG, CASE, COMMITS, OUTCOME, Setting, WRITES, in_own_dir, write_whole};
use crate::apply::hand_back;
use crate::briefs::Briefs;
use crate::case::Case;
use crate::clock::{Clock, SystemClock};
use crate::clones::Clones;
use crate::domain::Timestamp;
use crate::factory::FactoryConfig;
use crate::git;
use crate::harness::{Harness, NextTurn, Purpose, Reporter};
use crate::prompts::{self, Repo};
use crate::results::{self, Correctness, JudgeResult, Leak};
use crate::run::RunWith;
use crate::runs::{Ending, OutcomeName, Ran, RunId};
use crate::session::{
    self, Beside, Cancellation, Failure, RecordsSessions, SessionCtx, Sessions, converse, infra,
    lock_set_whole, open_session, parse_retry,
};
use crate::winding_up::WindingUp;
use crate::work_settings::{RunKind, SessionSettings};
use crate::{COMMAND, VERSION_AND_COMMIT};

mod copy;

pub use copy::ReplayCopy;

const JUDGEMENT: &str = "judgement.json";

/// How a replay's run ended, of the endings a judge judges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgeable {
    Done,
    HandedBack,
}

/// The output of a replay that a judge can judge: its run ended done or
/// handed back, and it was replayed from the case it is judged against.
#[derive(Clone, Debug)]
pub struct Replayed {
    /// Absolute.
    dir: PathBuf,
    outcome: Judgeable,
    /// The branch of the replay's last push that its commits hold, if any.
    pushed: Option<String>,
}

/// Why an output cannot be judged.
#[derive(Debug, thiserror::Error)]
pub enum JudgeRefusal {
    #[error("the output's directory cannot be used: {0}")]
    Unusable(std::io::Error),
    #[error("the output has no `{OUTCOME}` part")]
    NoOutcome,
    #[error(
        "the replay's outcome is `{0}`: only a replay whose run ended done or handed back is judged"
    )]
    Outcome(String),
    #[error("the output has no `{CASE}` part, which names the run its case was captured from")]
    NoCase,
    #[error("the output's `{CASE}` part names the run {named}, not {case}, the run of the case")]
    OtherRun { named: String, case: RunId },
    #[error("the output's `{part}` part cannot be read: {error}")]
    Unreadable { part: &'static str, error: String },
}

impl Replayed {
    /// The output in `dir`, replayed from `case`.
    ///
    /// # Errors
    ///
    /// When the output's outcome is missing or other than done or handed
    /// back, its `case` part is missing or names a run other than `case`'s,
    /// or a part cannot be read.
    pub async fn open(dir: &Path, case: &Case) -> Result<Replayed, JudgeRefusal> {
        let dir = std::path::absolute(dir).map_err(JudgeRefusal::Unusable)?;
        let ended = read_part(&dir, OUTCOME)?.ok_or(JudgeRefusal::NoOutcome)?;
        let outcome = match serde_json::from_value(serde_json::Value::String(ended.clone())) {
            Ok(OutcomeName::Ended(Ending::Done)) => Judgeable::Done,
            Ok(OutcomeName::Ended(Ending::HandedBack)) => Judgeable::HandedBack,
            Ok(_) | Err(_) => return Err(JudgeRefusal::Outcome(ended)),
        };
        let named = read_part(&dir, CASE)?.ok_or(JudgeRefusal::NoCase)?;
        if named.parse::<RunId>().as_ref() != Ok(case.run()) {
            return Err(JudgeRefusal::OtherRun {
                named,
                case: case.run().clone(),
            });
        }
        let unreadable = |error: anyhow::Error| JudgeRefusal::Unreadable {
            part: WRITES,
            error: format!("{error:#}"),
        };
        let mut pushed = None;
        for branch in pushes(&dir.join(WRITES))
            .map_err(unreadable)?
            .into_iter()
            .rev()
        {
            if holds(&dir.join(COMMITS), &branch).await {
                pushed = Some(branch);
                break;
            }
        }
        Ok(Replayed {
            dir,
            outcome,
            pushed,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn outcome(&self) -> Judgeable {
        self.outcome
    }
}

/// The text of the output's part `name`, its trailing white space trimmed,
/// or `None` when there is none.
fn read_part(dir: &Path, name: &'static str) -> Result<Option<String>, JudgeRefusal> {
    match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => Ok(Some(text.trim_end().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(JudgeRefusal::Unreadable {
            part: name,
            error: e.to_string(),
        }),
    }
}

/// A push, as the writes keep it.
#[derive(Deserialize)]
struct Push {
    branch: String,
}

/// The branch of each push kept in the writes in `writes`, in the order
/// they were made.
fn pushes(writes: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(writes) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut pushes = vec![];
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let number = name
            .strip_suffix("-push.json")
            .and_then(|number| number.parse::<u32>().ok());
        if let Some(number) = number {
            pushes.push((number, name));
        }
    }
    pushes.sort();
    pushes
        .into_iter()
        .map(|(_, name)| {
            let text = std::fs::read_to_string(writes.join(&name))?;
            let push: Push = serde_json::from_str(&text).with_context(|| name.clone())?;
            Ok(push.branch)
        })
        .collect()
}

/// Whether the bare repository `repository` holds `branch`: a push the
/// writes keep is kept before it is made, and it may then have failed.
async fn holds(repository: &Path, branch: &str) -> bool {
    git::git(
        repository,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
    .is_ok()
}

/// The record of a judge's session: its directory under the output's
/// `judge/`, and what the session ran with once it is opened. It is removed
/// when dropped, unless the judgement that names it is kept.
#[derive(Debug)]
pub struct JudgeRecord {
    /// Absolute.
    dir: PathBuf,
    /// Relative to the output.
    relative: PathBuf,
    ran: Arc<Mutex<Option<Ran>>>,
    kept: bool,
}

impl JudgeRecord {
    /// A new, empty directory for the record of a judge of `replayed`.
    ///
    /// # Errors
    ///
    /// When the directory cannot be made.
    pub fn make(replayed: &Replayed) -> Result<JudgeRecord> {
        let dir = session::new_record_dir(&replayed.dir, &Purpose::Judge, SystemClock.now())
            .context("making the record of the judge's session")?;
        let relative = dir
            .strip_prefix(&replayed.dir)
            .context("the judge's record is in the output")?
            .to_path_buf();
        Ok(JudgeRecord {
            dir,
            relative,
            ran: Arc::default(),
            kept: false,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Empties the record for the session's retry.
    fn restart(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.dir)
            .and_then(|()| std::fs::create_dir(&self.dir))
            .context("emptying the record of the judge's session")
    }
}

impl Drop for JudgeRecord {
    fn drop(&mut self) {
        if !self.kept {
            discard(&self.dir);
        }
    }
}

/// Removes the judge's record in `dir`, and the output's `judge/` with it
/// when that holds no other, logging when it cannot.
pub fn discard(dir: &Path) {
    let mut removed = std::fs::remove_dir_all(dir);
    for _ in 0..2 {
        match &removed {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                removed = std::fs::remove_dir_all(dir);
            }
            _ => break,
        }
    }
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            warn!(dir = %dir.display(), error = %e, "removing the judge's record failed");
        }
        _ => {
            if let Some(judges) = dir.parent() {
                let _ = std::fs::remove_dir(judges);
            }
        }
    }
}

/// The judge's one session is recorded in the record's directory, and what
/// it ran with is kept for its judgement.
#[async_trait::async_trait]
impl RecordsSessions for JudgeRecord {
    fn record_dir(&self, _purpose: &Purpose, _now: Timestamp) -> Result<PathBuf> {
        Ok(self.dir.clone())
    }

    async fn session(&self, _dir: &Path, settings: &SessionSettings, briefs: &Briefs) -> Reporter {
        *lock_set_whole(&self.ran) = Some(Ran::new(settings, briefs));
        let ran = self.ran.clone();
        Reporter::new(move |reported| {
            if let Some(ran) = lock_set_whole(&ran).as_mut() {
                ran.reported_model = Some(reported.model);
            }
        })
    }
}

/// What a judge decided about a replay, made only from its accepted
/// result. Kept as `judgement.json` in the output, its record with it.
#[derive(Debug, Serialize)]
pub struct Judgement {
    correctness: Correctness,
    leak: Leak,
    session: JudgeSession,
    /// The version of Linewright that ran the judge.
    linewright: String,
    #[serde(skip)]
    record: JudgeRecord,
}

/// The judge's session: where its record is, relative to the output, and
/// what it ran with.
#[derive(Debug, Serialize)]
struct JudgeSession {
    record: PathBuf,
    #[serde(flatten)]
    ran: Ran,
}

impl Judgement {
    /// The verdict and whether a leak was found, one to a line.
    pub fn summary(&self) -> String {
        let leak = if self.leak.found() {
            "found"
        } else {
            "none found"
        };
        format!(
            "Verdict: {}\nLeak: {leak}\n",
            self.correctness.verdict.name()
        )
    }
}

/// Why a judge made no judgement.
#[derive(Debug)]
pub enum Unjudged {
    GaveUp(String),
    /// Its session ended, failing or timing out or with no result accepted,
    /// for `reason`, with the end of the log that shows it, if any.
    Ended {
        reason: String,
        log_tail: Option<String>,
    },
    UsageLimit {
        resets_at: Timestamp,
    },
    Failed(anyhow::Error),
}

impl fmt::Display for Unjudged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unjudged::GaveUp(reason) => write!(f, "the judge gave up: {reason}"),
            Unjudged::Ended { reason, log_tail } => {
                write!(f, "the judge's session ended without a judgement: {reason}")?;
                match log_tail {
                    Some(tail) => write!(f, "\n\n{tail}"),
                    None => Ok(()),
                }
            }
            Unjudged::UsageLimit { resets_at } => write!(
                f,
                "the judge's session reached the usage limit, which resets at {resets_at}"
            ),
            Unjudged::Failed(error) => write!(f, "the judge could not judge: {error:#}"),
        }
    }
}

impl std::error::Error for Unjudged {}

impl From<Failure> for Unjudged {
    fn from(failure: Failure) -> Unjudged {
        match failure {
            Failure::HandBack(hand_back) => Unjudged::Ended {
                reason: hand_back.reason,
                log_tail: hand_back.log_tail.map(|tail| tail.text().to_string()),
            },
            Failure::UsageLimit(limit) => Unjudged::UsageLimit {
                resets_at: limit.resets_at,
            },
            Failure::Infrastructure { error, .. } | Failure::Apply { error, .. } => {
                Unjudged::Failed(error)
            }
            Failure::Cancelled => Unjudged::Failed(anyhow!("the judge's session was cancelled")),
        }
    }
}

/// Runs a judge of `replayed`, the output of a replay of `case`, with
/// `config` and `harness`, in the setting a replay of `case` has, its
/// session recorded in `record`. Its clone has the branch the replay
/// pushed last checked out, from the output's commits, or the case's
/// default branch when it pushed none. A session that fails on its
/// infrastructure is retried once, as a stage's is.
///
/// # Errors
///
/// When the judge gives up, its session fails, times out or ends with no
/// result accepted, the usage limit is reached, or the setting cannot be
/// prepared. The record is removed then, and a failure's error is redacted.
pub async fn run(
    case: &Case,
    replayed: &Replayed,
    record: JudgeRecord,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
) -> Result<Judgement, Unjudged> {
    let redaction = config.secrets.redaction().clone();
    let (work_dir, winding_up) = (config.work_dir.clone(), config.winding_up.clone());
    let judged = in_own_dir(&work_dir, winding_up, "judge", async |own_dir, clones| {
        judge_in(case, replayed, &record, harness, &config, own_dir, clones).await
    })
    .await
    .map_err(Unjudged::Failed)
    .flatten()
    .map_err(|unjudged| match unjudged {
        Unjudged::Failed(error) => {
            Unjudged::Failed(anyhow!(redaction.apply(&format!("{error:#}"))))
        }
        unjudged @ (Unjudged::GaveUp(_) | Unjudged::Ended { .. } | Unjudged::UsageLimit { .. }) => {
            unjudged
        }
    })?;
    let (correctness, leak) = judged;
    let ran = lock_set_whole(&record.ran)
        .take()
        .ok_or_else(|| Unjudged::Failed(anyhow!("the judge's session is not recorded")))?;
    Ok(Judgement {
        correctness,
        leak,
        session: JudgeSession {
            record: record.relative.clone(),
            ran,
        },
        linewright: format!("{COMMAND} {VERSION_AND_COMMIT}"),
        record,
    })
}

/// [`run`], in the judge's own directory `own_dir`, whose clones
/// `clones_winding_up` winds up.
async fn judge_in(
    case: &Case,
    replayed: &Replayed,
    record: &JudgeRecord,
    harness: Arc<dyn Harness>,
    config: &FactoryConfig,
    own_dir: &Path,
    clones_winding_up: &WindingUp,
) -> Result<(Correctness, Leak), Unjudged> {
    let repository = own_dir.join(COMMITS);
    let built = Mutex::new(None);
    let prepared = Setting::prepare(
        case,
        &RunKind::Judge,
        config,
        harness.clone(),
        &own_dir.join("store"),
        &repository,
        |output| {
            *lock_set_whole(&built) = Some(output.to_string());
            Ok(())
        },
    )
    .await;
    let built = lock_set_whole(&built).take();
    let setting = match (prepared, built) {
        (Ok(setting), built) => {
            if let Some(output) = built {
                std::fs::write(record.dir().join(BUILD_LOG), output)
                    .context("writing the build's output")
                    .map_err(Unjudged::Failed)?;
            }
            setting
        }
        (Err(e), built) => {
            if let Some(output) = built {
                error!(output, "the build of the judge's environment");
            }
            return Err(Unjudged::Failed(e));
        }
    };
    if let Some(branch) = &replayed.pushed {
        fetch_pushed(&repository, &replayed.dir.join(COMMITS), branch)
            .await
            .map_err(Unjudged::Failed)?;
    }
    let sessions = setting.sessions(
        case,
        setting.store.clone(),
        harness,
        config,
        Clones::new(own_dir, clones_winding_up.clone()),
    );
    let with = RunWith {
        briefs: setting.repo.briefs.clone(),
        settings: config
            .work
            .for_run(RunKind::Judge, &setting.repo.settings, &config.environments),
    };
    let mut retries = 0;
    let result = loop {
        match session(case, replayed, record, config, &sessions, &with).await {
            Err(Failure::Infrastructure { error, .. })
                if hand_back::retries_infrastructure_failure(retries) =>
            {
                retries += 1;
                let error = config.secrets.redaction().apply(&format!("{error:#}"));
                warn!(
                    error,
                    "the judge's session failed on its infrastructure; retrying once"
                );
                record.restart().map_err(Unjudged::Failed)?;
            }
            ended => break ended?,
        }
    };
    drop(setting);
    match result {
        JudgeResult::Judged { correctness, leak } => Ok((correctness, leak)),
        JudgeResult::GaveUp { reason } => Err(Unjudged::GaveUp(reason)),
    }
}

/// The judge's session in `sessions`, recorded in `record`, on what the
/// replay left: sent the filled `briefs/prompts/judge.md`, until its result is
/// accepted.
async fn session(
    case: &Case,
    replayed: &Replayed,
    record: &JudgeRecord,
    config: &FactoryConfig,
    sessions: &Sessions,
    with: &RunWith,
) -> Result<JudgeResult, Failure> {
    let cancel = Cancellation::default();
    let ctx = SessionCtx {
        with,
        cancel: &cancel,
        records: record,
    };
    let branch = replayed
        .pushed
        .as_deref()
        .unwrap_or(&sessions.default_branch);
    let (opened, ws, ()) =
        open_session(sessions, ctx, Purpose::Judge, branch, async |_| Ok(())).await?;
    let copying = async { ReplayCopy::make(replayed, &ws.make_beside(Beside::Replay)?).await };
    let copy = copying
        .await
        .context("copying the replay for the judge")
        .map_err(|error| opened.ended(sessions, infra(error)))?;
    let settings = &case.properties().settings;
    let repo = Repo {
        name: &settings.repo,
        default_branch: &sessions.default_branch,
        spec_glob: config.spec_glob.pattern(),
    };
    let briefs = &with.briefs;
    let (mut spec, _attachments) = opened.spec(|_| {
        prompts::judge(
            &briefs.templates,
            &repo,
            case.stage().kind(),
            case.criteria(),
            &copy,
        )
    });
    let (result, _) =
        converse(
            sessions,
            ctx,
            &mut spec,
            NextTurn::Start,
            |message| async move {
                results::parse::<JudgeResult>(&message).map_err(parse_retry(briefs))
            },
            |result| async move { Ok(result) },
        )
        .await?;
    Ok(result)
}

/// Fetches `branch` from the output's commits `commits` into the bare
/// repository `repository`, in place of the branch it holds, if any.
async fn fetch_pushed(repository: &Path, commits: &Path, branch: &str) -> Result<()> {
    let commits = commits
        .to_str()
        .context("the output's commits are at a UTF-8 path")?;
    git::git(
        repository,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            commits,
            &format!("+refs/heads/{branch}:refs/heads/{branch}"),
        ],
    )
    .await
    .with_context(|| {
        format!("fetching {branch}, the branch the replay pushed, from its commits")
    })?;
    Ok(())
}

/// A judgement as `judgement.json` holds it, as far as keeping a new one
/// reads it.
#[derive(Deserialize)]
struct Kept {
    session: KeptSession,
}

#[derive(Deserialize)]
struct KeptSession {
    record: PathBuf,
}

/// Keeps `judgement` in `replayed`'s output in place of the earlier
/// judgement, if any, so that a reader sees the one or the other whole, and
/// then removes the record of the earlier judge's session.
///
/// # Errors
///
/// When the judgement cannot be written: the earlier one is kept then, and
/// nothing of `judgement`.
pub fn keep(replayed: &Replayed, mut judgement: Judgement) -> Result<()> {
    let path = replayed.dir.join(JUDGEMENT);
    let earlier = earlier_record(&path);
    let mut text = serde_json::to_string_pretty(&judgement)?;
    text.push('\n');
    let own = judgement
        .session
        .record
        .file_name()
        .context("the judge's record has a name")?
        .to_string_lossy()
        .into_owned();
    write_whole(
        &path,
        &replayed.dir.join(format!(".{JUDGEMENT}-{own}")),
        &text,
    )?;
    judgement.record.kept = true;
    if let Some(earlier) = earlier
        && earlier != judgement.session.record
    {
        discard(&replayed.dir.join(earlier));
    }
    Ok(())
}

/// The record that the judgement at `path` names, relative to the output,
/// if there is one and it is a judge's record under `judge/`.
fn earlier_record(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    let record = serde_json::from_str::<Kept>(&text).ok()?.session.record;
    let judges = session::purpose_name(&Purpose::Judge);
    match record.components().collect::<Vec<_>>().as_slice() {
        [Component::Normal(dir), Component::Normal(_)] if *dir == judges.as_str() => Some(record),
        _ => None,
    }
}
