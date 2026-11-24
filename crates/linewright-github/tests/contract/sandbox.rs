use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use futures_util::future::BoxFuture;
use linewright_core::clock::SystemClock;
use linewright_core::domain::label::Label;
use linewright_core::domain::{
    IssueCloseReason, Login, MergeMethod, MergeMethods, Number, PullState, Repo, ReviewState,
    Timestamp, Windows,
};
use linewright_core::git;
use linewright_core::store::local::{LocalStore, Seed};
use linewright_core::store::{NewIssue, NewPull, PushMode, Store};
use linewright_github::GitHub;
use reqwest::Method;
use serde_json::{Value, json};
use tempfile::TempDir;

/// One promise of the `Store` trait, checked through the trait in a
/// [`Sandbox`].
pub type Contract = fn(Sandbox) -> BoxFuture<'static, Result<()>>;

/// How long GitHub may take to answer a read with what a write just wrote,
/// as its search and timeline indexes do.
const GITHUB_LAG: Duration = Duration::from_secs(20);

/// How long a contract on GitHub may run, at the longest.
const LONGEST_CONTRACT: chrono::TimeDelta = chrono::TimeDelta::minutes(10);

/// How far the contracts allow GitHub's clock to be from the runner's,
/// either way.
const GITHUB_SKEW: chrono::TimeDelta = chrono::TimeDelta::seconds(5);

/// How finely GitHub stamps a write: its timestamps are in ISO 8601
/// (<https://docs.github.com/en/rest/using-the-rest-api/timezones-and-the-rest-api>),
/// to the whole second.
const GITHUB_RESOLUTION: Duration = Duration::from_secs(1);

/// What starts the name of everything a contract makes, after the number of
/// a branch named for a ticket.
const MARK: &str = "contract-";

/// Held by a contract while it merges into the default branch. GitHub works
/// out again whether a pull request merges once the default branch moves
/// (<https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request>).
static MERGING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The repository the local store stands in for.
const LOCAL_REPO: &str = "contract/sandbox";

/// Who Linewright acts as on the local store.
const LOCAL_LINEWRIGHT: &str = "contract-linewright";

/// A token the local store lets read it.
const LOCAL_READER: &str = "contract-reader";

/// What a review that starts a thread says, beside its one comment.
const THREAD_REVIEW: &str = "Started by a contract.";

/// Who made the local store's first commit, as the sandbox's owner made its
/// first.
const LOCAL_OWNER: &str = "contract-owner";

/// What a contract runs against: a store and what it shares with the other
/// contracts on it.
pub struct Sandbox {
    side: Side,
    repo: Repo,
    /// How long the store may take to answer a read with what a write just
    /// wrote.
    lag: Duration,
    run: String,
    /// The contract's own directory, which holds the local store.
    scratch: Arc<TempDir>,
    clones: AtomicUsize,
    made: Arc<Mutex<Vec<Thing>>>,
}

/// Which store a contract runs against: the local store or the GitHub store,
/// and no other.
#[derive(Clone)]
enum Side {
    /// The local store, as its own type.
    Local(Arc<LocalStore>),
    /// The GitHub store, with the token it was reached with.
    GitHub { github: GitHub, token: String },
}

impl Side {
    fn store(&self) -> &dyn Store {
        match self {
            Side::Local(local) => &**local,
            Side::GitHub { github, .. } => github,
        }
    }
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

/// A commit to make: its message and its files.
pub struct NewCommit<'a> {
    pub message: &'a str,
    /// Each file it writes, with what it holds.
    pub files: &'a [(&'a str, &'a [u8])],
}

/// The start of a PNG
/// (<https://www.w3.org/TR/png-3/#5PNG-file-signature>), which holds NUL
/// bytes.
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// Something a contract made, to close or delete once it ends.
#[derive(Debug)]
enum Thing {
    Branch(String),
    Issue(Number),
    Pull(Number),
}

