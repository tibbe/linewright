//! Cases: a spec change or issue the factory handled, captured on disk and
//! run again through the whole pipeline against a local store
//! (`docs/specs/product.md`, "Replay").
//!
//! A case directory holds `case.json` (the repository's facts and the event),
//! `repo.bundle` (the repository at the event's base commit, and the merge
//! of a spec change) and, when captured on the factory's machine,
//! `traces/<subject>/` with the sessions that handled it.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::clock::Clock;
use crate::domain::{Login, MergeMethods, Number, UpdateMethod, UpdateRule, label};
use crate::factory::{Factory, FactoryConfig};
use crate::git;
use crate::reconcile::Subject;
use crate::run::subject_name;
use crate::runtime::Runtime;
use crate::store::github::GitHub;
use crate::store::local::{LocalStore, Seed};
use crate::store::{Categories, Store};
use crate::tracking;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub repo: String,
    pub default_branch: String,
    pub machine_user: Login,
    pub issue_types: Option<Vec<String>>,
    /// `rebase` exactly where the default branch requires linear history.
    pub update_method: UpdateMethod,
    /// The merge methods the repository allows; every method in cases
    /// captured before they were recorded.
    #[serde(default = "MergeMethods::all")]
    pub allowed_merge_methods: MergeMethods,
    /// The commit the event starts from.
    pub base: String,
    pub event: Event,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Event {
    SpecChange {
        number: Number,
        title: String,
        body: String,
        author: Login,
        /// The default branch after the merge.
        merge_commit: String,
        commits: Vec<CaseCommit>,
    },
    Issue {
        number: Number,
        title: String,
        body: String,
        author: Login,
        comments: Vec<CaseComment>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCommit {
    pub sha: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseComment {
    pub author: Login,
    pub body: String,
}

const BASE_REF: &str = "refs/heads/linewright-base";
const MERGE_REF: &str = "refs/heads/linewright-merge";

/// Writes the case of `number` (a merged spec change or an issue) to `dir`,
/// with the traces of the sessions that handled it from `sessions`.
pub async fn capture(github: &GitHub, number: Number, dir: &Path, sessions: &Path) -> Result<()> {
    if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
        bail!("{} is not empty", dir.display());
    }
    std::fs::create_dir_all(dir)?;
    let settings = github.settings().await?;
    let machine = github.machine_user();
    let issue = github.repo_get(&format!("/issues/{}", number.0)).await?;
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let author = |v: &Value| Login::new(text(&v["user"]["login"]));
    let mut handled = vec![];
    let (event, base, merge) = if issue["pull_request"].is_object() {
        let pr = github.repo_get(&format!("/pulls/{}", number.0)).await?;
        let merge = pr["merge_commit_sha"]
            .as_str()
            .filter(|_| pr["merged"] == true);
        let merge =
            merge.with_context(|| format!("{number} is a pull request that was not merged"))?;
        let commit = github.repo_get(&format!("/commits/{merge}")).await?;
        let base = text(&commit["parents"][0]["sha"]);
        let commits = github
            .repo_get(&format!("/pulls/{}/commits?per_page=100", number.0))
            .await?;
        let commits = commits
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| CaseCommit {
                sha: text(&c["sha"]),
                message: text(&c["commit"]["message"]),
            })
            .collect();
        handled.push(Subject::SpecChange(number));
        handled.extend(tickets_of(github, &machine, number, &text(&pr["title"])).await?);
        let event = Event::SpecChange {
            number,
            title: text(&pr["title"]),
            body: text(&pr["body"]),
            author: author(&pr),
            merge_commit: merge.to_string(),
            commits,
        };
        (event, base, Some(merge.to_string()))
    } else {
        let created = text(&issue["created_at"]);
        let commits = github
            .repo_get(&format!(
                "/commits?sha={}&until={created}&per_page=1",
                settings.default_branch
            ))
            .await?;
        let base = text(&commits[0]["sha"]);
        let comments = github
            .repo_get(&format!("/issues/{}/comments?per_page=100", number.0))
            .await?;
        let comments = comments
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| CaseComment {
                author: author(c),
                body: text(&c["body"]),
            })
            .collect();
        handled.push(Subject::Issue(number));
        let event = Event::Issue {
            number,
            title: text(&issue["title"]),
            body: text(&issue["body"]),
            author: author(&issue),
            comments,
        };
        (event, base, None)
    };
    let work = tempfile::tempdir()?;
    let clone = work.path().join("repo");
    github.clone_repo(&clone, &settings.default_branch).await?;
    git::git(&clone, &["update-ref", BASE_REF, &base]).await?;
    let mut refs = vec![BASE_REF];
    if let Some(merge) = &merge {
        git::git(&clone, &["update-ref", MERGE_REF, merge]).await?;
        refs.push(MERGE_REF);
    }
    let bundle = dir.canonicalize()?.join("repo.bundle");
    let bundle = bundle.to_str().context("case paths are UTF-8")?;
    let mut args = vec!["bundle", "create", "--quiet", bundle];
    args.extend(&refs);
    git::git(&clone, &args).await?;
    let case = Case {
        repo: github.full_name(),
        default_branch: settings.default_branch.clone(),
        machine_user: machine,
        issue_types: match settings.categories {
            Categories::IssueTypes(types) => Some(types),
            Categories::Labels => None,
        },
        update_method: match settings.update_rule {
            UpdateRule::Always(UpdateMethod::Rebase) => UpdateMethod::Rebase,
            UpdateRule::Always(UpdateMethod::Merge) | UpdateRule::RebaseOnConflict => {
                UpdateMethod::Merge
            }
        },
        allowed_merge_methods: settings.allowed_merge_methods,
        base,
        event,
    };
    std::fs::write(dir.join("case.json"), serde_json::to_string_pretty(&case)?)?;
    for subject in handled {
        let name = subject_name(subject);
        let from = sessions.join(&name);
        if from.is_dir() {
            copy_dir(&from, &dir.join("traces").join(&name))?;
        }
    }
    Ok(())
}

