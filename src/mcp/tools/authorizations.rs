//! The tools on one's own authorizations (`docs/specs/access.md`,
//! Authorizations and End authorization).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Context, Tool, id};
use crate::access::oauth::store::{AuthorizationListed, EndRefusal};
use crate::access::oauth::{AuthorizationId, client_name};
use crate::access::{Permit, Read};
use crate::actions::{self, Action, Missing};
use crate::domain::Timestamp;
use crate::mcp::page::{Page, Paging};

/// An authorization of the caller's, as a tool returns it.
#[derive(Serialize, JsonSchema)]
pub(super) struct AuthorizationOut {
    id: i64,
    /// The client's name as it registered it, or "an unnamed client".
    client: String,
    /// The host of the client's redirect URI.
    sends_back_to: String,
    /// When the caller allowed the client.
    allowed_at: Timestamp,
    /// When it ends, or for one just ended, when it ended.
    ends_at: Timestamp,
}

impl From<AuthorizationListed> for AuthorizationOut {
    fn from(listed: AuthorizationListed) -> AuthorizationOut {
        AuthorizationOut {
            id: listed.id.row(),
            client: client_name(listed.client_name.as_deref()).to_string(),
            sends_back_to: listed.redirect_uri.host().to_string(),
            allowed_at: listed.allowed_at,
            ends_at: listed.ends_at,
        }
    }
}

/// See one's own authorizations, newest first, which is the order of
/// their IDs from the highest (`docs/specs/access.md`,
/// Authorizations).
#[derive(Deserialize, JsonSchema)]
#[serde(transparent)]
pub(in crate::mcp) struct ListAuthorizations {
    paging: Paging,
}

impl Tool for ListAuthorizations {
    const ACTION: Action = Action::ListAuthorizations;
    type Level = Read;
    type Output = Page<AuthorizationOut>;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<Page<AuthorizationOut>, String> {
        let listed = actions::list_authorizations(permit, context.sources)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let listed = listed.into_iter().map(AuthorizationOut::from).collect();
        Ok(self.paging.page_in_order(
            listed,
            |authorization: &AuthorizationOut| authorization.id.to_string(),
            |authorization, after| {
                after
                    .parse()
                    .is_ok_and(|after: i64| authorization.id < after)
            },
        ))
    }
}

/// End one of one's own authorizations: its client can no longer act as
/// one (`docs/specs/access.md`, End authorization).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct EndAuthorization {
    authorization_id: i64,
}

impl Tool for EndAuthorization {
    const ACTION: Action = Action::EndAuthorization;
    type Level = Read;
    type Output = AuthorizationOut;

    fn changing(&self) -> String {
        format!("authorization {}, ended", self.authorization_id)
    }

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<AuthorizationOut, String> {
        let id: AuthorizationId = id(self.authorization_id, str::parse, Missing::Authorization)?;
        match actions::end_authorization(permit, context.sources, id).await {
            Ok(ended) => Ok(AuthorizationOut {
                ends_at: context.sources.clock.now(),
                ..ended.into()
            }),
            Err(e @ EndRefusal::NoSuchAuthorization(_)) => Err(e.to_string()),
            Err(EndRefusal::Failed(e)) => Err(format!("{e:#}")),
        }
    }
}
