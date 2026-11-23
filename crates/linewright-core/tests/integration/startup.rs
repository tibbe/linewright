use linewright_core::domain::MergeMethod;
use linewright_core::testing::*;

#[test_log::test(tokio::test)]
async fn a_merge_method_the_repository_does_not_allow_stops_startup_naming_those_it_does() {
    let without_auto_merge = World::start(Repository::allowing([MergeMethod::Rebase]), |config| {
        config.merge_method = Some(MergeMethod::Merge);
    })
    .await;
    let linear_history = Repository {
        linear_history: true,
        ..Repository::allowing([MergeMethod::Merge, MergeMethod::Squash])
    };
    let merge_with_linear_history = World::start(linear_history, |config| {
        config.auto_merge = AUTO_MERGE;
        config.merge_method = Some(MergeMethod::Merge);
    })
    .await;

    for (started, allowed) in [
        (without_auto_merge, "only `rebase`"),
        (merge_with_linear_history, "only `squash`"),
    ] {
        let error = format!("{:#}", started.err().expect("startup went ahead"));
        assert!(error.contains(allowed), "{error}");
    }
}

#[test_log::test(tokio::test)]
async fn the_merge_method_in_effect_is_logged() {
    let (log, _guard) = Log::capture();
    World::start(Repository::default(), |_| {}).await.unwrap();

    let started: Vec<String> = log
        .lines("INFO")
        .into_iter()
        .filter(|line| line.contains("started"))
        .collect();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(started[0].contains("merge_method=merge"), "{}", started[0]);
}
