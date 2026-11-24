//! The actions on one's own authorizations (`docs/specs/access.md`,
//! Authorizations and End authorization): seeing them, and ending one.

use super::{Action, Ended, Ending, logged};
use crate::access::oauth::store::{AuthorizationListed, Authorizations, EndRefusal};
use crate::access::oauth::{AuthorizationId, client_name};
use crate::access::{Permit, Read};
use crate::api::Sources;

/// The authorizations that `sources` keep.
fn authorizations(sources: &Sources) -> Authorizations {
    Authorizations::new(sources.runs.database().clone())
}

/// The authorizations of `permit`'s caller that have not ended, newest
/// first (`docs/specs/access.md`, Authorizations).
pub async fn list_authorizations(
    permit: &Permit<Read>,
    sources: &Sources,
) -> anyhow::Result<Vec<AuthorizationListed>> {
    authorizations(sources)
        .list_of(permit.caller().person().id, sources.clock.now())
        .await
}

/// Ends authorization `id` of `permit`'s caller, and gives it as it was
/// (`docs/specs/access.md`, End authorization).
///
/// # Errors
///
/// When the caller has no such authorization that has not ended, such as
/// another person's, or it cannot be written.
pub async fn end_authorization(
    permit: &Permit<Read>,
    sources: &Sources,
    id: AuthorizationId,
) -> Result<AuthorizationListed, EndRefusal> {
    let ended = authorizations(sources)
        .end(permit.caller().person().id, id, sources.clock.now())
        .await;
    let client = ended.as_ref().map_or(String::new(), |ended| {
        format!(" ({})", client_name(ended.client_name.as_deref()))
    });
    logged(
        permit.holder(),
        Action::EndAuthorization,
        &format!("authorization {id}{client}, ended"),
        Ended::of(&ended),
    );
    ended
}

impl Ending for EndRefusal {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            EndRefusal::NoSuchAuthorization(_) => None,
            EndRefusal::Failed(e) => Some(e),
        }
    }
}
