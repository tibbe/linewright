use crate::common;

use common::rejected;
use linewright::domain::MergeMethod::{self, Merge, Rebase, Squash};
use linewright::domain::Repo;
use linewright::store::{Categories, RepoSettings, missing_permission};

/// The settings GitHub gives a repository that allows `allowed`, requiring a
/// linear history or not.
fn read(linear_history: bool, allowed: &[MergeMethod]) -> RepoSettings {
    RepoSettings::new(
        "owner/widgets".parse::<Repo>().unwrap(),
        "main".into(),
        linear_history,
        allowed.iter().copied().collect(),
        Categories::Labels,
    )
}

#[test_log::test]
fn the_default_merge_method_is_the_first_of_merge_rebase_and_squash_the_repository_allows() {
    for (linear_history, allowed, merge_method) in [
        (false, &[Merge, Squash, Rebase][..], Merge),
        (false, &[Merge, Squash], Merge),
        (false, &[Merge, Rebase], Merge),
        (false, &[Merge], Merge),
        (false, &[Squash, Rebase], Rebase),
        (false, &[Rebase], Rebase),
        (false, &[Squash], Squash),
        (true, &[Merge, Squash, Rebase], Rebase),
        (true, &[Merge, Rebase], Rebase),
        (true, &[Merge, Squash], Squash),
        (true, &[Squash, Rebase], Rebase),
        (true, &[Rebase], Rebase),
        (true, &[Squash], Squash),
    ] {
        let case = format!("linear history {linear_history}, allowing {allowed:?}");
        let settings = read(linear_history, allowed).configured(None).expect(&case);

        assert_eq!(settings.merge_method, merge_method, "{case}");
        assert_eq!(settings.linear_history, linear_history, "{case}");
    }
}

#[test_log::test]
fn the_merge_method_setting_wins_over_the_default_where_the_repository_allows_it() {
    for (linear_history, allowed, setting) in [
        (false, &[Merge, Squash, Rebase][..], Squash),
        (false, &[Merge, Squash, Rebase], Rebase),
        (false, &[Squash, Rebase], Squash),
        (true, &[Merge, Squash, Rebase], Squash),
    ] {
        let case = format!("linear history {linear_history}, allowing {allowed:?}, {setting}");
        let settings = read(linear_history, allowed)
            .configured(Some(setting))
            .expect(&case);

        assert_eq!(settings.merge_method, setting, "{case}");
    }
}

#[test_log::test]
fn a_merge_method_the_repository_cannot_take_is_an_error_naming_what_it_allows() {
    for (linear_history, allowed, setting, error) in [
        (
            false,
            &[Merge, Rebase][..],
            Some(Squash),
            "the repository doesn't allow the merge method `squash`: it allows `merge` and \
             `rebase`; set `merge-method` to a method it allows",
        ),
        (
            true,
            &[Merge, Squash],
            Some(Rebase),
            "the repository doesn't allow the merge method `rebase`: it allows only `squash`; \
             set `merge-method` to a method it allows",
        ),
        (
            true,
            &[Merge, Squash, Rebase],
            Some(Merge),
            "the repository requires linear history, so it doesn't allow the merge method \
             `merge`: it allows `rebase` and `squash`; set `merge-method` to a method it allows",
        ),
        (
            true,
            &[Merge, Rebase],
            Some(Merge),
            "the repository requires linear history, so it doesn't allow the merge method \
             `merge`: it allows only `rebase`; set `merge-method` to a method it allows",
        ),
        (
            true,
            &[Merge],
            None,
            "the repository requires linear history and allows only merge commits, so it \
             allows no merge method: allow rebase or squash merging in its settings",
        ),
        (
            false,
            &[],
            None,
            "the repository allows no merge method: allow merge, rebase or squash merging in \
             its settings",
        ),
    ] {
        let configured = read(linear_history, allowed).configured(setting);

        assert_eq!(configured.unwrap_err().to_string(), error);
    }
}

#[test_log::test]
fn a_push_refused_for_a_missing_permission_gives_githubs_reason() {
    const TOKEN_WITHOUT_WORKFLOW: &str = "refusing to allow a Personal Access Token to create \
         or update workflow `.github/workflows/ci.yml` without `workflow` scope";
    const APP_WITHOUT_WORKFLOWS: &str = "refusing to allow a GitHub App to create or update \
         workflow `.github/workflows/ci.yml` without `workflows` permission";
    for (output, reason) in [
        (rejected(TOKEN_WITHOUT_WORKFLOW), TOKEN_WITHOUT_WORKFLOW),
        (rejected(APP_WITHOUT_WORKFLOWS), APP_WITHOUT_WORKFLOWS),
        (
            "remote: Permission to acme/widgets.git denied to linewright-bot.\n\
             fatal: unable to access 'https://github.com/acme/widgets.git/': The requested \
             URL returned error: 403"
                .to_string(),
            "Permission to acme/widgets.git denied to linewright-bot.",
        ),
    ] {
        assert_eq!(
            missing_permission(&output).as_deref(),
            Some(reason),
            "{output}"
        );
    }
}

#[test_log::test]
fn a_push_refused_for_another_reason_names_no_missing_permission() {
    for output in [
        rejected("pre-receive hook declined"),
        "To https://github.com/acme/widgets.git\n ! [rejected] HEAD -> branch (stale info)\n\
         error: failed to push some refs to 'https://github.com/acme/widgets.git'"
            .to_string(),
        "fatal: unable to access 'https://github.com/acme/widgets.git/': Could not resolve \
         host: github.com"
            .to_string(),
    ] {
        assert_eq!(missing_permission(&output), None, "{output}");
    }
}
