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

/// The sign-ins begun, in the database.
#[derive(Clone, Debug)]
pub struct SignIns {
    db: Database,
}

/// A sign-in begun at a provider, with where the browser goes once it is
/// back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub provider: Provider,
    pub state: String,
    pub verifier: String,
    pub nonce: String,
    /// The path the browser returns to once signed in.
    pub return_to: String,
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

    /// Begins a sign-in at `provider` at `now`, for a browser to return to
    /// `return_to`, forgetting those that waited too long. The browser's
    /// [`SIGN_IN_COOKIE`], `browser`, ties it to the browser, so that its
    /// other sign-ins begun still wait; or a new one, for a browser without.
    pub async fn begin(
        &self,
        provider: Provider,
        return_to: &str,
        browser: Option<&str>,
        now: Timestamp,
    ) -> anyhow::Result<Pending> {
        let pending = Pending {
            provider,
            state: random_token(),
            verifier: random_token(),
            nonce: random_token(),
            return_to: return_to.to_string(),
            browser: browser.map_or_else(random_token, ToString::to_string),
        };
        sqlx::query("DELETE FROM pending_sign_ins WHERE expires_at <= ?")
            .bind(time_text(now))
            .execute(self.db.pool())
            .await
            .context("forgetting the sign-ins that waited too long")?;
        sqlx::query(
            "INSERT INTO pending_sign_ins
                (state, browser, provider, verifier, nonce, return_to, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(hashed(&pending.state))
        .bind(hashed(&pending.browser))
        .bind(provider.to_string())
        .bind(&pending.verifier)
        .bind(&pending.nonce)
        .bind(return_to)
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
        let row: Option<(String, String, String, String)> = sqlx::query_as(
            "DELETE FROM pending_sign_ins WHERE state = ? AND browser = ? AND provider = ?
             RETURNING verifier, nonce, return_to, expires_at",
        )
        .bind(hashed(state))
        .bind(hashed(browser))
        .bind(provider.to_string())
        .fetch_optional(self.db.pool())
        .await
        .context("taking the sign-in")?;
        let Some((verifier, nonce, return_to, expires_at)) = row else {
            return Ok(None);
        };
        if parse_time(&expires_at)? <= now {
            return Ok(None);
        }
        Ok(Some(Pending {
            provider,
            state: state.to_string(),
            verifier,
            nonce,
            return_to,
            browser: browser.to_string(),
        }))
    }
}
