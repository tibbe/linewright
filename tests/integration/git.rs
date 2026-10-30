use crate::common;

use common::{
    UNFINISHED, UNFINISHED_MESSAGE, assert_in_a_process_group_of_its_own, commit_in, git_in,
    git_output,
};
use linewright::domain::{UpdateMethod, UpdateRule};
use linewright::git::{self, Addition, BranchStart, Commit, UnfinishedCommits, Updated};
use std::path::{Path, PathBuf};

/// So the SIGINT a terminal sends `serve`'s process group on Ctrl-C, which
/// drains, does not also end the `git` commands of the runs in flight.
#[tokio::test]
async fn git_runs_in_a_process_group_of_its_own() {
    let dir = tempfile::tempdir().unwrap();

    // A shell alias runs in `git`'s process group.
    let group = git::git(
        dir.path(),
        &["-c", "alias.group=!ps -o pgid= -p $$", "group"],
    )
    .await
    .unwrap();

    assert_in_a_process_group_of_its_own(&group);
}

/// A repository standing in for the remote, whose `main` starts with
/// `src/gadgets.txt` and a `.gitignore` of `*.log`, and a clone of it.
struct Repository {
    _dir: tempfile::TempDir,
    remote: PathBuf,
    clone: PathBuf,
    /// The commit `main` starts at.
    begin: String,
}

impl Repository {
    fn new() -> Repository {
        Repository::with_remote(|_| {})
    }

    /// The repository, with `remote` run in the remote before it is cloned.
    fn with_remote(remote: impl FnOnce(&Path)) -> Repository {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "--quiet", "-b", "main"]);
        as_tester(&origin);
        commit_in(
            &origin,
            &[("src/gadgets.txt", "gadgets\n"), (".gitignore", "*.log\n")],
            "Begin",
        );
        let begin = git_in(&origin, &["rev-parse", "HEAD"]).trim().to_string();
        remote(&origin);
        git_in(dir.path(), &["clone", "--quiet", "origin", "clone"]);
        let clone = dir.path().join("clone");
        as_tester(&clone);
        Repository {
            _dir: dir,
            remote: origin,
            clone,
            begin,
        }
    }

    /// Commits `files` to the remote's `main`, and fetches it into the
    /// clone.
    fn main_moved_on(&self, files: &[(&str, &str)], message: &str) {
        git_in(&self.remote, &["checkout", "--quiet", "main"]);
        commit_in(&self.remote, files, message);
        git_in(&self.clone, &["fetch", "--quiet", "origin"]);
    }

    /// Checks out a new branch `branch` at `main` in the clone, and commits
    /// each of `commits` to it.
    fn branch(&self, branch: &str, commits: &[(&[(&str, &str)], &str)]) {
        git_in(&self.clone, &["checkout", "--quiet", "-b", branch]);
        for (files, message) in commits {
            commit_in(&self.clone, files, message);
        }
    }

    fn write(&self, path: &str, contents: &str) {
        std::fs::write(self.clone.join(path), contents).unwrap();
    }

    fn file(&self, path: &str) -> String {
        std::fs::read_to_string(self.clone.join(path)).unwrap()
    }

    /// What `path` holds in the clone's `HEAD`, if anything.
    fn committed(&self, path: &str) -> Option<String> {
        let output = git_output(&self.clone, &["show", &format!("HEAD:{path}")]);
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap())
    }

    /// The subjects of the commits on the clone's `HEAD` past where `main`
    /// began, oldest first, merges included.
    fn subjects(&self) -> Vec<String> {
        let range = format!("{}..HEAD", self.begin);
        git_in(&self.clone, &["log", "--reverse", "--format=%s", &range])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.clone, args).trim().to_string()
    }

    /// The abbreviated name of the clone's `HEAD`.
    fn short_head(&self) -> String {
        self.git(&["rev-parse", "--short", "HEAD"])
    }

    /// Whether the clone's `HEAD` is an unfinished commit, with the whole
    /// message Linewright gives one.
    fn head_unfinished(&self) -> bool {
        self.git(&["log", "-1", "--format=%B"]) == UNFINISHED_MESSAGE
    }

    /// Whether a rebase or merge is in progress in the clone.
    fn mid_operation(&self) -> bool {
        ["rebase-merge", "rebase-apply", "MERGE_HEAD"]
            .iter()
            .any(|name| {
                let path = self.git(&["rev-parse", "--path-format=absolute", "--git-path", name]);
                Path::new(&path).exists()
            })
    }
}

