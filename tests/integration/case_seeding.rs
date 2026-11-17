use crate::common;

use std::collections::BTreeSet;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::TimeDelta;

use common::*;
use linewright::benchmarks::Suites;
use linewright::case::{
    self, Case, CaseItemKind, CaseName, CasePart, CasePull, CasePullState, Criteria, Merge,
    SuiteChoice,
};
use linewright::clock::RunningFrom;
use linewright::domain::{MergeMethod, Number, Scope, Windows, label};
use linewright::git::CommitId;
use linewright::input;
use linewright::reconcile::Work;
use linewright::spec_gap;
use linewright::store::local::LocalStore;
use linewright::store::{NewIssue, NewPull, Store};

/// A design run on the first ticket of a spec change cut into tickets,
/// given its parent, its spec change and an open spec gap filed for it,
/// captured as a case.
struct DesignCase {
    world: World,
    dir: PathBuf,
    ticket: Number,
    tracking: Number,
    spec_change: Number,
    gap: Number,
}

#[track_caller]
fn design_case() -> impl Future<Output = DesignCase> {
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
        let name: CaseName = ok("design".parse(), caller, "the name");
        let run = ok(
            format!("{}/design-1", ticket.0).parse(),
            caller,
            "the run's ID",
        );
        let runs = runs_in(&work).await;
        let suites = Suites::new(runs.database().clone());
        let suite = SuiteChoice::New(ok("designs".parse(), caller, "the suite"));
        let capturable = case::capturable(&runs, &suites, &run, &name, &suite).await;
        let capturable = ok(capturable, caller, "the run");
        let criteria = ok(
            Criteria::parse("The design cuts the work in two.\n").ok_or("blank"),
            caller,
            "the criteria",
        );
        let dir = case::capture(&*world.store, capturable, &criteria, &suites).await;
        let dir = ok(dir, caller, "capturing");
        DesignCase {
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
fn open(dir: &Path) -> impl Future<Output = Case> + '_ {
    let caller = Location::caller();
    async move {
        Case::open(dir)
            .await
            .unwrap_or_else(|refusal| panic!("opening the case at {caller}: {refusal}"))
    }
}

/// A store seeded from `case` under `root`, on the clock running from the
/// case's start.
#[track_caller]
fn seeded<'a>(root: &'a Path, case: &'a Case) -> impl Future<Output = LocalStore> + 'a {
    let caller = Location::caller();
    async move {
        LocalStore::from_case(
            &root.join("store"),
            &root.join("repository.git"),
            case,
            Arc::new(RunningFrom::new(case.start())),
        )
        .await
        .unwrap_or_else(|e| panic!("seeding the store at {caller}: {e:#}"))
    }
}

fn numbers(case: &Case) -> BTreeSet<Number> {
    case.items().iter().map(|item| item.number).collect()
}

#[test_log::test(tokio::test)]
async fn the_store_seeded_from_a_case_gives_back_what_the_case_holds() {
    let DesignCase { world, dir, .. } = design_case().await;
    let case = open(&dir).await;
    let store = seeded(world.dir.path(), &case).await;

    let again = world.dir.path().join("again");
    std::fs::create_dir(&again).unwrap();
    for file in ["case.json", "criteria.md"] {
        std::fs::copy(dir.join(file), again.join(file)).unwrap();
    }
    let items = store.record_items(&numbers(&case)).await.unwrap();
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
        .fetch_commits(&again.join("repository.git"), case.heads())
        .await
        .unwrap();
    let reopened = open(&again).await;

    assert_eq!(reopened.items(), case.items());
    assert_eq!(reopened.properties(), case.properties());
    assert_eq!(reopened.heads(), case.heads());
    assert_eq!(
        (reopened.stage(), reopened.item(), reopened.start()),
        (case.stage(), case.item(), case.start())
    );
}

#[test_log::test(tokio::test)]
async fn the_snapshot_of_the_seeded_store_holds_the_cases_item_linked_as_in_the_case() {
    let DesignCase {
        world,
        dir,
        ticket,
        tracking,
        spec_change,
        gap,
    } = design_case().await;
    let case = open(&dir).await;
    let store = seeded(world.dir.path(), &case).await;

    let windows = Windows::at(case.start(), TimeDelta::days(7), TimeDelta::days(7));
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
    assert_eq!(wanted.items, numbers(&case));
    let diff = store
        .diff(spec_change, &["docs/spec.md".to_string()])
        .await
        .unwrap();
    assert!(diff.contains("+Gadgets exist."), "{diff}");
}

#[test_log::test(tokio::test)]
async fn the_seeded_store_numbers_the_next_issue_and_pull_request_above_the_cases_items() {
    let DesignCase { world, dir, .. } = design_case().await;
    let case = open(&dir).await;
    let store = seeded(world.dir.path(), &case).await;
    let highest = numbers(&case).last().copied().unwrap();

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
    let DesignCase { world, dir, .. } = design_case().await;
    let case = open(&dir).await;

    let store = seeded(world.dir.path(), &case).await;

    for label in label::ALL {
        assert!(
            store.label_exists(label.name).await.unwrap(),
            "{}",
            label.name
        );
    }
}

/// A copy of the case in `dir`, at `to`.
#[track_caller]
fn copy_case(dir: &Path, to: &Path) {
    let status = std::process::Command::new("cp")
        .args(["-R"])
        .arg(dir)
        .arg(to)
        .status()
        .unwrap();
    assert!(status.success(), "copying {}", dir.display());
}

/// Sets the field `name` of the case's `case.json` to `value`.
#[track_caller]
fn set_in_case_file(dir: &Path, name: &str, value: serde_json::Value) {
    let path = dir.join("case.json");
    let mut file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file[name] = value;
    std::fs::write(&path, file.to_string()).unwrap();
}

/// What spoils a copy of a case, given its directory.
type Spoil = Box<dyn Fn(&Path)>;

#[test_log::test(tokio::test)]
async fn a_case_lacking_a_part_or_holding_one_that_does_not_read_is_refused_naming_the_part() {
    let DesignCase { world, dir, .. } = design_case().await;
    let remove = |file: &'static str| {
        Box::new(move |case_dir: &Path| {
            let path = case_dir.join(file);
            if path.is_dir() {
                std::fs::remove_dir_all(path).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }) as Spoil
    };
    let write = |file: &'static str, contents: &'static [u8]| {
        Box::new(move |case_dir: &Path| {
            let path = case_dir.join(file);
            if path.is_dir() {
                std::fs::remove_dir_all(&path).unwrap();
            }
            std::fs::write(path, contents).unwrap();
        }) as Spoil
    };
    let set = |name: &'static str, value: serde_json::Value| {
        Box::new(move |case_dir: &Path| set_in_case_file(case_dir, name, value.clone())) as Spoil
    };
    let empty_repository = Box::new(|case_dir: &Path| {
        let path = case_dir.join("repository.git");
        std::fs::remove_dir_all(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
    }) as Spoil;
    let forgotten_history = Box::new(|case_dir: &Path| {
        let repository = case_dir.join("repository.git");
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
    let cases: Vec<(&str, Spoil, CasePart)> = vec![
        (
            "a repository without the spec change's merge",
            forgotten_history,
            CasePart::Repository,
        ),
        ("no case.json", remove("case.json"), CasePart::Stage),
        ("no items.json", remove("items.json"), CasePart::Items),
        (
            "no properties.json",
            remove("properties.json"),
            CasePart::Properties,
        ),
        (
            "no repository.git",
            remove("repository.git"),
            CasePart::Repository,
        ),
        (
            "case.json not JSON",
            write("case.json", b"{"),
            CasePart::Stage,
        ),
        (
            "a rework case",
            set("stage", serde_json::json!("rework")),
            CasePart::Stage,
        ),
        (
            "an item not a number",
            set("item", serde_json::json!("one")),
            CasePart::Item,
        ),
        (
            "a start not a time",
            set("start", serde_json::json!("soon")),
            CasePart::Start,
        ),
        (
            "a run not an ID",
            set("run", serde_json::json!("")),
            CasePart::Run,
        ),
        (
            "an item not among the items",
            set("item", serde_json::json!(999)),
            CasePart::Items,
        ),
        (
            "items.json not JSON",
            write("items.json", b"["),
            CasePart::Items,
        ),
        (
            "an item without its resources",
            write("items.json", br#"[{"number": 1, "resources": {}}]"#),
            CasePart::Items,
        ),
        (
            "properties.json without the properties",
            write("properties.json", br#"{"resources": {}}"#),
            CasePart::Properties,
        ),
        (
            "repository.git not a repository",
            empty_repository,
            CasePart::Repository,
        ),
        ("no criteria.md", remove("criteria.md"), CasePart::Criteria),
        (
            "criteria.md only white space",
            write("criteria.md", b" \n\t\n"),
            CasePart::Criteria,
        ),
        (
            "criteria.md not text",
            write("criteria.md", b"\xff\xfe"),
            CasePart::Criteria,
        ),
    ];
    for (n, (what, spoil, part)) in cases.into_iter().enumerate() {
        let copy = world.dir.path().join(format!("spoiled-{n}"));
        copy_case(&dir, &copy);
        spoil(&copy);

        let refusal = Case::open(&copy).await.err();

        assert_eq!(refusal.map(|r| r.part), Some(part), "{what}");
    }
}

#[test_log::test(tokio::test)]
async fn a_case_opens_with_its_criteria() {
    let DesignCase {
        world: _world, dir, ..
    } = design_case().await;

    let case = open(&dir).await;

    assert_eq!(
        case.criteria().as_str(),
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

/// [`design_case`]'s, with a pull request of two commits then merged by
/// rebase, captured into a case of its own: the case's directory, the pull
/// request, and what its merge did as Linewright reads it.
#[track_caller]
fn case_with_a_rebased_pull() -> impl Future<Output = (DesignCase, PathBuf, Number, Merge)> {
    let caller = Location::caller();
    async move {
        let design = design_case().await;
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
            .and_then(|()| std::fs::copy(design.dir.join("case.json"), dir.join("case.json")))
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

/// The merge `case` holds for its pull request `pull`.
#[track_caller]
fn merge_in(case: &Case, pull: Number) -> &Merge {
    let item = case
        .items()
        .iter()
        .find(|item| item.number == pull)
        .unwrap();
    match &item.kind {
        CaseItemKind::Pull(CasePull {
            state: CasePullState::Merged { merge, .. },
            ..
        }) => merge,
        kind @ (CaseItemKind::Issue(_) | CaseItemKind::Pull(_)) => {
            panic!("#{} is not merged: {kind:?}", pull.0)
        }
    }
}

#[test_log::test(tokio::test)]
async fn a_pull_request_rebased_onto_the_default_branch_merged_each_of_its_commits() {
    let (design, dir, pull, merge) = case_with_a_rebased_pull().await;

    let case = open(&dir).await;

    assert_eq!(merge_in(&case, pull), &merge);
    let store = seeded(design.world.dir.path(), &case).await;
    let diff = store.diff(pull, &merge.files).await.unwrap();
    assert!(
        diff.contains("+They expire.") && diff.contains("+# Expiry"),
        "{diff}"
    );
}

#[test_log::test(tokio::test)]
async fn a_case_opens_from_a_relative_path() {
    let DesignCase { world, dir, .. } = design_case().await;
    let here = std::env::current_dir().unwrap();
    let up = "../".repeat(here.components().count() - 1);
    let relative = Path::new(&up).join(dir.strip_prefix("/").unwrap());

    let case = open(&relative).await;

    let store = seeded(world.dir.path(), &case).await;
    assert!(store.label_exists(label::READY_FOR_AGENT).await.unwrap());
}
