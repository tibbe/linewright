use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncWriteExt;

use crate::child;
use crate::domain::{UpdateMethod, UpdateRule};

pub type GitEnv = [(String, String)];

/// The git binary that the `git` on the path runs, found on first use
/// (`docs/specs/operation.md`, Running, "git"). A `git` on the path may be a
/// stub or shim that finds another git and runs it, as macOS's
/// `/usr/bin/git` does with xcrun on every run, which costs more than many
/// a git command itself. git's install puts a `git` among its helpers, in
/// the directory `git --exec-path` names
/// (<https://github.com/git/git/blob/master/Makefile>, `install`), and that
/// `git` finds the rest of its install from there. Where it is missing, the
/// `git` on the path is run.
fn program() -> &'static Path {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        std::process::Command::new("git")
            .arg("--exec-path")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()).join("git"))
            .filter(|git| git.is_file())
            .unwrap_or_else(|| PathBuf::from("git"))
    })
}

pub async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_with(dir, args, &[]).await
}

pub async fn git_with(dir: &Path, args: &[&str], env: &GitEnv) -> Result<String> {
    git_input(dir, args, env, &[]).await
}

/// git's output as it wrote it, which need not be text.
pub async fn git_bytes(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    git_output(dir, args, &[], &[]).await
}

/// The contents of `object`, such as `main:README.md`, and its type, or none
/// when there is no such object, read in one process: `cat-file --batch`
/// says `missing` rather than failing
/// (<https://git-scm.com/docs/git-cat-file#_batch_output>). `-Z` ends the
/// name and the header with NUL, so a path may hold a newline.
pub async fn object(dir: &Path, object: &str) -> Result<Option<(String, Vec<u8>)>> {
    let input = format!("{object}\0");
    let output = git_output(dir, &["cat-file", "--batch", "-Z"], &[], input.as_bytes()).await?;
    let header_end = output
        .iter()
        .position(|&b| b == 0)
        .context("git cat-file --batch wrote no header")?;
    let header = String::from_utf8_lossy(&output[..header_end]);
    // `<object> <type> <size>`, or `<object> missing`.
    let mut fields = header.rsplitn(3, ' ');
    let (Some(size), Some(kind)) = (fields.next(), fields.next()) else {
        bail!("git cat-file --batch wrote {header}");
    };
    if size == "missing" || size == "ambiguous" {
        return Ok(None);
    }
    let size: usize = size
        .parse()
        .with_context(|| format!("the size in {header}"))?;
    let contents = output
        .get(header_end + 1..header_end + 1 + size)
        .context("git cat-file --batch wrote less than the object's size")?;
    Ok(Some((kind.to_string(), contents.to_vec())))
}

/// Runs git with `input` piped to its standard input.
pub async fn git_input(dir: &Path, args: &[&str], env: &GitEnv, input: &[u8]) -> Result<String> {
    let output = git_output(dir, args, env, input).await?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

async fn git_output(dir: &Path, args: &[&str], env: &GitEnv, input: &[u8]) -> Result<Vec<u8>> {
    let mut child = child::command(program())
        .args(args)
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    let mut stdin = child.stdin.take().context("git's stdin is piped")?;
    let (written, output) = tokio::join!(
        async move {
            let written = stdin.write_all(input).await;
            drop(stdin);
            written
        },
        child.wait_with_output()
    );
    let output = output.with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    written.with_context(|| format!("writing to git {}", args.join(" ")))?;
    Ok(output.stdout)
}

/// Clones `branch` of `url` into `dir`, with each `config` setting, such as
/// `user.name`, written to the clone's config.
pub async fn clone(
    url: &str,
    dir: &Path,
    branch: &str,
    config: &[(&str, &str)],
    env: &GitEnv,
) -> Result<()> {
    let parent = dir.parent().context("a clone directory has a parent")?;
    tokio::fs::create_dir_all(parent).await?;
    let target = dir.to_str().context("clone paths are UTF-8")?;
    let settings: Vec<String> = config
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let mut args = vec!["clone", "--quiet", "--no-tags", "--branch", branch];
    for setting in &settings {
        args.extend(["--config", setting]);
    }
    args.extend([url, target]);
    git_with(parent, &args, env).await?;
    Ok(())
}

pub async fn head(dir: &Path) -> Result<String> {
    Ok(git(dir, &["rev-parse", "HEAD"]).await?.trim().to_string())
}

async fn create_branch(dir: &Path, branch: &str) -> Result<()> {
    git(dir, &["checkout", "--quiet", "-B", branch]).await?;
    Ok(())
}

/// What a branch [`check_out_branch`] checked out starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchStart {
    /// The branch as the clone's remote has it, with the `unfinished`
    /// commits it holds.
    Existing {
        unfinished: Option<UnfinishedCommits>,
    },
    /// A new branch at the commit checked out before.
    New,
}

