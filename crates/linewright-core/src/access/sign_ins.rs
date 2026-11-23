//! The sign-ins begun at an identity provider and not yet back: each kept
//! by its `state`, with its PKCE verifier and its nonce, for the browser
//! that began it, and taken once by the provider's callback.

use anyhow::Context as _;
use chrono::TimeDelta;

use super::providers::{Begun, Provider};
use super::tokens::{hashed, host_cookie, random_token};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;

/// The name of the cookie that ties a sign-in begun to its browser, so that
/// no one can have another's browser finish a sign-in they began.
/// <https://www.rfc-editor.org/rfc/rfc9700#name-cross-site-request-forgery>
pub const SIGN_IN_COOKIE: &str = "__Host-linewright-sign-in";

/// How long a sign-in begun waits for the provider to send the browser
/// back.
const PENDING_FOR: TimeDelta = TimeDelta::minutes(10);

/// A sign-in taken: its verifier, nonce, return path, authorization request
/// and when it stops waiting.
type TakenRow = (String, String, Option<String>, Option<String>, String);

/// The sign-ins begun, in the database.
#[derive(Clone, Debug)]
pub struct SignIns {
    db: Database,
}

/// What a sign-in is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum After {
    /// Returning to the screen of this path.
    Screen(String),
    /// The authorization request of this handle.
    Authorization(String),
}

impl After {
    /// The path of the screen it returns to, if it is for one.
    fn screen(&self) -> Option<&str> {
        match self {
            After::Screen(path) => Some(path),
            After::Authorization(_) => None,
        }
    }

    /// The handle of the authorization request, if it is for one.
    fn authorization(&self) -> Option<&str> {
        match self {
            After::Screen(_) => None,
            After::Authorization(handle) => Some(handle),
        }
    }
}

/// A sign-in begun at a provider, with what it is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub provider: Provider,
    pub state: String,
    pub verifier: String,
    pub nonce: String,
    pub after: After,
    /// The value of [`SIGN_IN_COOKIE`] in the browser that began it.
    pub browser: String,
}

impl Pending {
    pub fn begun(&self) -> Begun<'_> {
        Begun {
            state: &self.state,
            verifier: &self.verifier,
            nonce: &self.nonce,
        }
    }

    /// The `Set-Cookie` that ties it to the browser, as long as it waits.
    pub fn set_cookie(&self) -> String {
        host_cookie(SIGN_IN_COOKIE, &self.browser, PENDING_FOR)
    }
}

impl SignIns {
    pub fn new(db: Database) -> SignIns {
        SignIns { db }
    }

    /// Begins a sign-in at `provider` at `now`, for `after`, forgetting
    /// those that waited too long. The browser's [`SIGN_IN_COOKIE`],
    /// `browser`, ties it to the browser, so that its other sign-ins begun
    /// still wait; or a new one, for a browser without.
    pub async fn begin(
        &self,
        provider: Provider,
        after: &After,
        browser: Option<&str>,
        now: Timestamp,
    ) -> anyhow::Result<Pending> {
        let pending = Pending {
            provider,
            state: random_token(),
            verifier: random_token(),
            nonce: random_token(),
            after: after.clone(),
            browser: browser.map_or_else(random_token, ToString::to_string),
        };
        sqlx::query("DELETE FROM pending_sign_ins WHERE expires_at <= ?")
            .bind(time_text(now))
            .execute(self.db.pool())
            .await
            .context("forgetting the sign-ins that waited too long")?;
        sqlx::query(
            "INSERT INTO pending_sign_ins
                (state, browser, provider, verifier, nonce, return_to,
                 authorization_request, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(hashed(&pending.state))
        .bind(hashed(&pending.browser))
        .bind(provider.to_string())
        .bind(&pending.verifier)
        .bind(&pending.nonce)
        .bind(after.screen())
        .bind(after.authorization())
        .bind(time_text(now + PENDING_FOR))
        .execute(self.db.pool())
        .await
        .context("beginning the sign-in")?;
        Ok(pending)
    }

    /// Takes the sign-in of `state` at `provider` begun by the browser whose
    /// [`SIGN_IN_COOKIE`] is `browser`, so that no other request can: `None`
    /// when there is none, or when it waited too long.
    pub async fn take(
        &self,
        provider: Provider,
        state: &str,
        browser: Option<&str>,
        now: Timestamp,
    ) -> anyhow::Result<Option<Pending>> {
        let Some(browser) = browser else {
            return Ok(None);
        };
        let row: Option<TakenRow> = sqlx::query_as(
            "DELETE FROM pending_sign_ins WHERE state = ? AND browser = ? AND provider = ?
             RETURNING verifier, nonce, return_to, authorization_request, expires_at",
        )
        .bind(hashed(state))
        .bind(hashed(browser))
        .bind(provider.to_string())
        .fetch_optional(self.db.pool())
        .await
        .context("taking the sign-in")?;
        let Some((verifier, nonce, return_to, request, expires_at)) = row else {
            return Ok(None);
        };
        let after = match (return_to, request) {
            (Some(path), None) => After::Screen(path),
            (None, Some(handle)) => After::Authorization(handle),
            _ => anyhow::bail!(
                "the sign-in is for both a screen and an authorization request, or for neither"
            ),
        };
        if parse_time(&expires_at)? <= now {
            return Ok(None);
        }
        Ok(Some(Pending {
            provider,
            state: state.to_string(),
            verifier,
            nonce,
            after,
            browser: browser.to_string(),
        }))
    }
}
