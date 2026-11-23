use std::collections::BTreeSet;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::TimeDelta;
use linewright_core::benchmarks::Benchmarks;
use linewright_core::clock::RunningFrom;
use linewright_core::domain::{MergeMethod, Number, Scope, Windows, label};
use linewright_core::git::CommitId;
use linewright_core::input;
use linewright_core::name::Name;
use linewright_core::reconcile::Work;
use linewright_core::spec_gap;
use linewright_core::store::local::LocalStore;
use linewright_core::store::{NewIssue, NewPull, Store};
use linewright_core::task::{
    self, BenchmarkChoice, Criteria, Merge, Task, TaskItemKind, TaskPart, TaskPull, TaskPullState,
};
use linewright_core::testing::*;

/// A design run on the first ticket of a spec change cut into tickets,
/// given its parent, its spec change and an open spec gap filed for it,
/// captured as a task.
struct DesignTask {
    world: World,
    dir: PathBuf,
    ticket: Number,
    tracking: Number,
    spec_change: Number,
    gap: Number,
}

#[track_caller]
fn design_task() -> impl Future<Output = DesignTask> {
    let caller = Location::caller();
    async move {
        let mut world = World::new().await;
        let cut = world.spec_change_cut_into_tickets().await;
        let ticket = cut.tickets[0];
        let spec_change = world.issue(ticket).parent.and_then(|p| p.spec_change);
        let spec_change = ok(spec_change.ok_or("none"), caller, "the spec change");
        let gap = world
            .store
            .create_issue(NewIssue {
                title: "Should gadgets expire?".into(),
                body: spec_gap::body("The spec does not say.", ticket),
                labels: vec!["ready-for-human".into()],
            })
            .await;
        let gap = ok(gap, caller, "filing the spec gap");
        let commented = world
            .store
            .comment_as("maintainer", ticket, "Written before.");
        ok(commented, caller, "commenting");
        let unlabeled = world
            .store
            .remove_label(ticket, "linewright:designed")
            .await;
        ok(unlabeled, caller, "unlabeling");
        design_issues(&world);
        world.poll().await;
        let work = world.dir.path().join("work");
        let name: Name = ok("design".parse(), caller, "the name");
        let run = ok(
            format!("{}/design-1", ticket.0).parse(),
            caller,
            "the run's ID",
        );
        let runs = runs_in(&work).await;
        let benchmarks = Benchmarks::new(runs.database().clone());
        let benchmark = BenchmarkChoice::New(ok("designs".parse(), caller, "the suite"));
        let capturable = task::capturable(&runs, &benchmarks, &run, &name, &benchmark).await;
        let capturable = ok(capturable, caller, "the run");
        let criteria = ok(
            Criteria::parse("The design cuts the work in two.\n").ok_or("blank"),
            caller,
            "the criteria",
        );
        let dir = task::capture(&*world.store, capturable, &criteria, &benchmarks).await;
        let dir = ok(dir, caller, "capturing").dir;
        DesignTask {
            world,
            dir,
            ticket,
            tracking: cut.tracking,
            spec_change,
            gap,
        }
    }
}

#[track_caller]
fn open(dir: &Path) -> impl Future<Output = Task> + '_ {
    let caller = Location::caller();
    async move {
        Task::open(dir)
            .await
            .unwrap_or_else(|refusal| panic!("opening the case at {caller}: {refusal}"))
    }
}

/// A store seeded from `task` under `root`, on the clock running from the
/// task's start.
#[track_caller]
fn seeded<'a>(root: &'a Path, task: &'a Task) -> impl Future<Output = LocalStore> + 'a {
    let caller = Location::caller();
    async move {
        LocalStore::from_task(
            &root.join("store"),
            &root.join("repository.git"),
            task,
            Arc::new(RunningFrom::new(task.start())),
        )
        .await
        .unwrap_or_else(|e| panic!("seeding the store at {caller}: {e:#}"))
    }
}

fn numbers(task: &Task) -> BTreeSet<Number> {
    task.items().iter().map(|item| item.number).collect()
}