fn as_tester(dir: &Path) {
    git_in(dir, &["config", "user.name", "tester"]);
    git_in(dir, &["config", "user.email", "tester@example.com"]);
}

/// Runs git in `dir`, which fails, as a rebase or merge stopping on a
/// conflict does.
fn git_failing(dir: &Path, args: &[&str]) {
    let output = git_output(dir, args);
    assert!(!output.status.success(), "git {args:?} stops on a conflict");
}

/// Commits `files` as an unfinished commit Linewright made.
fn commit_marked_unfinished(dir: &Path, files: &[(&str, &str)]) {
    commit_in(dir, files, UNFINISHED_MESSAGE);
}

#[tokio::test]
async fn an_unfinished_commit_holds_the_edits_and_new_files_but_no_ignored_file() {
    let repository = Repository::new();
    repository.branch("12-gadgets", &[(&[("src/gadgets.txt", "half\n")], "Start")]);
    repository.write("src/gadgets.txt", "half and more\n");
    repository.write("src/new.txt", "new\n");
    repository.write("build.log", "noise\n");

    git::commit_unfinished(&repository.clone).await.unwrap();

    assert_eq!(repository.subjects(), ["Start", UNFINISHED]);
    assert!(repository.head_unfinished());
    assert_eq!(
        repository.committed("src/gadgets.txt").as_deref(),
        Some("half and more\n")
    );
    assert_eq!(
        repository.committed("src/new.txt").as_deref(),
        Some("new\n")
    );
    assert_eq!(repository.committed("build.log"), None);
    assert_eq!(repository.git(&["status", "--porcelain"]), "");
}

#[tokio::test]
async fn with_nothing_uncommitted_but_ignored_files_no_unfinished_commit_is_made() {
    let repository = Repository::new();
    repository.branch("12-gadgets", &[(&[("src/gadgets.txt", "half\n")], "Start")]);
    repository.write("build.log", "noise\n");

    git::commit_unfinished(&repository.clone).await.unwrap();

    assert_eq!(repository.subjects(), ["Start"]);
}

#[tokio::test]
async fn an_unfinished_commit_already_at_head_is_amended_not_followed_by_a_second() {
    let repository = Repository::new();
    repository.branch("12-gadgets", &[(&[("src/gadgets.txt", "half\n")], "Start")]);
    commit_marked_unfinished(&repository.clone, &[("src/new.txt", "new\n")]);
    repository.write("src/new.txt", "newer\n");
    repository.write("src/more.txt", "more\n");

    git::commit_unfinished(&repository.clone).await.unwrap();

    assert_eq!(repository.subjects(), ["Start", UNFINISHED]);
    assert!(repository.head_unfinished());
    assert_eq!(
        repository.committed("src/new.txt").as_deref(),
        Some("newer\n")
    );
    assert_eq!(
        repository.committed("src/more.txt").as_deref(),
        Some("more\n")
    );
}

#[tokio::test]
async fn a_rebase_stopped_on_a_conflict_is_ended_keeping_its_resolution_new_files_and_commits_not_yet_replayed()
 {
    let repository = Repository::new();
    repository.branch(
        "12-gadgets",
        &[
            (&[("src/gadgets.txt", "ours\n")], "Start"),
            (&[("src/more.txt", "more\n")], "More"),
        ],
    );
    repository.main_moved_on(&[("src/gadgets.txt", "theirs\n")], "Theirs");
    git_failing(&repository.clone, &["rebase", "--quiet", "origin/main"]);
    repository.write("src/gadgets.txt", "ours, then theirs\n");
    repository.write("src/new.txt", "new\n");

    git::commit_unfinished(&repository.clone).await.unwrap();

    assert!(!repository.mid_operation());
    assert_eq!(repository.git(&["branch", "--show-current"]), "12-gadgets");
    assert_eq!(repository.subjects(), ["Start", "More", UNFINISHED]);
    assert!(repository.head_unfinished());
    assert_eq!(repository.file("src/gadgets.txt"), "ours, then theirs\n");
    assert_eq!(repository.file("src/more.txt"), "more\n");
    assert_eq!(repository.file("src/new.txt"), "new\n");
    assert_eq!(repository.git(&["status", "--porcelain"]), "");
}