/// Checks out `branch` as the remote had it when cloned, or as a new branch
/// at `HEAD` when the remote had no such branch. `base` is what the
/// branch's unfinished commits are counted past.
pub async fn check_out_branch(dir: &Path, branch: &str, base: &str) -> Result<BranchStart> {
    let remote = format!("refs/remotes/origin/{branch}");
    if git(dir, &["rev-parse", "--verify", "--quiet", &remote])
        .await
        .is_ok()
    {
        git(dir, &["checkout", "--quiet", "-B", branch, &remote]).await?;
        let unfinished = unfinished_commits(dir, base, "HEAD").await?;
        Ok(BranchStart::Existing { unfinished })
    } else {
        create_branch(dir, branch).await?;
        Ok(BranchStart::New)
    }
}

pub enum Updated {
    UpToDate,
    Updated,
    Conflict(Conflict),
}

/// A rebase or merge of the default branch that stopped on conflicts in
/// `files` and is left in progress.
#[derive(Debug, PartialEq, Eq)]
pub struct Conflict {
    pub method: UpdateMethod,
    pub files: Vec<String>,
}

/// Brings the checked-out branch up to date with `origin/<default_branch>`,
/// as fresh as the clone, with the method `rule` picks.
pub async fn update(dir: &Path, default_branch: &str, rule: UpdateRule) -> Result<Updated> {
    let upstream = format!("origin/{default_branch}");
    if git(dir, &["merge-base", "--is-ancestor", &upstream, "HEAD"])
        .await
        .is_ok()
    {
        return Ok(Updated::UpToDate);
    }
    match rule {
        UpdateRule::Always(method) => update_with(dir, &upstream, method).await,
        UpdateRule::RebaseOnConflict => {
            match update_with(dir, &upstream, UpdateMethod::Merge).await? {
                Updated::Conflict(_) => {
                    git(dir, &["merge", "--abort"]).await?;
                    update_with(dir, &upstream, UpdateMethod::Rebase).await
                }
                updated @ (Updated::UpToDate | Updated::Updated) => Ok(updated),
            }
        }
    }
}

/// Aborts the update that stopped on `conflict`, leaving the branch as it
/// was before it.
pub async fn abort_update(dir: &Path, conflict: &Conflict) -> Result<()> {
    let operation = match conflict.method {
        UpdateMethod::Rebase => "rebase",
        UpdateMethod::Merge => "merge",
    };
    git(dir, &[operation, "--abort"]).await?;
    Ok(())
}

/// Rebases the checked-out branch onto `origin/<default_branch>`, as fresh
/// as the clone, even when it already contains it: replaying the branch's
/// commits drops the merge commits on it. A branch with none that contains
/// it is left as it is.
pub async fn rebase(dir: &Path, default_branch: &str) -> Result<Updated> {
    update_with(
        dir,
        &format!("origin/{default_branch}"),
        UpdateMethod::Rebase,
    )
    .await
}

async fn update_with(dir: &Path, upstream: &str, method: UpdateMethod) -> Result<Updated> {
    let attempt = match method {
        UpdateMethod::Rebase => git(dir, &["rebase", "--quiet", upstream]).await,
        UpdateMethod::Merge => git(dir, &["merge", "--quiet", "--no-edit", upstream]).await,
    };
    match attempt {
        Ok(_) => Ok(Updated::Updated),
        Err(error) => {
            let files: Vec<String> = git(dir, &["diff", "--name-only", "--diff-filter=U"])
                .await?
                .lines()
                .map(str::to_string)
                .collect();
            if files.is_empty() {
                Err(error)
            } else {
                Ok(Updated::Conflict(Conflict { method, files }))
            }
        }
    }
}

pub async fn commits_ahead(dir: &Path, base: &str) -> Result<usize> {
    Ok(git(dir, &["rev-list", "--count", &format!("{base}..HEAD")])
        .await?
        .trim()
        .parse()?)
}

/// Where the work in a clone stands: the commit checked out and all that is
/// uncommitted, untracked files included and ignored ones not.
#[derive(Debug, PartialEq, Eq)]
pub struct WorkState {
    head: String,
    tree: String,
}