#[test_log::test(tokio::test)]
async fn the_store_seeded_from_a_task_gives_back_what_the_task_holds() {
    let DesignTask { world, dir, .. } = design_task().await;
    let task = open(&dir).await;
    let store = seeded(world.dir.path(), &task).await;

    let again = world.dir.path().join("again");
    std::fs::create_dir(&again).unwrap();
    for file in ["task.json", "criteria.md"] {
        std::fs::copy(dir.join(file), again.join(file)).unwrap();
    }
    let items = store.record_items(&numbers(&task)).await.unwrap();
    std::fs::write(
        again.join("items.json"),
        serde_json::to_vec(&items).unwrap(),
    )
    .unwrap();
    let properties = store.record_properties().await.unwrap();
    std::fs::write(
        again.join("properties.json"),
        serde_json::to_vec(&properties).unwrap(),
    )
    .unwrap();
    store
        .fetch_commits(&again.join("repository.git"), task.heads())
        .await
        .unwrap();
    let reopened = open(&again).await;

    assert_eq!(reopened.items(), task.items());
    assert_eq!(reopened.properties(), task.properties());
    assert_eq!(reopened.heads(), task.heads());
    assert_eq!(
        (reopened.stage(), reopened.item(), reopened.start()),
        (task.stage(), task.item(), task.start())
    );
}

#[test_log::test(tokio::test)]
async fn the_snapshot_of_the_seeded_store_holds_the_tasks_item_linked_as_in_the_task() {
    let DesignTask {
        world,
        dir,
        ticket,
        tracking,
        spec_change,
        gap,
    } = design_task().await;
    let task = open(&dir).await;
    let store = seeded(world.dir.path(), &task).await;

    let windows = Windows::at(task.start(), TimeDelta::days(7), TimeDelta::days(7));
    let snapshot = store.snapshot(windows, &Scope::default()).await.unwrap();

    let issue = snapshot
        .issue(ticket)
        .expect("the snapshot holds the ticket");
    let parent = issue.parent.expect("the ticket has its parent");
    assert_eq!(
        (parent.number, parent.spec_change),
        (tracking, Some(spec_change))
    );
    assert_eq!(
        spec_gap::filed_for(&snapshot, ticket)
            .into_iter()
            .map(|filed| filed.number)
            .collect::<Vec<_>>(),
        [gap]
    );
    assert!(
        snapshot
            .merged
            .iter()
            .any(|pull| pull.number == spec_change),
        "{:?}",
        snapshot.merged
    );
    let wanted = input::wanted(&snapshot, &Work::DesignIssue { issue: ticket }, "main").unwrap();
    assert_eq!(wanted.items, numbers(&task));
    let diff = store
        .diff(spec_change, &["docs/spec.md".to_string()])
        .await
        .unwrap();
    assert!(diff.contains("+Gadgets exist."), "{diff}");
}

#[test_log::test(tokio::test)]
async fn the_seeded_store_numbers_the_next_issue_and_pull_request_above_the_tasks_items() {
    let DesignTask { world, dir, .. } = design_task().await;
    let task = open(&dir).await;
    let store = seeded(world.dir.path(), &task).await;
    let highest = numbers(&task).last().copied().unwrap();

    let issue = store
        .create_issue(NewIssue {
            title: "Gadgets expire".into(),
            body: String::new(),
            labels: vec![],
        })
        .await
        .unwrap();
    store
        .commit("factory", "gadgets", &[("src/expiry.rs", "")], "Expire")
        .await
        .unwrap();
    let pull = store
        .open_pull(NewPull {
            head: "gadgets".into(),
            title: "Expire gadgets".into(),
            body: String::new(),
        })
        .await
        .unwrap();

    assert_eq!(
        (issue, pull),
        (Number(highest.0 + 1), Number(highest.0 + 2))
    );
}

#[test_log::test(tokio::test)]
async fn each_of_linewrights_labels_is_present_in_the_seeded_store() {
    let DesignTask { world, dir, .. } = design_task().await;
    let task = open(&dir).await;

    let store = seeded(world.dir.path(), &task).await;

    for label in label::ALL {
        assert!(
            store.label_exists(label.name).await.unwrap(),
            "{}",
            label.name
        );
    }
}

/// A copy of the task in `dir`, at `to`.
#[track_caller]
fn copy_task(dir: &Path, to: &Path) {
    let status = std::process::Command::new("cp")
        .args(["-R"])
        .arg(dir)
        .arg(to)
        .status()
        .unwrap();
    assert!(status.success(), "copying {}", dir.display());
}

/// Sets the field `name` of the task's `task.json` to `value`.
#[track_caller]
fn set_in_task_file(dir: &Path, name: &str, value: serde_json::Value) {
    let path = dir.join("task.json");
    let mut file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file[name] = value;
    std::fs::write(&path, file.to_string()).unwrap();
}

/// What spoils a copy of a task, given its directory.
type Spoil = Box<dyn Fn(&Path)>;

