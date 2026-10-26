//! The only writer of the local store's history: every commit it makes to its
//! repository and every ref it moves there.

use std::borrow::Cow;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::domain::{MergeMethod, Number};
use crate::git::{self, Merged};

/// A commit to write on `branch`, which starts from the default branch when
/// it doesn't exist yet.
pub struct NewCommit<'a> {
    pub branch: &'a str,
    pub default_branch: &'a str,
    pub author: &'a str,
    pub message: &'a str,
    pub files: &'a [(&'a str, &'a [u8])],
}

/// Writes `commit` and moves its branch to it, returning the commit.
pub async fn write_commit(repo: &Path, commit: NewCommit<'_>) -> Result<String> {
    let branch = branch_ref(commit.branch);
    let default_branch = branch_ref(commit.default_branch);
    // One process for both tips: `rev-parse --verify` takes one revision.
    // A pattern also matches the refs below it, so only exact names count.
    let tips = git::git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            &branch,
            &default_branch,
        ],
    )
    .await?;
    let tip = |refname: &str| {
        tips.lines()
            .find_map(|line| line.strip_prefix(refname)?.strip_prefix(' '))
    };
    let parent = tip(&branch).or_else(|| tip(&default_branch));
    // Without `--force`, fast-import refuses to move the branch other than
    // forward, as a push would.
    let id = git::git_input(
        repo,
        &["fast-import", "--quiet", "--date-format=now"],
        &[],
        &fast_import_stream(&commit, parent),
    )
    .await?;
    Ok(id.trim_end().to_string())
}

/// The `git fast-import` stream
/// (<https://git-scm.com/docs/git-fast-import>) that writes `commit` on top
/// of `parent` and prints it.
fn fast_import_stream(commit: &NewCommit<'_>, parent: Option<&str>) -> Vec<u8> {
    let mut stream = Vec::new();
    let author = commit.author;
    let identity = format!("{author} <{author}@example.com> now");
    stream.extend(format!("commit {}\nmark :1\n", branch_ref(commit.branch)).bytes());
    stream.extend(format!("author {identity}\ncommitter {identity}\n").bytes());
    add_data(&mut stream, commit.message.as_bytes());
    if let Some(parent) = parent {
        stream.extend(format!("from {parent}\n").bytes());
    }
    for (path, contents) in commit.files {
        stream.extend(format!("M 100644 inline {}\n", quote_path(path)).bytes());
        add_data(&mut stream, contents);
    }
    stream.extend(b"\nget-mark :1\n");
    stream
}

/// Adds a fast-import `data` command carrying `data` to `stream`.
fn add_data(stream: &mut Vec<u8>, data: &[u8]) {
    stream.extend(format!("data {}\n", data.len()).bytes());
    stream.extend(data);
    stream.push(b'\n');
}

/// `path` quoted where fast-import requires it
/// (<https://git-scm.com/docs/git-fast-import#_modify>), and otherwise as is.
fn quote_path(path: &str) -> String {
    if !path.starts_with('"') && !path.contains('\n') {
        return path.to_string();
    }
    let mut quoted = String::from('"');
    for c in path.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// A merge of pull request `pr`, from branch `head` into branch `into` at
/// `before`, by `by`.
pub struct MergeOf<'a> {
    pub into: &'a str,
    pub before: &'a str,
    pub head: &'a str,
    pub method: MergeMethod,
    pub by: &'a str,
    pub title: &'a str,
    pub pr: Number,
}