/// Where the work in `dir` stands. Reading it changes nothing in `dir`.
pub async fn work_state(dir: &Path) -> Result<WorkState> {
    // The tree is written from a copy of the index, so the clone's own,
    // which may hold a conflict, is left as it is.
    let [scratch, index] = git_paths(dir, ["linewright-state-index", "index"]).await?;
    tokio::fs::copy(&index, &scratch).await?;
    let env = [("GIT_INDEX_FILE".to_string(), scratch.clone())];
    let tree = async {
        git_with(dir, &["add", "--all"], &env).await?;
        git_with(dir, &["write-tree"], &env).await
    }
    .await;
    let _ = tokio::fs::remove_file(&scratch).await;
    Ok(WorkState {
        head: head(dir).await?,
        tree: tree?.trim().to_string(),
    })
}

/// A rebase or merge left in progress.
#[derive(Clone, Copy)]
enum InProgress {
    Rebase(Backend),
    Merge,
}

/// How a rebase replays commits, which says where it keeps its state.
#[derive(Clone, Copy)]
enum Backend {
    Merge,
    Apply,
}

impl Backend {
    /// The directory of the git directory the rebase keeps its state in.
    fn state(self) -> &'static str {
        match self {
            Backend::Merge => "rebase-merge",
            Backend::Apply => "rebase-apply",
        }
    }
}

impl InProgress {
    fn describe(self) -> &'static str {
        match self {
            InProgress::Rebase(_) => "a rebase is in progress",
            InProgress::Merge => "a merge is in progress",
        }
    }

    /// The commit the branch was at before the operation, and the tree that
    /// holds the branch with the operation's `work` tree applied to it, so
    /// the work is kept without finishing the operation. A rebase's `work`
    /// is on the commits it has replayed, so the commits it has not yet
    /// replayed are merged into it.
    async fn kept(self, dir: &Path, work: &str) -> Result<(String, String)> {
        let InProgress::Rebase(backend) = self else {
            return Ok((head(dir).await?, work.to_string()));
        };
        let state = git(dir, &["rev-parse", "--git-path", backend.state()]).await?;
        let state = dir.join(state.trim());
        let read = async |file: &str| -> Result<String> {
            tokio::fs::read_to_string(state.join(file))
                .await
                .with_context(|| format!("reading the rebase's {file}"))
        };
        let original = read("orig-head").await?.trim().to_string();
        // The commit replayed last: the rebase stopped on it, or after it.
        let replayed = match backend {
            Backend::Merge => last_replayed(&read("done").await?)
                .context("the rebase in progress has replayed no commit")?
                .to_string(),
            Backend::Apply => read("original-commit").await?.trim().to_string(),
        };
        let ours = git(dir, &["commit-tree", work, "-p", "HEAD", "-m", "Work"]).await?;
        let tree = match merge_tree(dir, Some(&replayed), ours.trim(), &original).await? {
            Merged::Clean(tree) | Merged::Conflicted(tree) => tree,
        };
        Ok((original, tree))
    }

    /// Ends the operation, leaving the branch as it was before it.
    async fn abort(self, dir: &Path) -> Result<()> {
        match self {
            InProgress::Rebase(_) => git(dir, &["rebase", "--abort"]).await?,
            InProgress::Merge => git(dir, &["reset", "--quiet", "--hard"]).await?,
        };
        Ok(())
    }
}

/// The commit a merge rebase replayed last, from the commands it lists as
/// `done`.
fn last_replayed(done: &str) -> Option<&str> {
    const REPLAYING: [&str; 10] = [
        "pick", "p", "reword", "r", "edit", "e", "squash", "s", "fixup", "f",
    ];
    done.lines().rev().find_map(|line| {
        let mut words = line.split_whitespace();
        let command = words.next()?;
        REPLAYING.contains(&command).then(|| words.next()).flatten()
    })
}

/// The tree a merge writes, as it writes it.
#[derive(Debug, PartialEq, Eq)]
pub enum Merged {
    Clean(String),
    /// The tree with the conflicts marked in the files.
    Conflicted(String),
}