/// The tickets Linewright cut a spec change into: the sub-issues of its
/// tracking issue.
async fn tickets_of(
    github: &GitHub,
    machine: &Login,
    pr: Number,
    title: &str,
) -> Result<Vec<Subject>> {
    let issues = github
        .repo_get(&format!("/issues?creator={machine}&state=all&per_page=100"))
        .await?;
    let tracking_title = tracking::title(title);
    let reference = format!("{pr}.");
    let Some(tracking_issue) = issues.as_array().into_iter().flatten().find(|i| {
        i["title"] == tracking_title.as_str()
            && i["body"]
                .as_str()
                .is_some_and(|b| b.lines().next().is_some_and(|l| l.ends_with(&reference)))
    }) else {
        return Ok(vec![]);
    };
    let number = tracking_issue["number"]
        .as_u64()
        .context("issues have numbers")?;
    let subs = github
        .repo_get(&format!("/issues/{number}/sub_issues?per_page=100"))
        .await?;
    Ok(subs
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["number"].as_u64())
        .map(|n| Subject::Issue(Number(n)))
        .collect())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Runs the case in `case_dir` through the whole pipeline against a local
/// store in `out`, until nothing is left to do. `out` ends up holding the
/// store's final state as files and every session's record under
/// `sessions/`.
pub async fn replay(
    case_dir: &Path,
    out: &Path,
    runtime: Arc<dyn Runtime>,
    clock: Arc<dyn Clock>,
    mut config: FactoryConfig,
) -> Result<()> {
    if out.exists() && std::fs::read_dir(out)?.next().is_some() {
        bail!("{} is not empty", out.display());
    }
    let case: Case = serde_json::from_str(
        &std::fs::read_to_string(case_dir.join("case.json")).context("reading case.json")?,
    )?;
    let store = Arc::new(
        LocalStore::create(
            out,
            Seed {
                default_branch: case.default_branch.clone(),
                machine_user: case.machine_user.clone(),
                issue_types: case.issue_types.clone(),
                linear_history: case.update_method == UpdateMethod::Rebase,
                allowed_merge_methods: case.allowed_merge_methods.clone(),
            },
            clock.clone(),
        )
        .await?,
    );
    store
        .import_bundle(&case_dir.join("repo.bundle"), &case.base)
        .await?;
    match &case.event {
        Event::SpecChange {
            title,
            body,
            author,
            merge_commit,
            ..
        } => {
            store
                .merge_commit(author.as_str(), title, body, merge_commit)
                .await?;
        }
        Event::Issue {
            title,
            body,
            author,
            comments,
            ..
        } => {
            // A ticket the factory wrote is already briefed; anything else
            // starts at triage. The factory's own comments are its output,
            // not the case.
            let entry = if *author == case.machine_user {
                label::READY_FOR_AGENT
            } else {
                label::NEEDS_TRIAGE
            };
            let issue = store.open_issue(author.as_str(), title, body, &[entry]);
            for comment in comments.iter().filter(|c| c.author != case.machine_user) {
                store.comment_as(comment.author.as_str(), issue, &comment.body)?;
            }
        }
    }
    // The implementer's own run of the repository's checks stands in for CI.
    config.ci_grace = chrono::TimeDelta::zero();
    // Workflows, whose agents read GitHub, are not captured or replayed.
    config.schedule_workflows = false;
    config.work_dir = out.canonicalize()?;
    let mut factory = Factory::start(store.clone(), runtime, clock, config).await?;
    for cycle in 1..=500 {
        let actions = factory.cycle().await?;
        store.export().await?;
        if actions.is_empty() && factory.is_idle() {
            info!(cycles = cycle, "replay finished: nothing left to do");
            return Ok(());
        }
        if !factory.is_idle() {
            factory.next_finished().await;
        }
    }
    factory.stop().await;
    bail!("the replay did not settle within 500 polls")
}