#[test_log::test(tokio::test)]
async fn a_task_lacking_a_part_or_holding_one_that_does_not_read_is_refused_naming_the_part() {
    let DesignTask { world, dir, .. } = design_task().await;
    let remove = |file: &'static str| {
        Box::new(move |task_dir: &Path| {
            let path = task_dir.join(file);
            if path.is_dir() {
                std::fs::remove_dir_all(path).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }) as Spoil
    };
    let write = |file: &'static str, contents: &'static [u8]| {
        Box::new(move |task_dir: &Path| {
            let path = task_dir.join(file);
            if path.is_dir() {
                std::fs::remove_dir_all(&path).unwrap();
            }
            std::fs::write(path, contents).unwrap();
        }) as Spoil
    };
    let set = |name: &'static str, value: serde_json::Value| {
        Box::new(move |task_dir: &Path| set_in_task_file(task_dir, name, value.clone())) as Spoil
    };
    let empty_repository = Box::new(|task_dir: &Path| {
        let path = task_dir.join("repository.git");
        std::fs::remove_dir_all(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
    }) as Spoil;
    let forgotten_history = Box::new(|task_dir: &Path| {
        let repository = task_dir.join("repository.git");
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg(format!("--git-dir={}", repository.display()))
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        let tree = git(&["mktree"]);
        let fresh = git(&[
            "-c",
            "user.name=Maintainer",
            "-c",
            "user.email=maintainer@example.com",
            "commit-tree",
            "-m",
            "Afresh",
            &tree,
        ]);
        git(&["update-ref", "refs/heads/main", &fresh]);
        git(&["reflog", "expire", "--expire=now", "--all"]);
        git(&["gc", "--quiet", "--prune=now"]);
    }) as Spoil;
    let tasks: Vec<(&str, Spoil, TaskPart)> = vec![
        (
            "a repository without the spec change's merge",
            forgotten_history,
            TaskPart::Repository,
        ),
        ("no task.json", remove("task.json"), TaskPart::Stage),
        ("no items.json", remove("items.json"), TaskPart::Items),
        (
            "no properties.json",
            remove("properties.json"),
            TaskPart::Properties,
        ),
        (
            "no repository.git",
            remove("repository.git"),
            TaskPart::Repository,
        ),
        (
            "task.json not JSON",
            write("task.json", b"{"),
            TaskPart::Stage,
        ),
        (
            "a rework case",
            set("stage", serde_json::json!("rework")),
            TaskPart::Stage,
        ),
        (
            "an item not a number",
            set("item", serde_json::json!("one")),
            TaskPart::Item,
        ),
        (
            "a start not a time",
            set("start", serde_json::json!("soon")),
            TaskPart::Start,
        ),
        (
            "a run not an ID",
            set("run", serde_json::json!("")),
            TaskPart::Run,
        ),
        (
            "an item not among the items",
            set("item", serde_json::json!(999)),
            TaskPart::Items,
        ),
        (
            "items.json not JSON",
            write("items.json", b"["),
            TaskPart::Items,
        ),
        (
            "an item without its resources",
            write("items.json", br#"[{"number": 1, "resources": {}}]"#),
            TaskPart::Items,
        ),
        (
            "properties.json without the properties",
            write("properties.json", br#"{"resources": {}}"#),
            TaskPart::Properties,
        ),
        (
            "repository.git not a repository",
            empty_repository,
            TaskPart::Repository,
        ),
        ("no criteria.md", remove("criteria.md"), TaskPart::Criteria),
        (
            "criteria.md only white space",
            write("criteria.md", b" \n\t\n"),
            TaskPart::Criteria,
        ),
        (
            "criteria.md not text",
            write("criteria.md", b"\xff\xfe"),
            TaskPart::Criteria,
        ),
    ];
    for (n, (what, spoil, part)) in tasks.into_iter().enumerate() {
        let copy = world.dir.path().join(format!("spoiled-{n}"));
        copy_task(&dir, &copy);
        spoil(&copy);

        let refusal = Task::open(&copy).await.err();

        assert_eq!(refusal.map(|r| r.part), Some(part), "{what}");
    }
}

#[test_log::test(tokio::test)]
async fn a_task_opens_with_its_criteria() {
    let DesignTask {
        world: _world, dir, ..
    } = design_task().await;

    let task = open(&dir).await;

    assert_eq!(
        task.criteria().as_str(),
        "The design cuts the work in two.\n"
    );
}

/// `result`'s value, failing the test at `caller` for `what` otherwise.
fn ok<T, E: std::fmt::Debug>(result: Result<T, E>, caller: &Location<'_>, what: &str) -> T {
    result.unwrap_or_else(|e| panic!("{what} at {caller}: {e:?}"))
}

/// The head of `branch` in `store`.
async fn head(caller: &Location<'_>, store: &LocalStore, branch: &str) -> CommitId {
    let heads = ok(store.heads(&[branch.to_string()]).await, caller, branch);
    heads[branch].clone()
}

/// [`design_task`]'s, with a pull request of two commits then merged by
/// rebase, captured into a task of its own: the task's directory, the pull
/// request, and what its merge did as Linewright reads it.
#[track_caller]
fn task_with_a_rebased_pull() -> impl Future<Output = (DesignTask, PathBuf, Number, Merge)> {
    let caller = Location::caller();
    async move {
        let design = design_task().await;
        let store = &design.world.store;
        let pull = ok(
            store
                .open_pull_as(
                    "maintainer",
                    "expiry",
                    "Expire gadgets",
                    "",
                    &[("docs/spec.md", "# Spec\n\nGadgets exist.\nThey expire.\n")],
                )
                .await,
            caller,
            "opening the pull request",
        );
        let said_when = store
            .commit(
                "maintainer",
                "expiry",
                &[("docs/expiry.md", "# Expiry\n")],
                "Say when",
            )
            .await;
        ok(said_when, caller, "committing to the pull request");
        let before = head(caller, store, "main").await;
        let tip = head(caller, store, "expiry").await;
        let merged = store.merge(pull, &tip, MergeMethod::Rebase).await;
        ok(merged, caller, "merging");
        let after = head(caller, store, "main").await;
        let mut numbers = numbers(&open(&design.dir).await);
        numbers.insert(pull);
        let dir = design.world.dir.path().join("with-the-pull");
        let items = ok(store.record_items(&numbers).await, caller, "recording");
        let written = std::fs::create_dir(&dir)
            .and_then(|()| std::fs::copy(design.dir.join("task.json"), dir.join("task.json")))
            .and_then(|_| std::fs::copy(design.dir.join("criteria.md"), dir.join("criteria.md")))
            .and_then(|_| {
                std::fs::copy(
                    design.dir.join("properties.json"),
                    dir.join("properties.json"),
                )
            })
            .and_then(|_| std::fs::write(dir.join("items.json"), serde_json::to_vec(&items)?));
        ok(written, caller, "writing the case");
        let heads = [("main".to_string(), after.clone())].into();
        let fetched = store
            .fetch_commits(&dir.join("repository.git"), &heads)
            .await;
        ok(fetched, caller, "fetching the commits");
        let merge = Merge {
            before,
            after,
            commits: 2,
            files: vec!["docs/expiry.md".into(), "docs/spec.md".into()],
        };
        (design, dir, pull, merge)
    }
}

/// The merge `task` holds for its pull request `pull`.
#[track_caller]
fn merge_in(task: &Task, pull: Number) -> &Merge {
    let item = task
        .items()
        .iter()
        .find(|item| item.number == pull)
        .unwrap();
    match &item.kind {
        TaskItemKind::Pull(TaskPull {
            state: TaskPullState::Merged { merge, .. },
            ..
        }) => merge,
        kind @ (TaskItemKind::Issue(_) | TaskItemKind::Pull(_)) => {
            panic!("#{} is not merged: {kind:?}", pull.0)
        }
    }
}

#[test_log::test(tokio::test)]
async fn a_pull_request_rebased_onto_the_default_branch_merged_each_of_its_commits() {
    let (design, dir, pull, merge) = task_with_a_rebased_pull().await;

    let task = open(&dir).await;

    assert_eq!(merge_in(&task, pull), &merge);
    let store = seeded(design.world.dir.path(), &task).await;
    let diff = store.diff(pull, &merge.files).await.unwrap();
    assert!(
        diff.contains("+They expire.") && diff.contains("+# Expiry"),
        "{diff}"
    );
}

#[test_log::test(tokio::test)]
async fn a_task_opens_from_a_relative_path() {
    let DesignTask { world, dir, .. } = design_task().await;
    let here = std::env::current_dir().unwrap();
    let up = "../".repeat(here.components().count() - 1);
    let relative = Path::new(&up).join(dir.strip_prefix("/").unwrap());

    let task = open(&relative).await;

    let store = seeded(world.dir.path(), &task).await;
    assert!(store.label_exists(label::READY_FOR_AGENT).await.unwrap());
}