/// What a contract made, to close or delete once it ends, whether it passed
/// or not.
pub struct Made {
    side: Side,
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
                requires_up_to_date: false,
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
        Ok(Sandbox::new(
            Side::Local(Arc::new(store)),
            repo,
            Duration::ZERO,
            scratch,
        ))
    }

    /// The GitHub store on the sandbox repository `repo`, reached with
    /// `token`.
    pub fn github(github: GitHub, token: String, repo: Repo) -> Result<Sandbox> {
        Ok(Sandbox::new(
            Side::GitHub { github, token },
            repo,
            GITHUB_LAG,
            tempfile::tempdir()?,
        ))
    }

    fn new(side: Side, repo: Repo, lag: Duration, scratch: TempDir) -> Sandbox {
        let run = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        Sandbox {
            side,
            repo,
            lag,
            run,
            scratch: Arc::new(scratch),
            clones: AtomicUsize::new(0),
            made: Arc::default(),
        }
    }

    /// What the contract makes, to clean up after it.
    pub fn made(&self) -> Made {
        Made {
            side: self.side.clone(),
            things: self.made.clone(),
            _scratch: self.scratch.clone(),
        }
    }

    pub fn store(&self) -> &dyn Store {
        self.side.store()
    }

    /// The repository the store is of.
    /// Who a review can be requested of: the repository's owner, a person
    /// other than Linewright.
    pub fn reviewer(&self) -> Login {
        Login::new(self.repo.owner())
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// `name`, made unique to this contract's run, for the title of an
    /// issue or pull request or the name of a branch or label.
    pub fn unique(&self, name: &str) -> String {
        format!("{MARK}{name}-{}", self.run)
    }

    /// The first answer `check` gives without failing, asked again on GitHub
    /// until [`GITHUB_LAG`] has passed and once on the local store.
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

    /// Waits until the next write is stamped later than the last: a second
    /// on GitHub, which stamps in whole seconds and asks for a second between
    /// writes
    /// (<https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#pause-between-mutative-requests>),
    /// and not at all on the local store.
    pub async fn tick(&self) {
        match &self.side {
            Side::Local(_) => {}
            Side::GitHub { .. } => tokio::time::sleep(GITHUB_RESOLUTION).await,
        }
    }

    /// Fails unless `what`, stamped `at`, was stamped between `before` and
    /// `after`, as the runner's clock read them: exactly on the local store,
    /// and on GitHub give or take a second of its resolution and
    /// [`GITHUB_SKEW`].
    pub fn stamped_between(
        &self,
        what: &str,
        at: Timestamp,
        before: Timestamp,
        after: Timestamp,
    ) -> Result<()> {
        let slack = match &self.side {
            Side::Local(_) => chrono::TimeDelta::zero(),
            Side::GitHub { .. } => {
                chrono::TimeDelta::from_std(GITHUB_RESOLUTION).expect("a second is in range")
                    + GITHUB_SKEW
            }
        };
        let (from, to) = (before - slack, after + slack);
        ensure!(
            from <= at && at <= to,
            "{what} is stamped {at}, not between {from} and {to}"
        );
        Ok(())
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
        self.push_named(self.unique(what), commits).await
    }

    /// Pushes the branch `name`, from the default branch with `commits` on
    /// top, and returns it.
    pub async fn push_named(&self, name: String, commits: &[NewCommit<'_>]) -> Result<Pushed> {
        let clone = self.clone_default_branch().await?;
        let head = commit(&clone, commits).await?;
        self.made_branch(&name);
        self.push(&clone, &name, PushMode::Replace).await?;
        Ok(Pushed { name, head })
    }

    /// Pushes `clone` to `branch` as [`Store::push`], asked again on GitHub
    /// while it refuses.
    pub async fn push(&self, clone: &Path, branch: &str, mode: PushMode) -> Result<()> {
        let head = git::head(clone).await?;
        self.eventually(async || {
            Ok(self
                .store()
                .push(clone, &head, branch, mode.clone())
                .await?)
        })
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

    /// Opens an issue titled for `what`, and a pull request closing it from a
    /// branch named for `what` that adds a file of its own, and returns the
    /// issue.
    pub async fn open_pull_closing_an_issue(&self, what: &str) -> Result<(Number, Opened)> {
        let issue = self
            .store()
            .create_issue(NewIssue {
                title: self.unique(&format!("{what}-issue")),
                body: "Opened for a pull request to close.".into(),
                labels: vec![],
            })
            .await?;
        self.made_issue(issue);
        let path = format!("{}.txt", self.unique(what));
        let branch = self
            .push_branch(
                what,
                &[NewCommit {
                    message: "Add a file",
                    files: &[(&path, b"added\n")],
                }],
            )
            .await?;
        let opened = self
            .open_pull_with(branch, what, &format!("Closes {issue}."))
            .await?;
        Ok((issue, opened))
    }

    /// Waits until #`pr` lists `issue` as the one it closes.
    pub async fn linked(&self, pr: Number, issue: Number) -> Result<()> {
        self.eventually(async || {
            let pull = self.store().pull(pr).await?;
            crate::check::equal("the issues it closes", &pull.closes, &vec![issue])
        })
        .await
    }

    /// Starts a thread on the first line of `path` in #`pr`, which changes
    /// it, as a review of Linewright's commenting `body` there, and returns
    /// the thread's id as #`pr`'s threads list it.
    pub async fn start_thread(&self, pr: Number, path: &str, body: &str) -> Result<String> {
        match &self.side {
            Side::Local(local) => local.review_as(
                LOCAL_LINEWRIGHT,
                pr,
                ReviewState::Commented,
                THREAD_REVIEW,
                &[(path, body)],
            )?,
            // GitHub refuses a line comment on a line the pull request's
            // diff lacks, as it may until it has worked out the diff of a
            // pull request just opened, and asks a `COMMENT` review for a
            // body
            // (<https://docs.github.com/en/rest/pulls/reviews#create-a-review-for-a-pull-request>).
            Side::GitHub { github, .. } => {
                self.eventually(async || {
                    github
                        .repo_request(
                            Method::POST,
                            &format!("/pulls/{}/reviews", pr.0),
                            Some(json!({
                                "event": "COMMENT",
                                "body": THREAD_REVIEW,
                                "comments": [{"path": path, "line": 1, "side": "RIGHT", "body": body}],
                            })),
                        )
                        .await
                })
                .await?;
            }
        }
        self.eventually(async || {
            let threads = self.store().pull(pr).await?.threads;
            threads
                .into_iter()
                .find(|thread| {
                    thread.path == path
                        && thread
                            .comments
                            .first()
                            .is_some_and(|first| first.body == body)
                })
                .map(|thread| thread.id)
                .with_context(|| format!("{pr} lists no thread on {path} saying {body:?}"))
        })
        .await
    }

    /// A token that can read the repository: the sandbox's own on GitHub,
    /// and any on the local store.
    pub fn reading_token(&self) -> String {
        match &self.side {
            Side::Local(_) => LOCAL_READER.into(),
            Side::GitHub { token, .. } => token.clone(),
        }
    }

    /// A token that cannot read the repository: on GitHub, one shaped as a
    /// personal access token that GitHub doesn't know, which it answers
    /// with 401; on the local store, one it is told cannot read.
    pub fn unreadable_token(&self) -> String {
        // A personal access token is `ghp_` and 36 characters
        // (<https://github.blog/engineering/platform-security/behind-githubs-new-authentication-token-formats/>):
        // here 32 of a fresh UUID's, padded with zeros.
        let token = format!("ghp_0000{}", uuid::Uuid::new_v4().simple());
        if let Side::Local(local) = &self.side {
            local.refuse_reads_by(&token);
        }
        token
    }

    /// Closes the issue #`issue` once the contract ends.
    pub fn made_issue(&self, issue: Number) {
        self.made
            .lock()
            .expect("not poisoned")
            .push(Thing::Issue(issue));
    }

    /// Closes #`pr`, if it is open, once the contract ends.
    pub fn made_pull(&self, pr: Number) {
        self.made
            .lock()
            .expect("not poisoned")
            .push(Thing::Pull(pr));
    }

    /// Deletes `branch` once the contract ends.
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
    Ok(git::head(clone).await?.into())
}

impl Made {
    /// Closes the issues and pull requests the contract opened and left
    /// open, then deletes its branches.
    pub async fn clean_up(self) -> Result<()> {
        let store = self.side.store();
        let mut things = std::mem::take(&mut *self.things.lock().expect("not poisoned"));
        things.sort_by_key(|thing| matches!(thing, Thing::Branch(_)));
        for thing in things {
            match thing {
                Thing::Pull(pr) => {
                    let pull = store.pull(pr).await?;
                    if matches!(pull.state, PullState::Open { .. }) {
                        store.close_pull(pr).await?;
                    }
                }
                Thing::Issue(issue) => {
                    store
                        .close_issue(issue, IssueCloseReason::NotPlanned)
                        .await?;
                }
                Thing::Branch(branch) => store.delete_branch(&branch).await?,
            }
        }
        Ok(())
    }
}

/// The label `name`, meaning `description`.
pub fn label(name: &str, description: &'static str) -> Label {
    Label {
        name: name.to_string().leak(),
        description,
    }
}

/// Windows that leave out what earlier runs closed.
pub fn recent() -> Windows {
    let hour = chrono::TimeDelta::hours(1);
    Windows::at(chrono::Utc::now(), hour, hour)
}

/// Closes the issues and pull requests contracts left open in the sandbox on
/// `github`, and deletes the pull requests' branches, of earlier runs: those
/// Linewright opened, titled by [`Sandbox::unique`] and not written to for
/// [`LONGEST_CONTRACT`]. Then deletes the branches named by
/// [`Sandbox::unique`] not committed to for as long.
pub async fn sweep(github: &GitHub) -> Result<()> {
    let linewright = github.linewright();
    let before = chrono::Utc::now() - LONGEST_CONTRACT;
    // The issues API lists pull requests among the issues
    // (<https://docs.github.com/en/rest/issues/issues#list-repository-issues>).
    let path = |page: u32| format!("/issues?state=open&per_page=100&page={page}");
    let mut left = vec![];
    for page in 1.. {
        let items = github.repo_request(Method::GET, &path(page), None).await?;
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
            let pr = github
                .repo_request(Method::GET, &format!("/pulls/{}", number.0), None)
                .await?;
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
        .repo_request(Method::GET, "/git/matching-refs/heads/", None)
        .await?;
    let refs = refs.as_array().context("GitHub lists the matching refs")?;
    for branch in refs {
        let name = branch["ref"].as_str().context("a ref has a name")?;
        let name = name.strip_prefix("refs/heads/").unwrap_or(name);
        let unnumbered = name
            .split_once('-')
            .filter(|(number, _)| number.bytes().all(|b| b.is_ascii_digit()))
            .map_or(name, |(_, rest)| rest);
        if !unnumbered.starts_with(MARK) {
            continue;
        }
        let sha = branch["object"]["sha"]
            .as_str()
            .context("a ref has a commit")?;
        let commit = github
            .repo_request(Method::GET, &format!("/commits/{sha}"), None)
            .await?;
        let committed: Option<Timestamp> = commit["commit"]["committer"]["date"]
            .as_str()
            .and_then(|at| at.parse().ok());
        if committed.is_some_and(|at| at < before) {
            github.delete_branch(name).await?;
        }
    }
    Ok(())
}

/// Whether the open issue or pull request `item`, as GitHub's REST API gives
/// it, is one a contract left: opened by `linewright`, titled by
/// [`Sandbox::unique`] and not written to since `before`.
fn left_over(item: &Value, linewright: &Login, before: Timestamp) -> bool {
    let opener = item["user"]["login"].as_str().map(Login::new);
    let updated: Option<Timestamp> = item["updated_at"].as_str().and_then(|at| at.parse().ok());
    let made = item["title"].as_str().is_some_and(|t| t.starts_with(MARK));
    made && opener.as_ref() == Some(linewright) && updated.is_some_and(|at| at < before)
}
