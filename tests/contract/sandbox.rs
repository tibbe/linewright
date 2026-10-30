use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use linewright::clock::SystemClock;
use linewright::domain::label::Label;
use linewright::domain::{
    IssueCloseReason, Login, MergeMethod, MergeMethods, Number, PullState, Repo, Timestamp, Windows,
};
use linewright::git;
use linewright::store::github::GitHub;
use linewright::store::local::{LocalStore, Seed};
use linewright::store::{NewPull, PushMode, Store};
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

/// Held by a contract while it merges into the default branch: GitHub
/// refuses a merge when the default branch moved since it worked out
/// whether the pull request merges, which merges at the same time would
/// keep doing.
static MERGING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The repository the local store stands in for.
const LOCAL_REPO: &str = "contract/sandbox";

/// Who Linewright acts as on the local store: not GitHub's account, so that
/// no contract can pass by assuming it.
const LOCAL_LINEWRIGHT: &str = "contract-linewright";

/// Who made the local store's first commit, as the sandbox's owner made its
/// first.
const LOCAL_OWNER: &str = "contract-owner";

/// What a contract runs against: a store and what it shares with the other
/// contracts on it.
pub struct Sandbox {
    store: Arc<dyn Store>,
    repo: Repo,
    /// How long the store may take to answer a read with what a write just
    /// wrote.
    lag: Duration,
    run: String,
    /// Kept by the [`Made`] too, as it holds the local store.
    scratch: Arc<TempDir>,
    clones: AtomicUsize,
    made: Arc<Mutex<Vec<Thing>>>,
}

/// A branch a contract pushed, and the commit it pushed it to.
pub struct Pushed {
    pub name: String,
    pub head: String,
}

/// A pull request a contract opened, and the branch it opened it from.
pub struct Opened {
    pub branch: Pushed,
    pub pr: Number,
}

/// A commit for [`Sandbox::push_branch`] or [`commit`] to make.
pub struct NewCommit<'a> {
    pub message: &'a str,
    /// Each file it writes, with what it holds.
    pub files: &'a [(&'a str, &'a [u8])],
}

/// The start of a PNG
/// (<https://www.w3.org/TR/png-3/#5PNG-file-signature>), which holds NUL
/// bytes.
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// What a contract made that the harness closes or deletes once it ends.
#[derive(Debug)]
enum Thing {
    Branch(String),
    Issue(Number),
    Pull(Number),
}

