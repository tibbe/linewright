use crate::common;

use common::rejected;
use linewright::domain::MergeMethod::{self, Merge, Rebase, Squash};
use linewright::domain::{AutoMerge, Repo, UpdateMethod, UpdateRule};
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

#[test]
fn the_repositorys_settings_decide_how_a_branch_is_updated_and_merged() {
    for (linear_history, allowed, update_rule, merge_method) in [
        (
            false,
            &[Merge, Squash, Rebase][..],
            UpdateRule::RebaseOnConflict,
            Merge,
        ),
        (
            false,
            &[Merge, Squash],
            UpdateRule::Always(UpdateMethod::Merge),
            Merge,
        ),
        (
            true,
            &[Merge, Squash, Rebase],
            UpdateRule::Always(UpdateMethod::Rebase),
            Rebase,
        ),
        (
            true,
            &[Squash],
            UpdateRule::Always(UpdateMethod::Rebase),
            Rebase,
        ),
        (false, &[Rebase], UpdateRule::RebaseOnConflict, Rebase),
        (
            false,
            &[Squash, Rebase],
            UpdateRule::RebaseOnConflict,
            Squash,
        ),
        (
            false,
            &[Squash],
            UpdateRule::Always(UpdateMethod::Merge),
            Squash,
        ),
    ] {
        let settings = read(linear_history, allowed);

        let case = format!("linear history {linear_history}, allowing {allowed:?}");
        assert_eq!(settings.update_rule, update_rule, "{case}");
        assert_eq!(settings.merge_method, merge_method, "{case}");
    }
}

#[test]
fn the_configs_update_branch_and_merge_method_win_over_the_repositorys() {
    let merging_by = |merge_method| Some(AutoMerge { merge_method });
    for (allowed, update_method, auto_merge, update_rule, merge_method) in [
        (
            &[Merge, Squash, Rebase][..],
            None,
            None,
            UpdateRule::RebaseOnConflict,
            Merge,
        ),
        (
            &[Merge, Squash, Rebase],
            Some(UpdateMethod::Merge),
            None,
            UpdateRule::Always(UpdateMethod::Merge),
            Merge,
        ),
        (
            &[Merge, Squash],
            Some(UpdateMethod::Rebase),
            None,
            UpdateRule::Always(UpdateMethod::Rebase),
            Merge,
        ),
        (
            &[Squash, Rebase],
            None,
            merging_by(None),
            UpdateRule::RebaseOnConflict,
            Squash,
        ),
        (
            &[Squash, Rebase],
            None,
            merging_by(Some(Rebase)),
            UpdateRule::RebaseOnConflict,
            Rebase,
        ),
        // Without auto-merge, the merge method is not checked against the
        // update-branch rule.
        (
            &[Rebase],
            Some(UpdateMethod::Merge),
            None,
            UpdateRule::Always(UpdateMethod::Merge),
            Rebase,
        ),
    ] {
        let case = format!("allowing {allowed:?}, {update_method:?}, {auto_merge:?}");
        let settings = read(false, allowed)
            .configured(update_method, auto_merge)
            .expect(&case);

        assert_eq!(settings.update_rule, update_rule, "{case}");
        assert_eq!(settings.merge_method, merge_method, "{case}");
    }
}

#[test]
fn with_auto_merge_settings_that_cannot_merge_are_an_error_saying_what_to_set() {
    for (linear_history, allowed, update_method, merge_method, error) in [
        (
            false,
            &[Merge, Rebase][..],
            None,
            Some(Squash),
            "the repository doesn't allow the merge method `squash`: it allows `merge` and \
             `rebase`; set `merge-method` to a method it allows",
        ),
        (
            true,
            &[Merge, Squash],
            None,
            None,
            "the repository doesn't allow the merge method `rebase`: it allows `merge` and \
             `squash`; set `merge-method` to a method it allows",
        ),
        (
            false,
            &[Rebase],
            Some(UpdateMethod::Merge),
            None,
            "`update-branch` `merge` cannot keep branches rebase-mergeable, and the merge \
             method is `rebase`: set `update-branch` to `rebase`, or `merge-method` to another \
             method the repository allows (it allows only `rebase`)",
        ),
    ] {
        let configured = read(linear_history, allowed)
            .configured(update_method, Some(AutoMerge { merge_method }));

        assert_eq!(configured.unwrap_err().to_string(), error);
    }
}

#[test]
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

#[test]
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
