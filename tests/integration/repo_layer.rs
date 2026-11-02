//! Which of the repository's files under `.linewright/` Linewright can use,
//! as `RepoLayer` reads them (docs/specs/repository.md, "Linewright's
//! files").

use crate::common;

use linewright::briefs::OperatorPrompts;
use linewright::prompts::Readers;
use linewright::repo_files::{FileError, RepoLayer, Unusable};
use linewright::store::Blob;

fn holding_every_run(path: &str, error: &str) -> Unusable {
    Unusable {
        file: FileError {
            path: path.into(),
            error: error.into(),
        },
        holds: Readers::Every,
    }
}

#[test_log::test]
fn a_file_that_is_not_one_of_linewrights_holds_every_run() {
    let linewrights_own = ".linewright/prompts/parts/facts.md";
    let where_an_earlier_release_kept_them = ".linewright/briefs/prompts/triage.md";
    let files = [
        ".linewright/prompts/implment.md",
        ".linewright/prompts/parts/x/y.md",
        linewrights_own,
        ".linewright/prompts/outputs/implement.md",
        ".linewright/notes.txt",
        where_an_earlier_release_kept_them,
    ]
    .into_iter()
    .map(|path| (path.to_string(), Blob::Text("Implement it.\n".into())))
    .collect();

    let layer = RepoLayer::parse(files, &OperatorPrompts::read(None).unwrap());

    assert_eq!(
        layer.unusable,
        [
            holding_every_run(
                ".linewright/briefs/prompts/triage.md",
                "not one of Linewright's files"
            ),
            holding_every_run(".linewright/notes.txt", "not one of Linewright's files"),
            holding_every_run(
                ".linewright/prompts/implment.md",
                "not one of Linewright's prompts"
            ),
            holding_every_run(
                ".linewright/prompts/outputs/implement.md",
                "not one of Linewright's prompts"
            ),
            holding_every_run(
                ".linewright/prompts/parts/facts.md",
                "not one of Linewright's prompts"
            ),
            holding_every_run(
                ".linewright/prompts/parts/x/y.md",
                "not one of Linewright's prompts"
            ),
        ]
    );
}

#[test_log::test(tokio::test)]
async fn hidden_files_and_the_automations_are_no_files_of_the_layer() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::store_holding(
        dir.path(),
        &[
            (".linewright/prompts/.editorconfig", "root = true\n"),
            (".linewright/.editorconfig", "root = true\n"),
            (".linewright/.hidden/x", "root = true\n"),
            (".linewright/automations/notes.txt", "Notes.\n"),
        ],
    )
    .await;

    let layer = RepoLayer::read(&store, "main", &OperatorPrompts::read(None).unwrap())
        .await
        .unwrap();

    assert_eq!(layer.unusable, []);
}