/// Merges as GitHub merges
/// (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github>),
/// in the object store as GitHub does, and returns the new tip of the branch
/// merged into.
pub async fn merge(repo: &Path, merge: MergeOf<'_>) -> Result<String> {
    let MergeOf {
        into,
        before,
        head,
        method,
        by,
        title,
        pr,
    } = merge;
    let by = Signature::of(by);
    let after = match method {
        MergeMethod::Merge => {
            let tree = merged_tree(repo, before, head).await?;
            let message = format!("Merge pull request {pr} from {head}");
            commit_tree(repo, &tree, &[before, head], &message, &by, &by).await?
        }
        MergeMethod::Squash => {
            let tree = merged_tree(repo, before, head).await?;
            let message = format!("{title} ({pr})");
            commit_tree(repo, &tree, &[before], &message, &by, &by).await?
        }
        MergeMethod::Rebase => {
            let Replayed::Onto(after) = replay(repo, before, head, by.name).await? else {
                bail!("the commits of {head} don't rebase onto {into} without conflicts");
            };
            after
        }
    };
    move_ref(repo, into, &after, before).await?;
    Ok(after)
}

/// The tree that merges `head` into the commit `tip`, which must merge
/// cleanly.
async fn merged_tree(repo: &Path, tip: &str, head: &str) -> Result<String> {
    match git::merge_tree(repo, None, tip, head).await? {
        Merged::Clean(tree) => Ok(tree),
        Merged::Conflicted(_) => bail!("{head} doesn't merge into {tip} without conflicts"),
    }
}

/// Who authored or committed a commit, and when, if not now
/// (<https://git-scm.com/docs/git-commit-tree#_commit_information>).
struct Signature<'a> {
    name: &'a str,
    email: Cow<'a, str>,
    date: Option<&'a str>,
}

impl<'a> Signature<'a> {
    /// The local store's signature for the login `name`, now.
    fn of(name: &'a str) -> Self {
        Signature {
            name,
            email: Cow::Owned(format!("{name}@example.com")),
            date: None,
        }
    }

    /// The environment that makes this `role`'s signature, `AUTHOR` or
    /// `COMMITTER`.
    fn env(&self, role: Role) -> Vec<(String, String)> {
        let role = match role {
            Role::Author => "AUTHOR",
            Role::Committer => "COMMITTER",
        };
        let mut env = vec![
            (format!("GIT_{role}_NAME"), self.name.to_string()),
            (format!("GIT_{role}_EMAIL"), self.email.to_string()),
        ];
        env.extend(
            self.date
                .map(|date| (format!("GIT_{role}_DATE"), date.to_string())),
        );
        env
    }
}

/// Which of a commit's two signatures a [`Signature`] is.
#[derive(Clone, Copy)]
enum Role {
    Author,
    Committer,
}

/// Writes a commit of `tree` with `parents`, returning it.
async fn commit_tree(
    repo: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
    author: &Signature<'_>,
    committer: &Signature<'_>,
) -> Result<String> {
    let mut args = vec!["commit-tree", tree];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    args.extend(["-F", "-"]);
    let env = [author.env(Role::Author), committer.env(Role::Committer)].concat();
    Ok(git::git_input(repo, &args, &env, message.as_bytes())
        .await?
        .trim()
        .to_string())
}

/// Where replaying a branch's commits ended.
pub enum Replayed {
    /// On this commit, the last one replayed.
    Onto(String),
    Conflict,
}

/// A commit to replay, as `git log` gives it.
struct Pick<'a> {
    commit: &'a str,
    tree: &'a str,
    parent: &'a str,
    author: Signature<'a>,
    message: &'a str,
}

/// The fields of a [`Pick`], in `git log`'s format
/// (<https://git-scm.com/docs/git-log#_pretty_formats>).
const PICK_FORMAT: &str = "--format=%H%x00%T%x00%P%x00%an%x00%ae%x00%ad%x00%B";
const PICK_FIELDS: usize = 7;

/// The commits in `log`, as `git log -z` gives them in [`PICK_FORMAT`],
/// to replay onto `onto`.
fn picks<'a>(log: &'a str, onto: &str) -> Result<Vec<Pick<'a>>> {
    let fields: Vec<&str> = log.split_terminator('\0').collect();
    fields
        .chunks(PICK_FIELDS)
        .map(|pick| {
            let &[commit, tree, parent, name, email, date, message] = pick else {
                bail!("git log gave an incomplete commit: {pick:?}");
            };
            if parent.is_empty() {
                bail!("{commit} is a root commit, which a rebase onto {onto} can't replay");
            }
            Ok(Pick {
                commit,
                tree,
                parent,
                author: Signature {
                    name,
                    email: Cow::Borrowed(email),
                    date: Some(date),
                },
                message,
            })
        })
        .collect()
}