#[tokio::test]
async fn a_merge_stopped_on_a_conflict_is_ended_keeping_its_resolution_in_an_unfinished_commit_not_a_merge()
 {
    let repository = Repository::new();
    repository.branch("12-gadgets", &[(&[("src/gadgets.txt", "ours\n")], "Start")]);
    repository.main_moved_on(&[("src/gadgets.txt", "theirs\n")], "Theirs");
    git_failing(
        &repository.clone,
        &["merge", "--quiet", "--no-edit", "origin/main"],
    );
    repository.write("src/gadgets.txt", "ours, then theirs\n");

    git::commit_unfinished(&repository.clone).await.unwrap();

    assert!(!repository.mid_operation());
    assert_eq!(repository.subjects(), ["Start", UNFINISHED]);
    assert_eq!(repository.git(&["rev-list", "--merges", "HEAD"]), "");
    assert!(repository.head_unfinished());
    assert_eq!(repository.file("src/gadgets.txt"), "ours, then theirs\n");
    assert_eq!(repository.git(&["status", "--porcelain"]), "");
}

fn commit(name: &str, unfinished: bool) -> Commit {
    Commit {
        name: name.to_string(),
        unfinished,
    }
}

#[test]
fn the_unfinished_of_a_branchs_commits_are_named_oldest_first() {
    let cases: [(Vec<Commit>, Option<UnfinishedCommits>); 4] = [
        (vec![], None),
        (
            vec![commit("aaa1111", false), commit("bbb2222", false)],
            None,
        ),
        (
            vec![commit("aaa1111", false), commit("bbb2222", true)],
            Some(UnfinishedCommits {
                first: "bbb2222".to_string(),
                rest: vec![],
            }),
        ),
        (
            vec![
                commit("aaa1111", true),
                commit("bbb2222", false),
                commit("ccc3333", true),
            ],
            Some(UnfinishedCommits {
                first: "aaa1111".to_string(),
                rest: vec!["ccc3333".to_string()],
            }),
        ),
    ];
    for (commits, unfinished) in cases {
        assert_eq!(git::unfinished_of(&commits), unfinished, "{commits:?}");
    }
}

#[tokio::test]
async fn a_branch_the_remote_has_is_checked_out_as_it_has_it_with_its_unfinished_commits() {
    let mut unfinished = String::new();
    let repository = Repository::with_remote(|remote| {
        git_in(remote, &["checkout", "--quiet", "-b", "12-gadgets"]);
        commit_in(remote, &[("src/gadgets.txt", "half\n")], "Start");
        commit_marked_unfinished(remote, &[("src/partial.txt", "half\n")]);
        unfinished = git_in(remote, &["rev-parse", "--short", "HEAD"])
            .trim()
            .to_string();
        git_in(remote, &["checkout", "--quiet", "main"]);
    });

    let start = git::check_out_branch(&repository.clone, "12-gadgets", "origin/main")
        .await
        .unwrap();

    assert_eq!(
        start,
        BranchStart::Existing {
            unfinished: Some(UnfinishedCommits {
                first: unfinished,
                rest: vec![],
            }),
        }
    );
    assert_eq!(repository.git(&["branch", "--show-current"]), "12-gadgets");
    assert_eq!(repository.file("src/partial.txt"), "half\n");
}

#[tokio::test]
async fn a_branch_the_remote_lacks_is_made_new_at_the_commit_checked_out() {
    let repository = Repository::new();

    let start = git::check_out_branch(&repository.clone, "12-gadgets", "origin/main")
        .await
        .unwrap();

    assert_eq!(start, BranchStart::New);
    assert_eq!(repository.git(&["branch", "--show-current"]), "12-gadgets");
    assert_eq!(repository.git(&["rev-parse", "HEAD"]), repository.begin);
}

#[tokio::test]
async fn the_commits_reachable_from_past_add_nothing() {
    let repository = Repository::new();
    repository.branch("12-gadgets", &[(&[("src/held.txt", "held\n")], "Held")]);
    let past = repository.git(&["rev-parse", "HEAD"]);
    commit_in(&repository.clone, &[("src/new.txt", "new\n")], "New");

    let additions = git::additions(&repository.clone, "HEAD", &[&past])
        .await
        .unwrap();

    assert_eq!(
        additions,
        [Addition {
            commit: repository.short_head(),
            file: "src/new.txt".to_string(),
            text: "new".to_string(),
        }]
    );
}

