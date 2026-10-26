use crate::common;

use common::assert_in_a_process_group_of_its_own;
use linewright::git;

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
