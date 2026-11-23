//! A browser's sign-in (`docs/specs/access.md`, Signing in, Signed in): an
//! opaque cookie, kept in the database only as its SHA-256, for 30 days or
//! until the person signs out.

use anyhow::Context as _;
use chrono::TimeDelta;

use super::Caller;
use super::people::{People, Person, PersonId};
use super::tokens::{hashed, host_cookie, random_token};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;

/// The name of the cookie that holds a browser's sign-in.
pub const SESSION_COOKIE: &str = "__Host-linewright-session";

/// How long a browser stays signed in.
pub const SIGNED_IN_FOR: TimeDelta = TimeDelta::days(30);

/// How long Linewright knows a sign-in after it ended, and a browser keeps
/// its cookie after that, so that Sign in can say that it ended
/// (`docs/specs/access.md`, Sign in, The message).
pub const KNOWN_ENDED_FOR: TimeDelta = TimeDelta::days(30);

/// The browsers' sign-ins, in the database.
#[derive(Clone, Debug)]
pub struct Sessions {
    db: Database,
}

/// The value of a browser's session cookie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCookie(String);

impl SessionCookie {
    /// The cookie whose value is `value`, as a browser sent it.
    pub fn of(value: String) -> SessionCookie {
        SessionCookie(value)
    }

    /// Its value.
    pub fn value(&self) -> &str {
        &self.0
    }

    /// The `Set-Cookie` that keeps it in the browser for as long as
    /// Linewright knows its sign-in: [`SIGNED_IN_FOR`], then
    /// [`KNOWN_ENDED_FOR`].
    pub fn set_cookie(&self) -> String {
        host_cookie(SESSION_COOKIE, &self.0, SIGNED_IN_FOR + KNOWN_ENDED_FOR)
    }

    /// The `Set-Cookie` that removes it from the browser.
    pub fn removed() -> String {
        host_cookie(SESSION_COOKIE, "", TimeDelta::zero())
    }
}

/// Who a browser's session cookie signs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    Caller(Caller),
    /// A sign-in past its 30 days.
    Ended,
    /// No sign-in: an unknown cookie, or a disabled person's.
    NoOne,
}

impl Sessions {
    pub fn new(db: Database) -> Sessions {
        Sessions { db }
    }

    /// Signs a browser in as `person` from `now` for [`SIGNED_IN_FOR`],
    /// forgetting the sign-ins that ended over [`KNOWN_ENDED_FOR`] before.
    /// A person disabled by then is signed in nowhere: the cookie signs no
    /// one in, even once they are enabled again.
    pub async fn start(&self, person: PersonId, now: Timestamp) -> anyhow::Result<SessionCookie> {
        let cookie = SessionCookie(random_token());
        sqlx::query("DELETE FROM browser_sessions WHERE expires_at < ?")
            .bind(time_text(now - KNOWN_ENDED_FOR))
            .execute(self.db.pool())
            .await
            .context("forgetting the ended sign-ins")?;
        sqlx::query(
            "INSERT INTO browser_sessions (hash, person, expires_at)
             SELECT ?, id, ? FROM people WHERE id = ? AND state = 'enabled'",
        )
        .bind(hashed(&cookie.0))
        .bind(time_text(now + SIGNED_IN_FOR))
        .bind(person.row())
        .execute(self.db.pool())
        .await
        .with_context(|| format!("signing person {person} in"))?;
        Ok(cookie)
    }

    /// Who `cookie` signs in at `now`, as their record has them now.
    pub async fn resolve(
        &self,
        cookie: &SessionCookie,
        now: Timestamp,
    ) -> anyhow::Result<Resolved> {
        let hash = hashed(&cookie.0);
        let Some((person, expires_at)) = self.session(&hash).await? else {
            return Ok(Resolved::NoOne);
        };
        if parse_time(&expires_at)? <= now {
            return Ok(Resolved::Ended);
        }
        let person = People::new(self.db.clone()).get(person).await?;
        Ok(person
            .and_then(Caller::of)
            .map_or(Resolved::NoOne, Resolved::Caller))
    }

    /// Signs out the browser of `cookie`, and gives who it signed in, if
    /// anyone.
    pub async fn end(&self, cookie: &SessionCookie) -> anyhow::Result<Option<Person>> {
        let hash = hashed(&cookie.0);
        let Some((person, _)) = self.session(&hash).await? else {
            return Ok(None);
        };
        self.forget(&hash).await?;
        People::new(self.db.clone()).get(person).await
    }

    /// The person the sign-in of `hash` is of, and when it ends, if there
    /// is one.
    async fn session(&self, hash: &[u8]) -> anyhow::Result<Option<(PersonId, String)>> {
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT person, expires_at FROM browser_sessions WHERE hash = ?")
                .bind(hash)
                .fetch_optional(self.db.pool())
                .await
                .context("reading the browser's sign-in")?;
        Ok(row.map(|(person, expires_at)| (PersonId::of_row(person), expires_at)))
    }

    async fn forget(&self, hash: &[u8]) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM browser_sessions WHERE hash = ?")
            .bind(hash)
            .execute(self.db.pool())
            .await
            .context("forgetting the browser's sign-in")?;
        Ok(())
    }
}
