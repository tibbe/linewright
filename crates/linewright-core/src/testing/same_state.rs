use std::collections::BTreeSet;
use std::path::Path;

use chrono::DateTime;
use serde_json::{Map, Value, json};
use sqlx::{Column as _, Row as _, TypeInfo as _, ValueRef as _};

use super::World;
use super::files::git_in;
use super::records::{back_door, runs_in};
use crate::domain::Timestamp;
use crate::runs::RunId;
use crate::store::Store;
use crate::store::local::LocalStore;

/// Asserts that `front`, left by running a stage, and `back`, written
/// through the back door, hold the same state, field for field: the
/// repository's labels, every issue and pull request as a snapshot holds
/// them, and every branch's commits and files. Timestamps are compared by
/// their order and commits by author, message and tree.
pub async fn assert_same_state(front: &LocalStore, back: &LocalStore) {
    let (front, back) = (state(front).await, state(back).await);
    if let Some(difference) = first_difference("", &front, &back) {
        panic!("the stores differ at {difference}");
    }
}

/// The state `store` holds, each timestamp replaced by its place in the
/// order of them all.
async fn state(store: &LocalStore) -> Value {
    let repo = store.repository();
    let labels: Map<String, Value> = Store::labels(store)
        .await
        .unwrap()
        .into_iter()
        .map(|label| (label.name, json!(label.description)))
        .collect();
    let issues: Map<String, Value> = store
        .issues()
        .into_iter()
        .map(|issue| {
            (
                issue.number.to_string(),
                serde_json::to_value(issue).unwrap(),
            )
        })
        .collect();
    let mut pulls = Map::new();
    for number in store.pulls() {
        let mut pull = serde_json::to_value(store.snapshot_pull(number).await).unwrap();
        let head = pull["head_sha"].as_str().unwrap().to_string();
        pull["head_sha"] = if head.is_empty() {
            Value::Null
        } else {
            commit(&repo, &head)
        };
        pulls.insert(number.to_string(), pull);
    }
    let branches: Map<String, Value> = git_in(
        &repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .lines()
    .map(|branch| (branch.to_string(), branch_state(&repo, branch)))
    .collect();
    in_time_order(
        json!({
            "labels": labels,
            "issues": issues,
            "pulls": pulls,
            "branches": branches,
        }),
        Stamps::Rfc3339,
    )
}

/// Asserts that the rows `query` gives in the database of the work
/// directory `front`, left by running a stage, are those it gives in that of
/// `back`, written through the back door, in its order, field for field.
/// Timestamps, also those that start a part of a path, as a session's
/// directory's name does, are compared by their order.
pub async fn assert_same_rows(front: &Path, back: &Path, query: &str) {
    let front = rows(&back_door(front).await, query).await;
    let back = rows(&back_door(back).await, query).await;
    if let Some(difference) = first_difference("", &front, &back) {
        panic!("the rows of {query} differ at {difference}");
    }
}

/// The rows `query` gives in `pool`, each an object of its columns by name,
/// each timestamp replaced by its place in the order of them all.
async fn rows(pool: &sqlx::SqlitePool, query: &str) -> Value {
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .fetch_all(pool)
        .await
        .unwrap_or_else(|e| panic!("querying {query}: {e}"));
    pool.close().await;
    let rows: Vec<Value> = rows
        .iter()
        .map(|row| {
            let columns: Map<String, Value> = row
                .columns()
                .iter()
                .map(|column| {
                    let i = column.ordinal();
                    let raw = row.try_get_raw(i).unwrap();
                    let value = if raw.is_null() {
                        Value::Null
                    } else {
                        match raw.type_info().name() {
                            "INTEGER" => json!(row.get::<i64, _>(i)),
                            "REAL" => json!(row.get::<f64, _>(i)),
                            "BLOB" => json!(row.get::<Vec<u8>, _>(i)),
                            _ => json!(row.get::<String, _>(i)),
                        }
                    };
                    (column.name().to_string(), value)
                })
                .collect();
            Value::Object(columns)
        })
        .collect();
    in_time_order(Value::Array(rows), Stamps::AlsoInPaths)
}

/// Asserts that the input the run `run` recorded in `front`, by running its
/// stage, is what `back` holds for it, written through the back door, field
/// for field. Timestamps are compared by their order and each head by its
/// commit's author, message and tree.
pub async fn assert_same_input(front: &World, back: &World, run: &str) {
    let id: RunId = run.parse().unwrap_or_else(|e| panic!("the run {run}: {e}"));
    let front_input = input(front, &id).await;
    let back_input = input(back, &id).await;
    if let Some(difference) = first_difference("", &front_input, &back_input) {
        panic!("the inputs of {id} differ at {difference}");
    }
}

/// The input `world` holds for the run `id`, each head as its commit, each
/// timestamp replaced by its place in the order of them all.
async fn input(world: &World, id: &RunId) -> Value {
    let runs = runs_in(&world.work_dir).await;
    let input = runs
        .read_input(id)
        .await
        .unwrap_or_else(|e| panic!("reading the input of {id}: {e:#}"))
        .unwrap_or_else(|| panic!("the run {id} recorded no input"));
    let repo = world.store.repository();
    let mut input = serde_json::to_value(&input).unwrap();
    for head in input["heads"].as_object_mut().unwrap().values_mut() {
        *head = commit(&repo, head.as_str().unwrap());
    }
    in_time_order(input, Stamps::Rfc3339)
}

/// Asserts that the directory `front`, left by running a stage, holds what
/// `back`, written through the back door, holds: the same directories,
/// links and files, each file with the same contents. A file of JSON is
/// compared field for field. A git repository under them, such as a
/// task's `repository.git`, is compared by its refs, each by its commit's
/// author, message and tree. Timestamps, also those that start a part of a
/// path, are compared by their order.
pub fn assert_same_tree(front: &Path, back: &Path) {
    let (front, back) = (tree(front), tree(back));
    if let Some(difference) = first_difference("", &front, &back) {
        panic!("the trees differ at {difference}");
    }
}

/// What `dir` holds, each entry by its path under it, timestamps in paths
/// and in contents replaced by their place in the order of them all.
fn tree(dir: &Path) -> Value {
    let mut held = vec![];
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in
            std::fs::read_dir(&at).unwrap_or_else(|e| panic!("reading {}: {e}", at.display()))
        {
            let path = entry.unwrap().path();
            let kind = std::fs::symlink_metadata(&path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
                .file_type();
            let contents = if kind.is_symlink() {
                json!({ "link to": std::fs::read_link(&path).unwrap().display().to_string() })
            } else if is_repository(&path) {
                refs(&path)
            } else if kind.is_dir() {
                pending.push(path.clone());
                json!("a directory")
            } else {
                let bytes = std::fs::read(&path)
                    .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
                serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                    json!(String::from_utf8(bytes.clone()).unwrap_or_else(|_| format!("{bytes:?}")))
                })
            };
            let name = path.strip_prefix(dir).unwrap().display().to_string();
            held.push(json!([name, contents]));
        }
    }
    let Value::Array(held) = in_time_order(Value::Array(held), Stamps::AlsoInPaths) else {
        unreachable!("an array stays an array");
    };
    Value::Object(
        held.into_iter()
            .map(|entry| {
                let [name, contents] =
                    <[Value; 2]>::try_from(entry.as_array().unwrap().clone()).unwrap();
                (name.as_str().unwrap().to_string(), contents)
            })
            .collect(),
    )
}