/// What a contract made, for the harness to close or delete once it ends,
/// whether it passed or not, so that the next run's sweep has little to do.
pub struct Made {
    store: Arc<dyn Store>,
    things: Arc<Mutex<Vec<Thing>>>,
    _scratch: Arc<TempDir>,
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
        store
            .commit(
                LOCAL_OWNER,
                "main",
                &[("README.md", "# The sandbox\n")],
                "Initial commit",
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
            scratch: Arc::new(scratch),
            clones: AtomicUsize::new(0),
            made: Arc::default(),
        }
    }

    /// What the contract makes, for the harness to clean up after it.
    pub fn made(&self) -> Made {
        Made {
            store: self.store.clone(),
            things: self.made.clone(),
            _scratch: self.scratch.clone(),
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

    /// A fresh clone of `branch`, committing as Linewright.
    pub async fn clone(&self, branch: &str) -> Result<PathBuf> {
        let identity = self.store().identity().await?;
        let clone = self.clone_dir();
        self.store()
            .clone_repo(
                &clone,
                branch,
                &[
                    ("user.name", &identity.name),
                    ("user.email", &identity.email),
                ],
            )
            .await?;
        Ok(clone)
    }

    /// A directory for a clone, of the contract's own.
    pub fn clone_dir(&self) -> PathBuf {
        let n = self.clones.fetch_add(1, Ordering::Relaxed);
        self.scratch().join(format!("clone-{n}"))
    }

    /// A directory of the contract's own, such as for a file to attach.
    pub fn scratch(&self) -> &Path {
        self.scratch.path()
    }

    /// The repository's default branch.
    pub async fn default_branch(&self) -> Result<String> {
        Ok(self.store().settings().await?.default_branch)
    }

    /// The method Linewright merges with, one the repository allows.
    pub async fn merge_method(&self) -> Result<MergeMethod> {
        Ok(self.store().settings().await?.merge_method)
    }

    /// A fresh clone of the default branch, committing as Linewright.
    pub async fn clone_default_branch(&self) -> Result<PathBuf> {
        self.clone(&self.default_branch().await?).await
    }

    /// Pushes a branch named for `what`, from the default branch with
    /// `commits` on top, and returns it.
    pub async fn push_branch(&self, what: &str, commits: &[NewCommit<'_>]) -> Result<Pushed> {
        let clone = self.clone_default_branch().await?;
        let name = self.unique(what);
        let head = commit(&clone, commits).await?;
        self.made_branch(&name);
        self.push(&clone, &name, PushMode::Replace).await?;
        Ok(Pushed { name, head })
    }

    /// Pushes `clone` to `branch` as [`Store::push`], asked again on GitHub,
    /// which refuses a push now and then while the repository is busy, such
    /// as with the other contracts' pushes and merges.
    pub async fn push(&self, clone: &Path, branch: &str, mode: PushMode) -> Result<()> {
        self.eventually(async || Ok(self.store().push(clone, branch, mode.clone()).await?))
            .await
    }

    /// Waits until no other contract merges into the default branch, and
    /// keeps them from it until the guard drops.
    pub async fn merging(&self) -> tokio::sync::MutexGuard<'static, ()> {
        MERGING.lock().await
    }

    /// Opens a pull request from `branch`, titled for `what`, closing nothing.
    pub async fn open_pull(&self, branch: Pushed, what: &str) -> Result<Opened> {
        self.open_pull_with(branch, what, "Opened by a contract.")
            .await
    }

    /// As [`Sandbox::open_pull`], with `body`.
    pub async fn open_pull_with(&self, branch: Pushed, what: &str, body: &str) -> Result<Opened> {
        let pr = self
            .store()
            .open_pull(NewPull {
                head: branch.name.clone(),
                title: self.unique(what),
                body: body.into(),
            })
            .await?;
        self.made_pull(pr);
        Ok(Opened { branch, pr })
    }

    /// Has the harness close the issue #`issue` once the contract ends.
    pub fn made_issue(&self, issue: Number) {
        self.made
            .lock()
            .expect("not poisoned")
            .push(Thing::Issue(issue));
    }

    /// Has the harness close #`pr`, if it is open, once the contract ends.
    pub fn made_pull(&self, pr: Number) {
        self.made
            .lock()
            .expect("not poisoned")
            .push(Thing::Pull(pr));
    }

    /// Has the harness delete `branch` once the contract ends.
    pub fn made_branch(&self, branch: &str) {
        let mut made = self.made.lock().expect("not poisoned");
        made.push(Thing::Branch(branch.to_string()));
    }
}

/// Makes `commits` in `clone`, each writing its files, and returns the
/// last.
pub async fn commit(clone: &Path, commits: &[NewCommit<'_>]) -> Result<String> {
    for NewCommit { message, files } in commits {
        for (path, contents) in *files {
            let file = clone.join(path);
            if let Some(dir) = file.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            tokio::fs::write(&file, contents).await?;
        }
        git::git(clone, &["add", "--all"]).await?;
        git::git(clone, &["commit", "--quiet", "--message", message]).await?;
    }
    git::head(clone).await
}

impl Made {
    /// Closes the issues and pull requests the contract opened and left
    /// open, then deletes its branches.
    pub async fn clean_up(self) -> Result<()> {
        let mut things = std::mem::take(&mut *self.things.lock().expect("not poisoned"));
        things.sort_by_key(|thing| matches!(thing, Thing::Branch(_)));
        for thing in things {
            match thing {
                Thing::Pull(pr) => {
                    let pull = self.store.pull(pr).await?;
                    if matches!(pull.state, PullState::Open { .. }) {
                        self.store.close_pull(pr).await?;
                    }
                }
                Thing::Issue(issue) => {
                    self.store
                        .close_issue(issue, IssueCloseReason::NotPlanned)
                        .await?;
                }
                Thing::Branch(branch) => self.store.delete_branch(&branch).await?,
            }
        }
        Ok(())
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
/// [`LONGEST_CONTRACT`]. Then deletes the branches named by
/// [`Sandbox::unique`] not committed to for as long, which a contract
/// stopped before its cleanup leaves without a pull request. GitHub deletes
/// neither issues nor pull requests, so closed ones are left for good, and
/// the contracts read snapshots over windows that leave them out.
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
    // <https://docs.github.com/en/rest/git/refs#list-matching-references>
    let refs = github
        .repo_get(&format!("/git/matching-refs/heads/{MARK}"))
        .await?;
    let refs = refs.as_array().context("GitHub lists the matching refs")?;
    for branch in refs {
        let name = branch["ref"].as_str().context("a ref has a name")?;
        let sha = branch["object"]["sha"]
            .as_str()
            .context("a ref has a commit")?;
        let commit = github.repo_get(&format!("/commits/{sha}")).await?;
        let committed: Option<Timestamp> = commit["commit"]["committer"]["date"]
            .as_str()
            .and_then(|at| at.parse().ok());
        if committed.is_some_and(|at| at < before) {
            let name = name.strip_prefix("refs/heads/").unwrap_or(name);
            github.delete_branch(name).await?;
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