/// Whether `pick`, which turned its parent's tree into its own, became
/// empty when replayed onto a tip with `tip_tree` as `tree`: a commit that
/// was empty to begin with is kept, as `git rebase` keeps it
/// (<https://git-scm.com/docs/git-rebase#Documentation/git-rebase.txt---emptydropkeepstop>).
fn becomes_empty(pick: &Pick, parent_tree: &str, tip_tree: &str, tree: &str) -> bool {
    tree == tip_tree && pick.tree != parent_tree
}

/// Replays the commits of `head` onto `onto`, as `git rebase`'s merge
/// backend replays them (<https://git-scm.com/docs/git-rebase#_behavioral_differences>),
/// with `committer` as their committer: merge commits are dropped, and so are
/// commits already upstream and commits that become empty. The commits it
/// writes are in `repo`, and it moves no ref there.
pub async fn replay(repo: &Path, onto: &str, head: &str, committer: &str) -> Result<Replayed> {
    let log = git::git(
        repo,
        &[
            "log",
            "-z",
            "--reverse",
            "--no-merges",
            "--cherry-pick",
            "--right-only",
            "--date=raw",
            PICK_FORMAT,
            &format!("{onto}...{head}"),
        ],
    )
    .await?;
    let picks = picks(&log, onto)?;
    // One rev-parse gives the tip's commit and tree and every parent's tree.
    let mut revs = vec![
        "rev-parse".to_string(),
        format!("{onto}^{{commit}}"),
        format!("{onto}^{{tree}}"),
    ];
    revs.extend(picks.iter().map(|pick| format!("{}^{{tree}}", pick.parent)));
    let revs: Vec<&str> = revs.iter().map(String::as_str).collect();
    let resolved = git::git(repo, &revs).await?;
    let mut resolved = resolved.lines();
    let (Some(tip), Some(tip_tree)) = (resolved.next(), resolved.next()) else {
        bail!("git rev-parse didn't resolve {onto}");
    };
    let (mut tip, mut tip_tree) = (tip.to_string(), tip_tree.to_string());
    let committer = Signature::of(committer);
    for (pick, parent_tree) in picks.iter().zip(resolved) {
        let tree = match git::merge_tree(repo, Some(pick.parent), &tip, pick.commit).await? {
            Merged::Clean(tree) => tree,
            Merged::Conflicted(_) => return Ok(Replayed::Conflict),
        };
        if becomes_empty(pick, parent_tree, &tip_tree, &tree) {
            continue;
        }
        tip = commit_tree(repo, &tree, &[&tip], pick.message, &pick.author, &committer).await?;
        tip_tree = tree;
    }
    Ok(Replayed::Onto(tip))
}

/// Moves `branch` to `new`, provided it is at `old`.
pub async fn move_ref(repo: &Path, branch: &str, new: &str, old: &str) -> Result<()> {
    git::git(repo, &["update-ref", &branch_ref(branch), new, old]).await?;
    Ok(())
}

/// Deletes `branch`, which succeeds when there is no such branch.
pub async fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    git::git(repo, &["update-ref", "-d", &branch_ref(branch)]).await?;
    Ok(())
}

/// Fetches all of `bundle`'s refs under `refs/case/` and moves
/// `default_branch` to `base`, wherever it was.
pub async fn import_bundle(
    repo: &Path,
    bundle: &Path,
    default_branch: &str,
    base: &str,
) -> Result<()> {
    let bundle = bundle.to_str().context("bundle paths are UTF-8")?;
    git::git(repo, &["fetch", "--quiet", bundle, "refs/*:refs/case/*"]).await?;
    git::git(repo, &["update-ref", &branch_ref(default_branch), base]).await?;
    Ok(())
}

/// The ref of `branch`.
fn branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}
