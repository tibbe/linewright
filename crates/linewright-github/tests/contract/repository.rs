//! Who Linewright is, and the repository's settings.

use anyhow::{Context, Result, bail, ensure};
use linewright_core::domain::{MergeMethod, Scope};
use linewright_core::store::Property;
use serde_json::json;

use crate::check::{contracts, equal};
use crate::sandbox::{Sandbox, recent};

contracts![
    linewright_acts_as_a_login_with_its_noreply_email,
    the_settings_name_the_repository_and_the_default_branch_a_snapshot_has,
    the_properties_are_recorded_as_githubs_answers_naming_the_repository_and_the_account,
    a_token_that_can_read_the_repository_passes_the_read_check,
    a_token_that_cannot_read_the_repository_is_refused_naming_read_access,
];

async fn linewright_acts_as_a_login_with_its_noreply_email(sandbox: Sandbox) -> Result<()> {
    let identity = sandbox.store().identity().await?;

    ensure!(!identity.login.as_str().is_empty(), "no login");
    equal("the name", identity.name.as_str(), identity.login.as_str())?;
    // <https://docs.github.com/en/account-and-profile/setting-up-and-managing-your-personal-account-on-github/managing-email-preferences/setting-your-commit-email-address#about-no-reply-email-addresses>
    let email = &identity.email;
    ensure!(
        email.ends_with(&format!("{}@users.noreply.github.com", identity.login)),
        "the email {email} is not {}'s no-reply address",
        identity.login
    );
    Ok(())
}

async fn the_settings_name_the_repository_and_the_default_branch_a_snapshot_has(
    sandbox: Sandbox,
) -> Result<()> {
    let settings = sandbox.store().settings().await?;
    let snapshot = sandbox
        .store()
        .snapshot(recent(), &Scope::default())
        .await?;

    equal("the repository", &settings.repo, sandbox.repo())?;
    equal(
        "the default branch",
        &settings.default_branch,
        &snapshot.default_branch,
    )?;
    ensure!(
        MergeMethod::ALL
            .into_iter()
            .any(|method| settings.allowed_merge_methods.contains(method)),
        "the repository allows no merge method"
    );
    Ok(())
}

async fn the_properties_are_recorded_as_githubs_answers_naming_the_repository_and_the_account(
    sandbox: Sandbox,
) -> Result<()> {
    let recorded = sandbox.store().record_properties().await?;
    let identity = sandbox.store().identity().await?;

    let answer = |property: Property| {
        recorded
            .resource(property)
            .with_context(|| format!("{property} is not recorded"))
    };
    equal(
        "the repository's full name",
        &answer(Property::Repository)?["full_name"],
        &json!(sandbox.repo().to_string()),
    )?;
    equal(
        "the account's login",
        &answer(Property::Account)?["login"],
        &json!(identity.login.as_str()),
    )?;
    ensure!(
        answer(Property::IssueTypesAndMergeMethods)?["repository"]["mergeCommitAllowed"]
            .is_boolean(),
        "the merge methods are not GitHub's answer"
    );
    // GitHub serves a branch's rules for a public repository or on a paid
    // plan only, and refuses them otherwise, as for a private repository on
    // a free plan:
    // <https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets>
    let rules = answer(Property::Rules)?;
    ensure!(
        rules.is_array() || rules.is_null(),
        "the rules are neither GitHub's list nor its refusal for the plan: {rules}"
    );
    answer(Property::Protection)?;
    Ok(())
}

async fn a_token_that_can_read_the_repository_passes_the_read_check(
    sandbox: Sandbox,
) -> Result<()> {
    sandbox.store().check_reads(&sandbox.reading_token()).await
}

async fn a_token_that_cannot_read_the_repository_is_refused_naming_read_access(
    sandbox: Sandbox,
) -> Result<()> {
    match sandbox
        .store()
        .check_reads(&sandbox.unreadable_token())
        .await
    {
        Ok(()) => bail!("the unreadable token passes"),
        Err(error) => {
            let message = format!("{error:#}");
            ensure!(
                message.contains("read access"),
                "the refusal names no read access: {message}"
            );
            Ok(())
        }
    }
}