/// The tree that merges `theirs` into `ours` as of `base`, or of their merge
/// base when there is none
/// (<https://git-scm.com/docs/git-merge-tree>).
pub(crate) async fn merge_tree(
    dir: &Path,
    base: Option<&str>,
    ours: &str,
    theirs: &str,
) -> Result<Merged> {
    let base = base.map(|base| format!("--merge-base={base}"));
    let mut args = vec!["merge-tree", "--write-tree"];
    args.extend(base.as_deref());
    args.extend([ours, theirs]);
    let output = child::command(program())
        .args(&args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .with_context(|| format!("running git {}", args.join(" ")))?;
    // Exit status 1 is a merge with conflicts, whose tree comes first all the
    // same.
    let merged: fn(String) -> Merged = match output.status.code() {
        Some(0) => Merged::Clean,
        Some(1) => Merged::Conflicted,
        _ => bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(|tree| merged(tree.to_string()))
        .context("git merge-tree wrote no tree")
}

async fn in_progress(dir: &Path) -> Result<Option<InProgress>> {
    let paths = git_paths(dir, ["rebase-merge", "rebase-apply", "MERGE_HEAD"]).await?;
    let operations = [
        InProgress::Rebase(Backend::Merge),
        InProgress::Rebase(Backend::Apply),
        InProgress::Merge,
    ];
    Ok(paths
        .iter()
        .zip(operations)
        .find_map(|(path, operation)| Path::new(path).exists().then_some(operation)))
}

/// Where git keeps each of `names` in the git directory of `dir`, as
/// absolute paths, in one process: `rev-parse` prints a line per
/// `--git-path`.
async fn git_paths<const N: usize>(dir: &Path, names: [&str; N]) -> Result<[String; N]> {
    let mut args = vec!["rev-parse", "--path-format=absolute"];
    for name in names {
        args.extend(["--git-path", name]);
    }
    let output = git(dir, &args).await?;
    let paths: Vec<String> = output.lines().map(str::to_string).collect();
    paths.try_into().map_err(|paths: Vec<String>| {
        anyhow::anyhow!("git rev-parse printed {} paths, not {N}", paths.len())
    })
}

/// What keeps the work in `dir` from being finished: a rebase or merge in
/// progress, or uncommitted changes to tracked files.
pub async fn loose_ends(dir: &Path) -> Result<Option<String>> {
    if let Some(operation) = in_progress(dir).await? {
        return Ok(Some(format!("{}; finish it.", operation.describe())));
    }
    let status = git(dir, &["status", "--porcelain", "--untracked-files=no"]).await?;
    if !status.trim().is_empty() {
        return Ok(Some(format!(
            "there are uncommitted changes; commit them:\n\n```\n{}\n```",
            status.trim_end()
        )));
    }
    Ok(None)
}

/// The trailer that marks an unfinished commit.
const UNFINISHED_TRAILER: &str = "Linewright-Unfinished";

/// Commits all that is uncommitted in `dir`, untracked files included and
/// ignored ones not, as one unfinished commit. A rebase or merge in progress
/// is ended, and what it did so far is kept in the unfinished commit on the
/// branch as it was before it. The commit amends the branch's last commit
/// when that is an unfinished commit already, so the work a run leaves stays
/// in one.
pub async fn commit_unfinished(dir: &Path) -> Result<()> {
    git(dir, &["add", "--all"]).await?;
    let work = git(dir, &["write-tree"]).await?.trim().to_string();
    let (tip, tree) = match in_progress(dir).await? {
        Some(operation) => {
            let kept = operation.kept(dir, &work).await?;
            operation.abort(dir).await?;
            kept
        }
        None => (head(dir).await?, work),
    };
    let tip_tree = git(dir, &["rev-parse", &format!("{tip}^{{tree}}")]).await?;
    if tree == tip_tree.trim() {
        return Ok(());
    }
    let parent = if is_unfinished(dir, &tip).await? {
        format!("{tip}^")
    } else {
        tip
    };
    let trailer = format!("{UNFINISHED_TRAILER}: true");
    let commit = git(
        dir,
        &[
            "commit-tree",
            &tree,
            "-p",
            &parent,
            "-m",
            "Unfinished work, kept when its run stopped",
            "-m",
            "Linewright committed what the session left uncommitted when its run \
             stopped. The next run folds it into its own commits.",
            "-m",
            &trailer,
        ],
    )
    .await?;
    git(dir, &["reset", "--quiet", "--hard", commit.trim()]).await?;
    Ok(())
}

async fn is_unfinished(dir: &Path, commit: &str) -> Result<bool> {
    let format = format!("--format=%(trailers:key={UNFINISHED_TRAILER},valueonly)");
    Ok(git(dir, &["log", "-1", &format, commit])
        .await?
        .lines()
        .any(|line| !line.trim().is_empty()))
}

/// The abbreviated names of one or more unfinished commits, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnfinishedCommits {
    pub first: String,
    pub rest: Vec<String>,
}

/// A commit on a branch: its abbreviated name, and whether it is an
/// unfinished commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub name: String,
    pub unfinished: bool,
}

