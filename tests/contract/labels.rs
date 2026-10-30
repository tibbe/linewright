//! The repository's labels.

use anyhow::{Result, ensure};
use linewright::domain::label::RepoLabel;
use linewright::store::NewIssue;

use crate::check::{contracts, equal};
use crate::sandbox::{Sandbox, label};

contracts![
    a_created_label_is_listed_with_its_description_and_exists,
    a_label_created_with_an_empty_description_is_listed_with_it,
    a_label_never_created_does_not_exist,
    a_label_exists_whatever_the_case_it_is_named_in,
    a_label_described_again_is_listed_with_its_new_description,
    creating_a_label_the_repository_has_in_another_case_fails,
    adding_a_label_the_repository_lacks_to_an_issue_creates_it,
    an_issue_filed_with_a_label_the_repository_lacks_creates_it,
];

/// The repository's label `name`, as the store lists it.
async fn listed(sandbox: &Sandbox, name: &str) -> Result<Option<RepoLabel>> {
    let labels = sandbox.store().labels().await?;
    Ok(labels.into_iter().find(|label| label.name == name))
}

async fn a_created_label_is_listed_with_its_description_and_exists(sandbox: Sandbox) -> Result<()> {
    let name = sandbox.new_label("listed", "Listed by the store").await?;

    let expected = RepoLabel {
        name: name.clone(),
        description: Some("Listed by the store".into()),
    };
    equal(
        "the label",
        &listed(&sandbox, &name).await?,
        &Some(expected),
    )?;
    ensure!(
        sandbox.store().label_exists(&name).await?,
        "{name} does not exist"
    );
    Ok(())
}

async fn a_label_created_with_an_empty_description_is_listed_with_it(
    sandbox: Sandbox,
) -> Result<()> {
    let name = sandbox.new_label("bare", "").await?;

    let expected = RepoLabel {
        name: name.clone(),
        description: Some(String::new()),
    };
    equal(
        "the label",
        &listed(&sandbox, &name).await?,
        &Some(expected),
    )
}

async fn a_label_never_created_does_not_exist(sandbox: Sandbox) -> Result<()> {
    let name = sandbox.unique("never");

    ensure!(!sandbox.store().label_exists(&name).await?, "{name} exists");
    equal("the label", &listed(&sandbox, &name).await?, &None)
}

/// GitHub matches label names regardless of case
/// (<https://docs.github.com/en/issues/using-labels-and-milestones-to-track-work/managing-labels>).
async fn a_label_exists_whatever_the_case_it_is_named_in(sandbox: Sandbox) -> Result<()> {
    let name = sandbox.new_label("cased", "Cased").await?;

    let upper = name.to_uppercase();
    ensure!(
        sandbox.store().label_exists(&upper).await?,
        "{upper} does not exist"
    );
    Ok(())
}

async fn a_label_described_again_is_listed_with_its_new_description(
    sandbox: Sandbox,
) -> Result<()> {
    let name = sandbox.new_label("described", "Before").await?;

    sandbox.store().describe_label(&name, "After").await?;

    let expected = RepoLabel {
        name: name.clone(),
        description: Some("After".into()),
    };
    equal(
        "the label",
        &listed(&sandbox, &name).await?,
        &Some(expected),
    )
}

/// GitHub refuses a label whose name another has, regardless of case
/// (<https://docs.github.com/en/rest/issues/labels#create-a-label>).
async fn creating_a_label_the_repository_has_in_another_case_fails(sandbox: Sandbox) -> Result<()> {
    let name = sandbox.new_label("taken", "First").await?;

    let again = sandbox
        .store()
        .create_label(&label(&name.to_uppercase(), "Second"))
        .await;

    ensure!(again.is_err(), "the second label was created");
    let expected = RepoLabel {
        name: name.clone(),
        description: Some("First".into()),
    };
    equal(
        "the label",
        &listed(&sandbox, &name).await?,
        &Some(expected),
    )
}

/// Adding a label by name creates it
/// (<https://docs.github.com/en/rest/issues/labels#add-labels-to-an-issue>).
async fn adding_a_label_the_repository_lacks_to_an_issue_creates_it(
    sandbox: Sandbox,
) -> Result<()> {
    let name = sandbox.unique("added");
    let issue = sandbox
        .store()
        .create_issue(NewIssue {
            title: sandbox.unique("labeled"),
            body: "Labeled with a label the repository lacks.".into(),
            labels: vec![],
        })
        .await?;

    sandbox.store().add_label(issue, &name).await?;

    created_without_a_description(&sandbox, &name).await
}

async fn an_issue_filed_with_a_label_the_repository_lacks_creates_it(
    sandbox: Sandbox,
) -> Result<()> {
    let name = sandbox.unique("filed");

    sandbox
        .store()
        .create_issue(NewIssue {
            title: sandbox.unique("filed-labeled"),
            body: "Filed with a label the repository lacks.".into(),
            labels: vec![name.clone()],
        })
        .await?;

    created_without_a_description(&sandbox, &name).await
}

/// Checks that the label `name` exists, as labeling an item with it
/// creates it: without a description.
async fn created_without_a_description(sandbox: &Sandbox, name: &str) -> Result<()> {
    ensure!(
        sandbox.store().label_exists(name).await?,
        "{name} does not exist"
    );
    let expected = RepoLabel {
        name: name.to_string(),
        description: None,
    };
    equal("the label", &listed(sandbox, name).await?, &Some(expected))
}
