//! Who Linewright is, and the repository's settings.

use anyhow::{Result, ensure};
use linewright::domain::MergeMethod;

use crate::check::{contracts, equal};
use crate::sandbox::{Sandbox, recent};

contracts![
    linewright_acts_as_a_login_with_its_noreply_email,
    the_settings_name_the_repository_and_the_default_branch_a_snapshot_has,
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
    let snapshot = sandbox.store().snapshot(recent()).await?;

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
