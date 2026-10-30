//! Files, as Linewright reads them from the head of a branch.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use linewright::store::Blob;

use crate::check::{contracts, equal};
use crate::sandbox::{NewCommit, PNG, Sandbox};

contracts![
    a_file_is_read_from_the_head_of_its_branch,
    the_files_asked_for_are_read_from_the_head_of_the_branch_leaving_out_paths_with_no_file,
    a_file_with_a_nul_byte_is_read_as_binary,
    a_file_in_latin_1_is_read_as_its_text,
    every_file_under_a_directory_is_listed_by_its_path_under_it,
];

async fn a_file_is_read_from_the_head_of_its_branch(sandbox: Sandbox) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "read",
            &[
                NewCommit {
                    message: "Add a file",
                    files: &[("read/file.txt", b"first\n")],
                },
                NewCommit {
                    message: "Change it",
                    files: &[("read/file.txt", b"second\n")],
                },
            ],
        )
        .await?;
    let store = sandbox.store();

    let read = store.read_file(&pushed.name, "read/file.txt").await?;

    equal("the file", &read.as_deref(), &Some("second\n"))?;
    let missing = store.read_file(&pushed.name, "read/missing.txt").await?;
    equal("a missing file", &missing, &None)?;
    let elsewhere = sandbox.unique("no-such-branch");
    let unbranched = store.read_file(&elsewhere, "read/file.txt").await?;
    equal("a file on a missing branch", &unbranched, &None)
}

async fn the_files_asked_for_are_read_from_the_head_of_the_branch_leaving_out_paths_with_no_file(
    sandbox: Sandbox,
) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "read-files",
            &[NewCommit {
                message: "Configure Linewright",
                files: &[
                    (".linewright/config.toml", b"model = \"opus\"\n"),
                    (".linewright/prompts/triage.md", b"Triage it.\n"),
                    (
                        ".linewright/prompts/follow-ups/result-error.md",
                        b"Parse error.\n",
                    ),
                    ("with space.txt", b"Spaced.\n"),
                    ("na\u{ef}ve.txt", b"Accented.\n"),
                ],
            }],
        )
        .await?;
    sandbox
        .push_branch(
            "read-files-elsewhere",
            &[NewCommit {
                message: "Configure it elsewhere",
                files: &[(".linewright/config.toml", b"effort = \"low\"\n")],
            }],
        )
        .await?;
    let paths = [
        ".linewright/config.toml",
        ".linewright/prompts/follow-ups/result-error.md",
        ".linewright/prompts/design.md",
        ".linewright/prompts/follow-ups",
        "with space.txt",
        "na\u{ef}ve.txt",
    ]
    .map(String::from);

    let files = sandbox.store().read_files(&pushed.name, &paths).await?;

    let text = |path: &str, text: &str| (path.to_string(), Blob::Text(text.into()));
    equal(
        "the files",
        &files,
        &BTreeMap::from([
            text(".linewright/config.toml", "model = \"opus\"\n"),
            text(
                ".linewright/prompts/follow-ups/result-error.md",
                "Parse error.\n",
            ),
            text("with space.txt", "Spaced.\n"),
            text("na\u{ef}ve.txt", "Accented.\n"),
        ]),
    )?;
    let none = sandbox.store().read_files(&pushed.name, &[]).await?;
    equal("the files of no paths", &none, &BTreeMap::new())
}

/// The file at `path` on a branch pushed with it holding `bytes`, as
/// [`Store::read_files`] reads it.
async fn read_back(sandbox: &Sandbox, what: &str, path: &str, bytes: &[u8]) -> Result<Blob> {
    let pushed = sandbox
        .push_branch(
            what,
            &[NewCommit {
                message: "Add a file",
                files: &[(path, bytes)],
            }],
        )
        .await?;
    let mut files = sandbox
        .store()
        .read_files(&pushed.name, &[path.into()])
        .await?;
    files
        .remove(path)
        .with_context(|| format!("{path} is not read"))
}

async fn a_file_with_a_nul_byte_is_read_as_binary(sandbox: Sandbox) -> Result<()> {
    let read = read_back(&sandbox, "binary", "logo.png", PNG).await?;

    equal("the file", &read, &Blob::Binary)
}

async fn a_file_in_latin_1_is_read_as_its_text(sandbox: Sandbox) -> Result<()> {
    let path = ".linewright/prompts/triage.md";

    let read = read_back(&sandbox, "latin-1", path, b"Triage \xff it.\n").await?;

    equal("the file", &read, &Blob::Text("Triage \u{ff} it.\n".into()))
}

async fn every_file_under_a_directory_is_listed_by_its_path_under_it(
    sandbox: Sandbox,
) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "list",
            &[NewCommit {
                message: "Configure Linewright",
                files: &[
                    (".linewright/config.toml", b"model = \"opus\"\n"),
                    (".linewright/prompts/triage.md", b"Triage it.\n"),
                    (
                        ".linewright/prompts/follow-ups/result-error.md",
                        b"Parse error.\n",
                    ),
                ],
            }],
        )
        .await?;
    let store = sandbox.store();

    let mut files = store.list_files(&pushed.name, ".linewright").await?;

    files.sort();
    equal(
        "the files",
        &files[..],
        &[
            "config.toml",
            "prompts/follow-ups/result-error.md",
            "prompts/triage.md",
        ]
        .map(String::from)[..],
    )?;
    let missing = store
        .list_files(&pushed.name, ".linewright/automations")
        .await?;
    equal("the files of a missing directory", &missing, &vec![])?;
    let elsewhere = sandbox.unique("no-such-branch");
    let unbranched = store.list_files(&elsewhere, ".linewright").await?;
    equal("the files on a missing branch", &unbranched, &vec![])
}
