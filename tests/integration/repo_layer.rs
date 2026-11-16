//! Which of the repository's files under `.linewright/` Linewright can use,
//! as `RepoLayer` reads them (docs/specs/repository.md, "Linewright's
//! files").

use crate::common;

use linewright::briefs::OperatorBriefs;
use linewright::layer::Layer;
use linewright::prompts::{Readers, Reads};
use linewright::reconcile::WorkKind;
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
fn a_file_that_is_not_one_of_linewrights_or_a_layer_may_not_hold_holds_every_run() {
    let where_an_earlier_release_kept_them = ".linewright/prompts/implement.md";
    let files = [
        ".linewright/briefs/prompts/implment.md",
        ".linewright/briefs/parts/x/y.md",
        ".linewright/briefs/results/implement.schema.json",
        ".linewright/briefs/README.md",
        ".linewright/notes.txt",
        where_an_earlier_release_kept_them,
    ]
    .into_iter()
    .map(|path| (path.to_string(), Blob::Text("Implement it.\n".into())))
    .collect();

    let layer = RepoLayer::parse(files, &OperatorBriefs::read(None).unwrap());

    assert_eq!(
        layer.unusable,
        [
            holding_every_run(".linewright/briefs/README.md", NOT_HELD),
            holding_every_run(".linewright/briefs/parts/x/y.md", NOT_HELD),
            holding_every_run(".linewright/briefs/prompts/implment.md", NOT_HELD),
            holding_every_run(".linewright/briefs/results/implement.schema.json", NOT_HELD),
            holding_every_run(".linewright/notes.txt", NOT_LINEWRIGHTS),
            holding_every_run(".linewright/prompts/implement.md", NOT_LINEWRIGHTS),
        ]
    );
}

const NOT_HELD: &str = "not a file a layer may hold";
const NOT_LINEWRIGHTS: &str = "not one of Linewright's files";

#[test_log::test]
fn a_file_with_wrong_placeholders_holds_the_runs_it_could_change() {
    let files = [
        (
            ".linewright/briefs/parts/fold-unfinished.md",
            "Fold {{them}}.\n",
        ),
        (
            ".linewright/briefs/plugin/skills/tdd/SKILL.md",
            "Mind {{ticket}}.\n",
        ),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_string(), Blob::Text(text.into())))
    .collect();

    let layer = RepoLayer::parse(files, &OperatorBriefs::read(None).unwrap());

    assert_eq!(
        layer.unusable,
        [
            Unusable {
                file: FileError {
                    path: ".linewright/briefs/parts/fold-unfinished.md".into(),
                    error: "it has unknown placeholders: `{{them}}`".into(),
                },
                holds: Readers::Only(&[
                    Reads::Stage(WorkKind::Implement),
                    Reads::Stage(WorkKind::Rework),
                    Reads::Stage(WorkKind::FixCi),
                    Reads::Stage(WorkKind::Update),
                ]),
            },
            holding_every_run(
                ".linewright/briefs/plugin/skills/tdd/SKILL.md",
                "it has unknown placeholders: `{{ticket}}`"
            ),
        ]
    );
    assert_eq!(
        layer.briefs.layers()["parts/fold-unfinished.md"],
        Layer::Linewright
    );
}

#[test_log::test]
fn the_repositorys_files_come_before_the_operators() {
    let operators = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(operators.path().join("plugin/skills/tdd")).unwrap();
    std::fs::write(
        operators.path().join("plugin/skills/tdd/SKILL.md"),
        "The operator's TDD.\n",
    )
    .unwrap();
    std::fs::create_dir(operators.path().join("parts")).unwrap();
    std::fs::write(
        operators.path().join("parts/no-commits.md"),
        "The operator's words.\n",
    )
    .unwrap();
    let files = [(
        ".linewright/briefs/plugin/skills/tdd/SKILL.md".to_string(),
        Blob::Text("The repository's TDD.\n".into()),
    )]
    .into_iter()
    .collect();

    let layer = RepoLayer::parse(
        files,
        &OperatorBriefs::read(Some(operators.path())).unwrap(),
    );

    assert_eq!(layer.unusable, []);
    let layers = layer.briefs.layers();
    assert_eq!(layers["plugin/skills/tdd/SKILL.md"], Layer::Repository);
    assert_eq!(layers["parts/no-commits.md"], Layer::Operator);
    assert_eq!(layers["prompts/implement.md"], Layer::Linewright);
}

#[test_log::test(tokio::test)]
async fn hidden_files_and_the_automations_are_no_files_of_the_layer() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::store_holding(
        dir.path(),
        &[
            (".linewright/briefs/.editorconfig", "root = true\n"),
            (".linewright/.editorconfig", "root = true\n"),
            (".linewright/.hidden/x", "root = true\n"),
            (".linewright/automations/notes.txt", "Notes.\n"),
        ],
    )
    .await;

    let layer = RepoLayer::read(&store, "main", &OperatorBriefs::read(None).unwrap())
        .await
        .unwrap();

    assert_eq!(layer.unusable, []);
}