#[tokio::test]
async fn the_default_branch_a_branch_took_in_adds_nothing() {
    let repository = Repository::new();
    repository.branch("12-gadgets", &[(&[("src/ours.txt", "ours\n")], "Ours")]);
    let start = repository.git(&["rev-parse", "HEAD"]);
    repository.main_moved_on(&[("src/theirs.txt", "theirs\n")], "Theirs");
    git_in(
        &repository.clone,
        &["merge", "--quiet", "--no-edit", "origin/main"],
    );
    commit_in(&repository.clone, &[("src/notes.txt", "notes\n")], "Notes");

    let additions = git::additions(&repository.clone, "HEAD", &[&start, "origin/main"])
        .await
        .unwrap();

    assert_eq!(
        additions,
        [Addition {
            commit: repository.short_head(),
            file: "src/notes.txt".to_string(),
            text: "notes".to_string(),
        }]
    );
}

#[tokio::test]
async fn lines_added_one_after_another_are_one_addition() {
    let repository = Repository::new();
    repository.branch("12-gadgets", &[(&[("key.pem", "one\ntwo\n")], "Start")]);
    let past = repository.git(&["rev-parse", "HEAD"]);
    commit_in(
        &repository.clone,
        &[(
            "key.pem",
            "one\n-----BEGIN KEY-----\nc2VjcmV0\n-----END KEY-----\ntwo\nthree\n",
        )],
        "Add the key",
    );

    let additions = git::additions(&repository.clone, "HEAD", &[&past])
        .await
        .unwrap();

    let commit = repository.short_head();
    assert_eq!(
        additions,
        [
            Addition {
                commit: commit.clone(),
                file: "key.pem".to_string(),
                text: "-----BEGIN KEY-----\nc2VjcmV0\n-----END KEY-----".to_string(),
            },
            Addition {
                commit,
                file: "key.pem".to_string(),
                text: "three".to_string(),
            },
        ]
    );
}

/// A clone on the branch `12-gadgets`, with a commit to `src/gadgets.txt`,
/// after `main` moved on with a commit to `path`.
fn behind_main(path: &str) -> Repository {
    let repository = Repository::new();
    repository.main_moved_on(&[(path, "theirs\n")], "Move on");
    repository.branch("12-gadgets", &[(&[("src/gadgets.txt", "ours\n")], "Start")]);
    repository
}

/// The method of the update `git::update` stops in on a conflict in
/// `src/gadgets.txt`, if it stops, and the subjects of the commits then on
/// the branch's first-parent line, oldest first.
async fn updated(repository: &Repository, rule: UpdateRule) -> (Option<UpdateMethod>, Vec<String>) {
    let stopped = match git::update(&repository.clone, "main", rule).await.unwrap() {
        Updated::Conflict(conflict) => {
            assert_eq!(conflict.files, ["src/gadgets.txt"]);
            assert!(repository.mid_operation());
            Some(conflict.method)
        }
        Updated::Updated => None,
        Updated::UpToDate => panic!("the branch is behind"),
    };
    let range = format!("{}..HEAD", repository.begin);
    let subjects = repository.git(&["log", "--first-parent", "--reverse", "--format=%s", &range]);
    (stopped, subjects.lines().map(str::to_string).collect())
}

#[tokio::test]
async fn an_update_always_merging_merges_the_default_branch_in_and_stops_in_a_merge_on_a_conflict()
{
    let rule = UpdateRule::Always(UpdateMethod::Merge);

    assert_eq!(
        updated(&behind_main("src/other.txt"), rule).await,
        (
            None,
            vec![
                "Start".to_string(),
                "Merge remote-tracking branch 'origin/main' into 12-gadgets".to_string(),
            ]
        )
    );
    assert_eq!(
        updated(&behind_main("src/gadgets.txt"), rule).await.0,
        Some(UpdateMethod::Merge)
    );
}

#[tokio::test]
async fn an_update_always_rebasing_rebases_onto_the_default_branch_and_stops_in_a_rebase_on_a_conflict()
 {
    let rule = UpdateRule::Always(UpdateMethod::Rebase);

    assert_eq!(
        updated(&behind_main("src/other.txt"), rule).await,
        (None, vec!["Move on".to_string(), "Start".to_string()])
    );
    assert_eq!(
        updated(&behind_main("src/gadgets.txt"), rule).await.0,
        Some(UpdateMethod::Rebase)
    );
}