/// The commits on `tip` past `base`, oldest first.
pub async fn commits_past(dir: &Path, base: &str, tip: &str) -> Result<Vec<Commit>> {
    let format =
        format!("--format=%h %(trailers:key={UNFINISHED_TRAILER},valueonly,separator=%x2C)");
    Ok(git(
        dir,
        &["log", "--reverse", &format, &format!("{base}..{tip}")],
    )
    .await?
    .lines()
    .filter_map(|line| {
        let (name, marked) = line.split_once(' ')?;
        Some(Commit {
            name: name.to_string(),
            unfinished: !marked.trim().is_empty(),
        })
    })
    .collect())
}

/// The unfinished ones of `commits`, if any.
pub fn unfinished_of(commits: &[Commit]) -> Option<UnfinishedCommits> {
    let mut unfinished = commits
        .iter()
        .filter(|commit| commit.unfinished)
        .map(|commit| commit.name.clone());
    unfinished.next().map(|first| UnfinishedCommits {
        first,
        rest: unfinished.collect(),
    })
}

/// The unfinished commits on `tip` past `base`, if any.
pub async fn unfinished_commits(
    dir: &Path,
    base: &str,
    tip: &str,
) -> Result<Option<UnfinishedCommits>> {
    Ok(unfinished_of(&commits_past(dir, base, tip).await?))
}

/// Lines a commit adds to a file, one after another.
#[derive(Debug, PartialEq, Eq)]
pub struct Addition {
    /// The commit's abbreviated name.
    pub commit: String,
    pub file: String,
    /// The lines, joined by newlines, so that a value that spans them is
    /// found whole.
    pub text: String,
}

/// What the commits on `tip` past each of `past` add, oldest commit first. A
/// merge adds only what it holds that none of its parents did, as its
/// resolution of a conflict does. Binary files add nothing.
pub async fn additions(dir: &Path, tip: &str, past: &[&str]) -> Result<Vec<Addition>> {
    let mut args = vec![
        "-c",
        "core.quotePath=false",
        "log",
        "--reverse",
        "--format=%x00%h",
        "--patch",
        "--cc",
        "--unified=0",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        tip,
        "--not",
    ];
    args.extend(past);
    args.push("--");
    Ok(parse_additions(&git(dir, &args).await?))
}

/// The additions in `log`, as [`additions`] has git print it: each commit's
/// name after a NUL, then its patch, or combined patch for a merge
/// (<https://git-scm.com/docs/git-diff#_combined_diff_format>).
fn parse_additions(log: &str) -> Vec<Addition> {
    let mut additions = vec![];
    let mut commit = "";
    let mut file = String::new();
    // The width of the hunk lines' prefix, one column per parent, once in a
    // hunk.
    let mut columns: Option<usize> = None;
    // The added lines since the last line that was not one.
    let mut run: Vec<&str> = vec![];
    for line in log.lines() {
        if let Some(width) = columns
            && let Some((prefix, rest)) = line.split_at_checked(width)
            && prefix.contains('+')
        {
            run.push(rest);
            continue;
        }
        end_run(&mut additions, &mut run, commit, &file);
        if let Some(name) = line.strip_prefix('\0') {
            commit = name;
            columns = None;
        } else if line.starts_with("diff ") {
            columns = None;
        } else if columns.is_none()
            && let Some(path) = line.strip_prefix("+++ ")
        {
            let path = path.trim_matches('"');
            file = path.strip_prefix("b/").unwrap_or(path).to_string();
        } else if line.starts_with("@@") {
            columns = Some(line.chars().take_while(|c| *c == '@').count() - 1);
        }
    }
    end_run(&mut additions, &mut run, commit, &file);
    additions
}

/// Adds the `run` of lines `commit` adds to `file` to `additions`, if any,
/// and starts the next.
fn end_run(additions: &mut Vec<Addition>, run: &mut Vec<&str>, commit: &str, file: &str) {
    if !run.is_empty() {
        additions.push(Addition {
            commit: commit.to_string(),
            file: file.to_string(),
            text: run.join("\n"),
        });
        run.clear();
    }
}
