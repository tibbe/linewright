use std::collections::BTreeSet;
use std::path::Path;

use chrono::DateTime;
use linewright::domain::Timestamp;
use linewright::store::Store;
use linewright::store::local::LocalStore;
use serde_json::{Map, Value, json};

use super::files::git_in;

/// Asserts that `front`, left by running a stage, and `back`, written
/// through the back door, hold the same state, field for field: the
/// repository's labels, every issue and pull request as a snapshot holds
/// them, and every branch's commits and files. Timestamps are compared by
/// their order and commits by author, message and tree, since the fake clock
/// ticks on every read and a commit's id depends on its time.
pub async fn assert_same_state(front: &LocalStore, back: &LocalStore) {
    let (front, back) = (state(front).await, state(back).await);
    if let Some(difference) = first_difference("", &front, &back) {
        panic!("the stores differ at {difference}");
    }
}

/// The state `store` holds, each timestamp replaced by its place in the
/// order of them all. The local store numbers its items 1, 2, … in the order
/// it makes them, so an item's number already is its place in that order.
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
    let mut state = json!({
        "labels": labels,
        "issues": issues,
        "pulls": pulls,
        "branches": branches,
    });
    let mut times = BTreeSet::new();
    each_time(&mut state, &mut |_, time| {
        times.insert(time);
    });
    let times: Vec<Timestamp> = times.into_iter().collect();
    each_time(&mut state, &mut |text, time| {
        let place = times.iter().position(|t| *t == time).unwrap();
        *text = format!("time {place} in order");
    });
    state
}

/// Calls `visit` on each RFC 3339 timestamp in `value`, with the time it
/// names.
fn each_time(value: &mut Value, visit: &mut impl FnMut(&mut String, Timestamp)) {
    match value {
        Value::String(text) => {
            if let Ok(time) = DateTime::parse_from_rfc3339(text) {
                visit(text, time.to_utc());
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|v| each_time(v, visit)),
        Value::Object(fields) => fields.values_mut().for_each(|v| each_time(v, visit)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
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
/// with the value each holds there.
fn first_difference(path: &str, front: &Value, back: &Value) -> Option<String> {
    match (front, back) {
        // The fields both hold first, so that two events of different kinds
        // differ at a field they share rather than at one only one has.
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
