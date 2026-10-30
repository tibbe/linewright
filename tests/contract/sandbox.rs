use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use linewright::clock::SystemClock;
use linewright::domain::label::Label;
use linewright::domain::{IssueCloseReason, Login, MergeMethods, Number, Repo, Timestamp, Windows};
use linewright::store::Store;
use linewright::store::github::GitHub;
use linewright::store::local::{LocalStore, Seed};
use serde_json::Value;
use tempfile::TempDir;

/// One promise of the `Store` trait, checked through the trait alone.
pub type Contract = fn(Sandbox) -> BoxFuture<'static, Result<()>>;

/// How long GitHub may take to answer a read with what a write just wrote,
/// as its search and timeline indexes do.
const GITHUB_LAG: Duration = Duration::from_secs(20);

/// How long a contract on GitHub may run, at the longest. The sweep leaves
/// alone what was written to since, as a contract of another run, such as a
/// trial nextest runs in a process of its own, may still be using it.
const LONGEST_CONTRACT: chrono::TimeDelta = chrono::TimeDelta::minutes(10);

/// What starts the name of everything a contract makes, so that the sweep
/// closes nothing else, even in a repository that is not a sandbox.
const MARK: &str = "contract-";

/// The repository the local store stands in for.
const LOCAL_REPO: &str = "contract/sandbox";

/// Who Linewright acts as on the local store: not GitHub's account, so that
/// no contract can pass by assuming it.
const LOCAL_LINEWRIGHT: &str = "contract-linewright";

/// What a contract runs against: a store and what it shares with the other
/// contracts on it.
pub struct Sandbox {
    store: Arc<dyn Store>,
    repo: Repo,
    /// How long the store may take to answer a read with what a write just
    /// wrote.
    lag: Duration,
    run: String,
    scratch: TempDir,
}

impl Sandbox {
    /// A fresh local store, in a directory of its own.
    pub async fn local() -> Result<Sandbox> {
        let scratch = tempfile::tempdir()?;
        let repo: Repo = LOCAL_REPO.parse()?;
        let store = LocalStore::create(
            &scratch.path().join("store"),
            Seed {
                repo: repo.clone(),
                default_branch: "main".into(),
                linewright: Login::new(LOCAL_LINEWRIGHT),
                issue_types: None,
                linear_history: false,
                allowed_merge_methods: MergeMethods::all(),
            },
            Arc::new(SystemClock),
        )
        .await?;
        Ok(Sandbox::new(Arc::new(store), repo, Duration::ZERO, scratch))
    }

    /// The GitHub store on the sandbox repository `repo`.
    pub fn github(github: GitHub, repo: Repo) -> Result<Sandbox> {
        Ok(Sandbox::new(
            Arc::new(github),
            repo,
            GITHUB_LAG,
            tempfile::tempdir()?,
        ))
    }

    fn new(store: Arc<dyn Store>, repo: Repo, lag: Duration, scratch: TempDir) -> Sandbox {
        let run = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        Sandbox {
            store,
            repo,
            lag,
            run,
            scratch,
        }
    }

    pub fn store(&self) -> &dyn Store {
        &*self.store
    }

    /// The repository the store is of.
    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// `name`, made unique to this contract's run, for the title of an
    /// issue or pull request or the name of a branch or label, which earlier
    /// runs in the same sandbox may have left behind. Everything a contract
    /// makes is named so, for the sweep to find.
    pub fn unique(&self, name: &str) -> String {
        format!("{MARK}{name}-{}", self.run)
    }

    /// The first answer `check` gives without failing: on GitHub, which
    /// may answer a read with what it held before a write, it is asked again
    /// until it passes or [`GITHUB_LAG`] has passed; the local store answers
    /// at once, so it is asked once.
    pub async fn eventually<T, F, Fut>(&self, mut check: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let until = Instant::now() + self.lag;
        loop {
            match check().await {
                Ok(answer) => return Ok(answer),
                Err(error) if Instant::now() >= until => return Err(error),
                Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        }
    }

    /// Creates a label for the contract, named for `what`, and returns its
    /// name.
    pub async fn new_label(&self, what: &str, description: &'static str) -> Result<String> {
        let name = self.unique(what);
        self.store()
            .create_label(&label(&name, description))
            .await?;
        Ok(name)
    }

    /// A directory of the contract's own, such as for a clone.
    #[allow(dead_code, reason = "for the contracts on branches and files")]
    pub fn scratch(&self) -> &Path {
        self.scratch.path()
    }
}

/// The label `name`, meaning `description`.
pub fn label(name: &str, description: &'static str) -> Label {
    Label {
        // A contract's labels are few, and live as long as the run.
        name: name.to_string().leak(),
        description,
    }
}

/// Windows that leave out what earlier runs closed, which piles up in the
/// sandbox.
pub fn recent() -> Windows {
    let hour = chrono::TimeDelta::hours(1);
    Windows::at(chrono::Utc::now(), hour, hour)
}

/// Closes the issues and pull requests contracts left open in the sandbox on
/// `github`, and deletes the pull requests' branches, of earlier runs: those
/// Linewright opened, titled by [`Sandbox::unique`] and not written to for
/// [`LONGEST_CONTRACT`]. GitHub deletes neither
/// issues nor pull requests, so closed ones are left for good, and the
/// contracts read snapshots over windows that leave them out.
pub async fn sweep(github: &GitHub) -> Result<()> {
    let linewright = github.linewright();
    let before = chrono::Utc::now() - LONGEST_CONTRACT;
    // The issues API lists pull requests among the issues
    // (<https://docs.github.com/en/rest/issues/issues#list-repository-issues>).
    let path = |page: u32| format!("/issues?state=open&per_page=100&page={page}");
    let mut left = vec![];
    for page in 1.. {
        let items = github.repo_get(&path(page)).await?;
        let items = items.as_array().context("GitHub lists the open issues")?;
        for item in items {
            if left_over(item, &linewright, before) {
                let number = item["number"].as_u64().context("an issue has a number")?;
                left.push((Number(number), item.get("pull_request").is_some()));
            }
        }
        if items.len() < 100 {
            break;
        }
    }
    for (number, pull) in left {
        if pull {
            let pr = github.repo_get(&format!("/pulls/{}", number.0)).await?;
            github.close_pull(number).await?;
            if let Some(branch) = pr["head"]["ref"].as_str() {
                github.delete_branch(branch).await?;
            }
        } else {
            github
                .close_issue(number, IssueCloseReason::NotPlanned)
                .await?;
        }
    }
    Ok(())
}

/// Whether the open issue or pull request `item`, as GitHub's REST API gives
/// it, is one a contract left that the sweep closes: opened by `linewright`,
/// titled by [`Sandbox::unique`] and not written to since `before`.
fn left_over(item: &Value, linewright: &Login, before: Timestamp) -> bool {
    let opener = item["user"]["login"].as_str().map(Login::new);
    let updated: Option<Timestamp> = item["updated_at"].as_str().and_then(|at| at.parse().ok());
    let made = item["title"].as_str().is_some_and(|t| t.starts_with(MARK));
    made && opener.as_ref() == Some(linewright) && updated.is_some_and(|at| at < before)
}
