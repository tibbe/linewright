//! A task's items, read from GitHub's REST resources as a task keeps them
//! (`docs/specs/benchmarks.md`, "Tasks").

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::domain::{Comment, IssueCloseReason, Login, Number, Timestamp};
use crate::git::CommitId;
use crate::store::{RecordedItem, Resource};

/// An issue or pull request of a task, as far as a store's record of it
/// needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskItem {
    pub number: Number,
    pub title: String,
    pub body: String,
    pub author: Login,
    pub labels: Vec<String>,
    /// Its issue type, by name.
    pub issue_type: Option<String>,
    pub assignees: Vec<Login>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Oldest first.
    pub comments: Vec<Comment>,
    /// Its parent issue, when the task holds it.
    pub parent: Option<Number>,
    /// The issues it is blocked by that the task holds.
    pub blocked_by: Vec<Number>,
    pub kind: TaskItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskItemKind {
    Issue(IssueState),
    Pull(TaskPull),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed {
        at: Timestamp,
        reason: IssueCloseReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskPull {
    /// The branch it merges into.
    pub base: String,
    /// Its branch.
    pub head: String,
    pub state: TaskPullState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskPullState {
    Open { draft: bool },
    Closed { at: Timestamp },
    Merged { at: Timestamp, merge: Merge },
}

/// What a pull request's merge did to the branch merged into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    /// The branch's head before the merge.
    pub before: CommitId,
    /// The branch's head after the merge.
    pub after: CommitId,
    /// How many commits the pull request held, as GitHub counts them.
    pub commits: u64,
    /// The paths of the files the merge changed, sorted.
    pub files: Vec<String>,
}

/// The commit GitHub names as a pull request's merge, with how many commits
/// the pull request held
/// (<https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request>,
/// `merge_commit_sha` and `commits`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeCommit {
    pub commit: CommitId,
    pub commits: u64,
}

#[derive(Deserialize)]
struct RestIssue {
    title: String,
    body: Option<String>,
    user: RestUser,
    labels: Vec<RestLabel>,
    state: String,
    state_reason: Option<String>,
    assignees: Vec<RestUser>,
    created_at: Timestamp,
    updated_at: Timestamp,
    closed_at: Option<Timestamp>,
    #[serde(rename = "type")]
    issue_type: Option<RestIssueType>,
}

#[derive(Deserialize)]
struct RestUser {
    login: String,
}

#[derive(Deserialize)]
struct RestLabel {
    name: String,
}

#[derive(Deserialize)]
struct RestIssueType {
    name: String,
}

#[derive(Deserialize)]
struct RestComment {
    body: Option<String>,
    user: RestUser,
    created_at: Timestamp,
}

#[derive(Deserialize)]
struct RestNumbered {
    number: Number,
}

#[derive(Deserialize)]
struct RestPull {
    state: String,
    draft: Option<bool>,
    closed_at: Option<Timestamp>,
    merged_at: Option<Timestamp>,
    merge_commit_sha: Option<String>,
    commits: Option<u64>,
    base: RestRef,
    head: RestRef,
}

#[derive(Deserialize)]
struct RestRef {
    #[serde(rename = "ref")]
    name: String,
}

/// `item`'s merge commit, if it is a merged pull request, or why its
/// resources do not read.
pub fn merge_commit(item: &RecordedItem) -> Result<Option<MergeCommit>, String> {
    if item.resource(Resource::Pull).is_none() {
        return Ok(None);
    }
    let pull: RestPull = resource(item, Resource::Pull)?;
    if pull.merged_at.is_none() {
        return Ok(None);
    }
    let commit = pull
        .merge_commit_sha
        .ok_or_else(|| format!("#{} merged without a merge commit", item.number))?;
    if !is_object_name(&commit) {
        return Err(format!(
            "#{}'s merge commit is not a commit's name: {commit:?}",
            item.number
        ));
    }
    let commits = pull
        .commits
        .ok_or_else(|| format!("#{} gives no count of its commits", item.number))?;
    Ok(Some(MergeCommit {
        commit: CommitId::from(commit.as_str()),
        commits,
    }))
}

/// Whether `name` is a full object name, in hex, as git's SHA-1 and SHA-256
/// give it (<https://git-scm.com/docs/hash-function-transition>).
fn is_object_name(name: &str) -> bool {
    matches!(name.len(), 40 | 64) && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `item` read from its resources, keeping only the links to `held`, the
/// items of its task, with what its merge did from `merges`, or why it does
/// not read.
pub fn read(
    item: &RecordedItem,
    held: &BTreeSet<Number>,
    merges: &BTreeMap<Number, Merge>,
) -> Result<TaskItem, String> {
    let number = item.number;
    let issue: RestIssue = resource(item, Resource::Issue)?;
    let comments: Vec<RestComment> = resource(item, Resource::Comments)?;
    let blocked_by: Vec<RestNumbered> = resource(item, Resource::BlockedBy)?;
    let parent = match item.resource(Resource::Parent) {
        Some(_) => Some(resource::<RestNumbered>(item, Resource::Parent)?.number),
        None => None,
    };
    let kind = match item.resource(Resource::Pull) {
        Some(_) => {
            let pull: RestPull = resource(item, Resource::Pull)?;
            let state = match (pull.merged_at, pull.state.as_str()) {
                (Some(at), _) => TaskPullState::Merged {
                    at,
                    merge: merges
                        .get(&number)
                        .cloned()
                        .ok_or_else(|| format!("#{number}'s merge is not read"))?,
                },
                (None, "open") => TaskPullState::Open {
                    draft: pull.draft.unwrap_or(false),
                },
                (None, "closed") => TaskPullState::Closed {
                    at: pull
                        .closed_at
                        .ok_or_else(|| format!("#{number} is closed with no time"))?,
                },
                (None, state) => return Err(format!("#{number} is in no known state: {state}")),
            };
            TaskItemKind::Pull(TaskPull {
                base: pull.base.name,
                head: pull.head.name,
                state,
            })
        }
        None => TaskItemKind::Issue(issue_state(number, &issue)?),
    };
    Ok(TaskItem {
        number,
        title: issue.title,
        body: issue.body.unwrap_or_default(),
        author: Login::new(issue.user.login),
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        issue_type: issue.issue_type.map(|issue_type| issue_type.name),
        assignees: issue
            .assignees
            .into_iter()
            .map(|user| Login::new(user.login))
            .collect(),
        created_at: issue.created_at,
        updated_at: issue.updated_at,
        comments: comments
            .into_iter()
            .map(|comment| Comment {
                author: Login::new(comment.user.login),
                body: comment.body.unwrap_or_default(),
                created_at: comment.created_at,
            })
            .collect(),
        parent: parent.filter(|parent| held.contains(parent)),
        blocked_by: blocked_by
            .into_iter()
            .map(|blocker| blocker.number)
            .filter(|blocker| held.contains(blocker))
            .collect(),
        kind,
    })
}

/// An issue's state as its resource gives it, closed as completed where
/// GitHub gives no reason, as it closes by default
/// (<https://docs.github.com/en/rest/issues/issues#update-an-issue>).
fn issue_state(number: Number, issue: &RestIssue) -> Result<IssueState, String> {
    match issue.state.as_str() {
        "open" => Ok(IssueState::Open),
        "closed" => Ok(IssueState::Closed {
            at: issue
                .closed_at
                .ok_or_else(|| format!("#{number} is closed with no time"))?,
            reason: match issue.state_reason.as_deref() {
                None | Some("completed") => IssueCloseReason::Completed,
                Some("not_planned") => IssueCloseReason::NotPlanned,
                Some("duplicate") => IssueCloseReason::Duplicate,
                Some(reason) => {
                    return Err(format!("#{number} is closed for no known reason: {reason}"));
                }
            },
        }),
        state => Err(format!("#{number} is in no known state: {state}")),
    }
}

/// The title of `item`, as its issue gives it, or why it lacks it or it
/// does not read.
pub fn title(item: &RecordedItem) -> Result<String, String> {
    resource::<RestIssue>(item, Resource::Issue).map(|issue| issue.title)
}

/// `item`'s `resource`, or why it lacks it or it does not read.
fn resource<T: DeserializeOwned>(item: &RecordedItem, resource: Resource) -> Result<T, String> {
    let path = resource.path(item.number);
    let value: &Value = item
        .resource(resource)
        .ok_or_else(|| format!("#{} lacks {path}", item.number))?;
    T::deserialize(value).map_err(|e| format!("#{}'s {path} does not read: {e}", item.number))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// #`number` of tibbe/linewright as GitHub gave it on 2026-11-12, with
    /// each of its `resources` by its path and fixture.
    fn recorded(number: u64, resources: &[(Resource, &str)]) -> RecordedItem {
        let number = Number(number);
        RecordedItem {
            number,
            resources: resources
                .iter()
                .map(|(resource, text)| {
                    (resource.path(number), serde_json::from_str(text).unwrap())
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn every(numbers: &[u64]) -> BTreeSet<Number> {
        numbers.iter().copied().map(Number).collect()
    }

    #[test_log::test]
    fn an_open_issue_reads_with_its_parent_and_blockers_the_task_holds() {
        let item = recorded(
            1158,
            &[
                (
                    Resource::Issue,
                    include_str!("../store/fixtures/rest-issue-1158.json"),
                ),
                (
                    Resource::Comments,
                    include_str!("../store/fixtures/rest-issue-1158-comments.json"),
                ),
                (
                    Resource::Parent,
                    include_str!("../store/fixtures/rest-issue-1158-parent.json"),
                ),
                (
                    Resource::BlockedBy,
                    include_str!("../store/fixtures/rest-issue-1158-blocked-by.json"),
                ),
            ],
        );

        let read = read(&item, &every(&[1156, 1157, 1158]), &BTreeMap::new()).unwrap();

        assert_eq!(read.number, Number(1158));
        assert_eq!(read.title, "Record a stage run's input at its start");
        assert!(read.body.starts_with("## What to build\n"), "{}", read.body);
        assert_eq!(read.author, Login::new("tibbe-bot"));
        assert_eq!(read.labels, ["ready-for-agent", "linewright:designed"]);
        assert_eq!(read.issue_type, None);
        assert_eq!(read.assignees, [Login::new("tibbe-bot")]);
        assert_eq!(
            (read.created_at, read.updated_at),
            (at("2026-11-12T10:22:18Z"), at("2026-11-12T11:03:29Z"))
        );
        assert_eq!(read.comments, []);
        assert_eq!(read.parent, Some(Number(1156)));
        assert_eq!(read.blocked_by, [Number(1157)]);
        assert_eq!(read.kind, TaskItemKind::Issue(IssueState::Open));

        let alone = super::read(&item, &every(&[1158]), &BTreeMap::new()).unwrap();
        assert_eq!((alone.parent, alone.blocked_by), (None, vec![]));
    }

    #[test_log::test]
    fn a_closed_issue_reads_with_its_closing_and_comments() {
        let item = recorded(
            1130,
            &[
                (
                    Resource::Issue,
                    include_str!("../store/fixtures/rest-issue-1130.json"),
                ),
                (
                    Resource::Comments,
                    include_str!("../store/fixtures/rest-issue-1130-comments.json"),
                ),
                (
                    Resource::BlockedBy,
                    include_str!("../store/fixtures/rest-issue-1130-blocked-by.json"),
                ),
            ],
        );

        let read = read(&item, &every(&[1127, 1130]), &BTreeMap::new()).unwrap();

        assert_eq!(
            read.kind,
            TaskItemKind::Issue(IssueState::Closed {
                at: at("2026-11-10T21:25:34Z"),
                reason: IssueCloseReason::Completed,
            })
        );
        let comments: Vec<(&str, Timestamp)> = read
            .comments
            .iter()
            .map(|comment| (comment.author.as_str(), comment.created_at))
            .collect();
        assert_eq!(
            comments,
            [
                ("tibbe-bot", at("2026-11-10T20:41:48Z")),
                ("tibbe-bot", at("2026-11-10T20:42:19Z")),
                ("tibbe-bot", at("2026-11-10T21:25:33Z")),
            ]
        );
        assert!(read.comments[0].body.starts_with("## Prior art\n"));
        assert_eq!(read.blocked_by, [Number(1127)]);
    }

    #[test_log::test]
    fn a_merged_pull_request_reads_with_its_branches_and_merge() {
        let item = recorded(
            1155,
            &[
                (
                    Resource::Issue,
                    include_str!("../store/fixtures/rest-issue-1155.json"),
                ),
                (
                    Resource::Comments,
                    include_str!("../store/fixtures/rest-issue-1155-comments.json"),
                ),
                (
                    Resource::BlockedBy,
                    include_str!("../store/fixtures/rest-issue-1155-blocked-by.json"),
                ),
                (
                    Resource::Pull,
                    include_str!("../store/fixtures/rest-pull-1155.json"),
                ),
            ],
        );
        let merge = Merge {
            before: CommitId::from("bc2a4a2b3f7ebe9e4d50e4b7c3d9b9493f448b54"),
            after: CommitId::from("c4a839801e4222c8550326de9fe095e794a53506"),
            commits: 1,
            files: vec!["docs/specs/benchmarks.md".into()],
        };

        assert_eq!(
            merge_commit(&item),
            Ok(Some(MergeCommit {
                commit: CommitId::from("c4a839801e4222c8550326de9fe095e794a53506"),
                commits: 1,
            }))
        );
        let merges = BTreeMap::from([(Number(1155), merge.clone())]);
        let read = read(&item, &every(&[1155]), &merges).unwrap();

        assert_eq!(read.title, "Capture a stage's run as a case");
        assert_eq!(read.author, Login::new("tibbe"));
        assert_eq!(
            read.kind,
            TaskItemKind::Pull(TaskPull {
                base: "main".into(),
                head: "benchmarks".into(),
                state: TaskPullState::Merged {
                    at: at("2026-11-12T10:13:19Z"),
                    merge,
                },
            })
        );
    }

    #[test_log::test]
    fn a_merge_commit_that_is_not_a_commits_name_does_not_read() {
        let mut pull: Value =
            serde_json::from_str(include_str!("../store/fixtures/rest-pull-1155.json")).unwrap();
        pull["merge_commit_sha"] = Value::from("--output=/tmp/x");
        let item = RecordedItem {
            number: Number(1155),
            resources: BTreeMap::from([(Resource::Pull.path(Number(1155)), pull)]),
        };

        assert!(merge_commit(&item).is_err());
    }
}