/// Whether `dir` is a bare git repository.
fn is_repository(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// The refs of the repository `repo`, each by its commit.
fn refs(repo: &Path) -> Value {
    let refs: Map<String, Value> =
        git_in(repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
            .lines()
            .map(|line| {
                let (name, id) = line.split_once(' ').unwrap();
                (name.to_string(), commit(repo, id))
            })
            .collect();
    json!({ "refs": refs })
}

/// Which strings [`in_time_order`] takes timestamps from.
#[derive(Clone, Copy)]
enum Stamps {
    /// Each that is an RFC 3339 timestamp.
    Rfc3339,
    /// Those, and each part of a path that starts with a timestamp in the
    /// form a session's directory is named in, such as
    /// `20261001T120007Z-implement`.
    AlsoInPaths,
}

/// `state`, each timestamp `stamps` names replaced by its place in the
/// order of them all.
fn in_time_order(mut state: Value, stamps: Stamps) -> Value {
    let mut times = BTreeSet::new();
    each_time(&mut state.clone(), stamps, &mut |time| {
        times.insert(time);
        String::new()
    });
    let times: Vec<Timestamp> = times.into_iter().collect();
    each_time(&mut state, stamps, &mut |time| {
        let place = times.iter().position(|t| *t == time).unwrap();
        format!("time {place} in order")
    });
    state
}

/// Replaces each timestamp `stamps` names in `value` with what `visit`
/// gives for the time it names.
fn each_time(value: &mut Value, stamps: Stamps, visit: &mut impl FnMut(Timestamp) -> String) {
    match value {
        Value::String(text) => {
            if let Ok(time) = DateTime::parse_from_rfc3339(text) {
                *text = visit(time.to_utc());
            } else if let Stamps::AlsoInPaths = stamps {
                *text = text
                    .split('/')
                    .map(|part| match compact_time(part) {
                        Some((time, rest)) => format!("{}{rest}", visit(time)),
                        None => part.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("/");
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|v| each_time(v, stamps, visit)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|v| each_time(v, stamps, visit)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// The time `part` starts with, written as `%Y%m%dT%H%M%SZ`, and what
/// follows it.
fn compact_time(part: &str) -> Option<(Timestamp, &str)> {
    const LEN: usize = "20261001T120007Z".len();
    let (time, rest) = (part.get(..LEN)?, part.get(LEN..)?);
    let time = chrono::NaiveDateTime::parse_from_str(time, "%Y%m%dT%H%M%SZ").ok()?;
    Some((time.and_utc(), rest))
}

/// A branch's commits, newest first, and its files.
fn branch_state(repo: &Path, branch: &str) -> Value {
    let commits: Vec<Value> = git_in(repo, &["rev-list", branch])
        .lines()
        .map(|id| commit(repo, id))
        .collect();
    let files: Map<String, Value> = git_in(repo, &["ls-tree", "-r", "-z", "--name-only", branch])
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| {
            let contents = git_in(repo, &["cat-file", "blob", &format!("{branch}:{path}")]);
            (path.to_string(), json!(contents))
        })
        .collect();
    json!({ "commits": commits, "files": files })
}

/// The commit `id`, by its author, message and tree.
fn commit(repo: &Path, id: &str) -> Value {
    let author = git_in(repo, &["log", "-1", "--format=%an <%ae>", id]);
    let message = git_in(repo, &["log", "-1", "--format=%B", id]);
    let tree = git_in(repo, &["rev-parse", &format!("{id}^{{tree}}")]);
    json!({
        "author": author.trim_end(),
        "message": message.trim_end(),
        "tree": tree.trim_end(),
    })
}

/// The first field at or under `path` where `front` and `back` differ,
/// with the value each holds there. Two objects differ at a field both hold
/// before they differ at one only one holds.
fn first_difference(path: &str, front: &Value, back: &Value) -> Option<String> {
    match (front, back) {
        (Value::Object(front), Value::Object(back)) => {
            let every = front.keys().chain(back.keys()).collect::<BTreeSet<_>>();
            front
                .iter()
                .find_map(|(key, value)| {
                    let path = format!("{path}.{key}");
                    first_difference(&path, value, back.get(key)?)
                })
                .or_else(|| {
                    every.into_iter().find_map(|key| {
                        field_difference(&format!("{path}.{key}"), front.get(key), back.get(key))
                    })
                })
        }
        (Value::Array(front), Value::Array(back)) => (0..front.len().max(back.len()))
            .find_map(|i| field_difference(&format!("{path}[{i}]"), front.get(i), back.get(i))),
        _ if front == back => None,
        _ => Some(format!(
            "{path}: the front door left {front}, the back door {back}"
        )),
    }
}

/// [`first_difference`]'s, at a field or element only `front` or only
/// `back` may hold.
fn field_difference(path: &str, front: Option<&Value>, back: Option<&Value>) -> Option<String> {
    match (front, back) {
        (Some(front), Some(back)) => first_difference(path, front, back),
        (Some(front), None) => Some(format!(
            "{path}: the front door left {front}, the back door nothing"
        )),
        (None, Some(back)) => Some(format!(
            "{path}: the front door left nothing, the back door {back}"
        )),
        (None, None) => None,
    }
}
