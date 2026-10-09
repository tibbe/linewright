//! Startup, end to end at the operator's level: the repository's settings
//! and Linewright's in, a factory or an error out, against the local store
//! (`docs/specs/product.md`, "Repository"). `serve`, `once` and
//! `workflow run` all start this way.

mod common;

use common::*;
use linewright::domain::MergeMethod;

/// The error startup fails with.
async fn refused(repository: Repository, merge_method: Option<MergeMethod>) -> String {
    let started = World::start(repository, |config| {
        config.auto_merge = true;
        config.merge_method = merge_method;
    })
    .await;
    match started {
        Ok(_) => panic!("startup went ahead"),
        Err(e) => format!("{e:#}"),
    }
}

#[tokio::test]
async fn with_auto_merge_a_merge_method_the_repository_disallows_is_an_error_naming_what_it_allows()
{
    let repository = Repository {
        allowed_merge_methods: [MergeMethod::Merge, MergeMethod::Rebase]
            .into_iter()
            .collect(),
        ..Repository::default()
    };

    let error = refused(repository, Some(MergeMethod::Squash)).await;

    assert!(error.contains("`squash`"), "{error}");
    assert!(error.contains("`merge` and `rebase`"), "{error}");
}

#[tokio::test]
async fn with_auto_merge_a_default_merge_method_the_repository_disallows_is_an_error() {
    let repository = Repository {
        linear_history: false,
        allowed_merge_methods: [MergeMethod::Squash].into_iter().collect(),
    };

    let error = refused(repository, None).await;

    assert!(error.contains("`merge`"), "{error}");
    assert!(error.contains("allows only `squash`"), "{error}");
}

#[tokio::test]
async fn with_auto_merge_and_linear_history_a_default_rebase_the_repository_disallows_is_an_error()
{
    let repository = Repository {
        linear_history: true,
        allowed_merge_methods: [MergeMethod::Merge, MergeMethod::Squash]
            .into_iter()
            .collect(),
    };

    let error = refused(repository, None).await;

    assert!(error.contains("`rebase`"), "{error}");
    assert!(error.contains("`merge` and `squash`"), "{error}");
}

#[tokio::test]
async fn with_auto_merge_an_allowed_merge_method_starts_and_is_logged() {
    let (log, _guard) = Log::capture();
    let repository = Repository {
        allowed_merge_methods: [MergeMethod::Squash].into_iter().collect(),
        ..Repository::default()
    };

    World::start(repository, |config| {
        config.auto_merge = true;
        config.merge_method = Some(MergeMethod::Squash);
    })
    .await
    .unwrap();

    let started: Vec<String> = log
        .lines("INFO")
        .into_iter()
        .filter(|line| line.contains("started"))
        .collect();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(started[0].contains("merge_method=squash"), "{}", started[0]);
}

#[tokio::test]
async fn without_auto_merge_the_merge_method_is_not_checked() {
    let repository = Repository {
        allowed_merge_methods: [MergeMethod::Squash].into_iter().collect(),
        ..Repository::default()
    };

    let started = World::start(repository, |config| {
        config.merge_method = Some(MergeMethod::Rebase);
    })
    .await;

    assert!(started.is_ok());
}
