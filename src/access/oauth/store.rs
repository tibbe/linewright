//! The authorization server's records in the database: the authorization
//! requests waiting for their person, the authorizations, and the codes and
//! tokens, each kept only as its SHA-256, and found by it.

use anyhow::Context as _;
use chrono::TimeDelta;
use tracing::info;

use super::{
    AuthorizationId, CODE_LASTS, Challenge, Checked, ClientId, Grant, OAuthError, RedirectUri,
    RefreshState, Server, Stored, StoredAuthorization, TokenDecision, TokenRequest, Used,
    access_works, authorization_ends, exchange,
};
use crate::access::Caller;
use crate::access::people::{People, PersonId};
use crate::access::tokens::{hashed, random_token};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;

/// How long an authorization request waits for its person to sign in, and
/// then to allow or deny its client.
const WAITS_FOR: TimeDelta = TimeDelta::minutes(10);

/// The authorization requests waiting for their person.
#[derive(Clone, Debug)]
pub struct AuthorizationRequests {
    db: Database,
}

/// An authorization request waiting for its person: its client, where it
/// sends the browser back, and who signed in for it, once someone did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    pub client: ClientId,
    pub client_name: Option<String>,
    pub redirect_uri: RedirectUri,
    pub challenge: Challenge,
    pub state: Option<String>,
    /// Who signed in for it, and when.
    pub signed_in: Option<(PersonId, Timestamp)>,
}

impl Waiting {
    /// Whether `person` signed in for it.
    pub fn signed_in_by(&self, person: PersonId) -> bool {
        self.signed_in
            .is_some_and(|(signed_in, _)| signed_in == person)
    }
}

type WaitingRow = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    String,
);

/// The query of the authorization request whose secret's hash it binds.
macro_rules! waiting {
    () => {
        "SELECT r.client, c.name, r.redirect_uri, r.challenge, r.state, r.person,
            r.signed_in_at, r.expires_at
         FROM authorization_requests r JOIN clients c ON c.id = r.client WHERE r.hash = ?"
    };
}

impl AuthorizationRequests {
    pub fn new(db: Database) -> AuthorizationRequests {
        AuthorizationRequests { db }
    }

    /// Keeps `checked` from `now` until its person signs in and allows or
    /// denies its client, forgetting those that waited too long, and gives
    /// the secret that names it.
    pub async fn begin(&self, checked: &Checked, now: Timestamp) -> anyhow::Result<String> {
        let secret = random_token();
        sqlx::query("DELETE FROM authorization_requests WHERE expires_at <= ?")
            .bind(time_text(now))
            .execute(self.db.pool())
            .await
            .context("forgetting the authorization requests that waited too long")?;
        sqlx::query(
            "INSERT INTO authorization_requests
                (hash, client, redirect_uri, challenge, state, expires_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(hashed(&secret))
        .bind(checked.client.as_str())
        .bind(checked.redirect_uri.as_str())
        .bind(checked.challenge.as_str())
        .bind(&checked.state)
        .bind(time_text(now + WAITS_FOR))
        .execute(self.db.pool())
        .await
        .context("keeping the authorization request")?;
        Ok(secret)
    }

    /// Records that `person` signed in at `now` for the request `secret`
    /// names, which then waits for them, and no one who signed in for it
    /// before, to allow or deny its client.
    pub async fn signed_in(
        &self,
        secret: &str,
        person: PersonId,
        now: Timestamp,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE authorization_requests SET person = ?, signed_in_at = ?, expires_at = ?
             WHERE hash = ? AND expires_at > ?",
        )
        .bind(person.row())
        .bind(time_text(now))
        .bind(time_text(now + WAITS_FOR))
        .bind(hashed(secret))
        .bind(time_text(now))
        .execute(self.db.pool())
        .await
        .context("recording the sign-in for the authorization request")?;
        Ok(())
    }

    /// The request `secret` names, if it still waits at `now`.
    pub async fn get(&self, secret: &str, now: Timestamp) -> anyhow::Result<Option<Waiting>> {
        let row: Option<WaitingRow> = sqlx::query_as(waiting!())
            .bind(hashed(secret))
            .fetch_optional(self.db.pool())
            .await
            .context("reading the authorization request")?;
        waiting_of(row, now)
    }

    /// Answers the request `secret` names with `decision` at `now`, for
    /// the MCP server of `server`, if it still waits, `person` signed in for
    /// it and is enabled, taking it so that no other request can. Allowing
    /// records an authorization that ends as [`authorization_ends`] says
    /// from that sign-in, and the code the client exchanges for its tokens.
    /// One transaction reads and writes, so that disabling the person
    /// cannot come between.
    pub async fn answer(
        &self,
        secret: &str,
        person: PersonId,
        decision: Decision,
        server: &Server,
        now: Timestamp,
    ) -> anyhow::Result<Option<Answered>> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<WaitingRow> = sqlx::query_as(concat!(
            waiting!(),
            " AND r.person = ? AND EXISTS
                (SELECT 1 FROM people p WHERE p.id = r.person AND p.state = 'enabled')"
        ))
        .bind(hashed(secret))
        .bind(person.row())
        .fetch_optional(&mut *tx)
        .await
        .context("reading the authorization request")?;
        let Some(waiting) = waiting_of(row, now)? else {
            return Ok(None);
        };
        let Some((_, signed_in_at)) = waiting.signed_in else {
            return Ok(None);
        };
        sqlx::query("DELETE FROM authorization_requests WHERE hash = ?")
            .bind(hashed(secret))
            .execute(&mut *tx)
            .await
            .context("taking the authorization request")?;
        let answered = match decision {
            Decision::Deny => Answered::Denied { waiting },
            Decision::Allow => {
                let code = random_token();
                let (id,): (i64,) = sqlx::query_as(
                    "INSERT INTO authorizations
                        (person, client, redirect_uri, resource, allowed_at, ends_at)
                     VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
                )
                .bind(person.row())
                .bind(waiting.client.as_str())
                .bind(waiting.redirect_uri.as_str())
                .bind(server.resource().as_str())
                .bind(time_text(now))
                .bind(time_text(authorization_ends(signed_in_at)))
                .fetch_one(&mut *tx)
                .await
                .context("recording the authorization")?;
                sqlx::query(
                    "INSERT INTO codes
                        (hash, authorization, challenge, redirect_uri, expires_at, used)
                     VALUES (?, ?, ?, ?, ?, 0)",
                )
                .bind(hashed(&code))
                .bind(id)
                .bind(waiting.challenge.as_str())
                .bind(waiting.redirect_uri.as_str())
                .bind(time_text(now + CODE_LASTS))
                .execute(&mut *tx)
                .await
                .context("keeping the code")?;
                Answered::Allowed {
                    waiting,
                    authorization: AuthorizationId::of_row(id),
                    code,
                }
            }
        };
        tx.commit().await?;
        Ok(Some(answered))
    }
}

/// What a person decides on Consent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
}

/// An authorization request answered, with the request as it waited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answered {
    /// Allowed, with its authorization and the code for its client.
    Allowed {
        waiting: Waiting,
        authorization: AuthorizationId,
        code: String,
    },
    Denied {
        waiting: Waiting,
    },
}

fn waiting_of(row: Option<WaitingRow>, now: Timestamp) -> anyhow::Result<Option<Waiting>> {
    let Some((
        client,
        client_name,
        redirect_uri,
        challenge,
        state,
        person,
        signed_in_at,
        expires_at,
    )) = row
    else {
        return Ok(None);
    };
    if parse_time(&expires_at)? <= now {
        return Ok(None);
    }
    let signed_in = match (person, signed_in_at) {
        (Some(person), Some(at)) => Some((PersonId::of_row(person), parse_time(&at)?)),
        _ => None,
    };
    Ok(Some(Waiting {
        client: ClientId::of(client),
        client_name,
        redirect_uri: stored_redirect_uri(&redirect_uri)?,
        challenge: stored_challenge(&challenge)?,
        state,
        signed_in,
    }))
}

/// The redirect URI `stored`, as the database holds it.
pub(crate) fn stored_redirect_uri(stored: &str) -> anyhow::Result<RedirectUri> {
    RedirectUri::parse(stored).map_err(anyhow::Error::msg)
}

/// The PKCE challenge `stored`, as the database holds it.
fn stored_challenge(stored: &str) -> anyhow::Result<Challenge> {
    Challenge::parse(stored).with_context(|| format!("{stored:?} is no PKCE challenge"))
}

/// The people's authorizations.
#[derive(Clone, Debug)]
pub struct Authorizations {
    db: Database,
}

/// An authorization of a person: its client's name, where it sends the
/// browser back, and when it was allowed and ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationListed {
    pub id: AuthorizationId,
    pub client_name: Option<String>,
    pub redirect_uri: RedirectUri,
    pub allowed_at: Timestamp,
    pub ends_at: Timestamp,
}

/// Why an authorization was not ended.
#[derive(Debug, thiserror::Error)]
pub enum EndRefusal {
    /// The reader has no such authorization that has not ended.
    #[error("No such authorization {0}")]
    NoSuchAuthorization(AuthorizationId),
    #[error(transparent)]
    Failed(anyhow::Error),
}

type ListedRow = (i64, Option<String>, String, String, String);

/// The query of [`AuthorizationListed`]s, of the authorizations `a` and
/// their clients `c`, short of its conditions.
macro_rules! listed_query {
    () => {
        "SELECT a.id, c.name, a.redirect_uri, a.allowed_at, a.ends_at
         FROM authorizations a JOIN clients c ON c.id = a.client "
    };
}

impl Authorizations {
    pub fn new(db: Database) -> Authorizations {
        Authorizations { db }
    }

    /// The authorizations of `person` that have not ended at `now`, newest
    /// first (`docs/specs/access.md`, Authorizations).
    pub async fn list_of(
        &self,
        person: PersonId,
        now: Timestamp,
    ) -> anyhow::Result<Vec<AuthorizationListed>> {
        let rows: Vec<ListedRow> = sqlx::query_as(concat!(
            listed_query!(),
            "WHERE a.person = ? AND a.ended_at IS NULL AND a.ends_at > ? ORDER BY a.id DESC"
        ))
        .bind(person.row())
        .bind(time_text(now))
        .fetch_all(self.db.pool())
        .await
        .context("reading the authorizations")?;
        rows.into_iter().map(listed_of).collect()
    }

    /// Ends authorization `id` of `person` at `now`, and gives it as it was.
    ///
    /// # Errors
    ///
    /// When `person` has no such authorization that has not ended, or it
    /// cannot be written.
    pub async fn end(
        &self,
        person: PersonId,
        id: AuthorizationId,
        now: Timestamp,
    ) -> Result<AuthorizationListed, EndRefusal> {
        let ended = async {
            let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
            let row: Option<ListedRow> = sqlx::query_as(concat!(
                listed_query!(),
                "WHERE a.id = ? AND a.person = ? AND a.ended_at IS NULL AND a.ends_at > ?"
            ))
            .bind(id.row())
            .bind(person.row())
            .bind(time_text(now))
            .fetch_optional(&mut *tx)
            .await?;
            let Some(listed) = row.map(listed_of).transpose()? else {
                return Ok(None);
            };
            end_on(&mut tx, id, now).await?;
            tx.commit().await?;
            anyhow::Ok(Some(listed))
        };
        match ended.await {
            Ok(Some(listed)) => Ok(listed),
            Ok(None) => Err(EndRefusal::NoSuchAuthorization(id)),
            Err(e) => Err(EndRefusal::Failed(
                e.context(format!("ending authorization {id}")),
            )),
        }
    }
}

/// Ends every authorization of `person` at `now`, and forgets the
/// authorization requests they signed in for, in the transaction `conn` has
/// open.
pub(crate) async fn end_all_of(
    conn: &mut sqlx::SqliteConnection,
    person: PersonId,
    now: Timestamp,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM authorization_requests WHERE person = ?")
        .bind(person.row())
        .execute(&mut *conn)
        .await
        .context("forgetting the person's authorization requests")?;
    sqlx::query(
        "DELETE FROM access_tokens WHERE authorization IN
            (SELECT id FROM authorizations WHERE person = ? AND ended_at IS NULL)",
    )
    .bind(person.row())
    .execute(&mut *conn)
    .await
    .context("forgetting the person's access tokens")?;
    sqlx::query("UPDATE authorizations SET ended_at = ? WHERE person = ? AND ended_at IS NULL")
        .bind(time_text(now))
        .bind(person.row())
        .execute(&mut *conn)
        .await
        .context("ending the person's authorizations")?;
    Ok(())
}

/// Ends authorization `id` at `now`, in the transaction `conn` has open.
async fn end_on(
    conn: &mut sqlx::SqliteConnection,
    id: AuthorizationId,
    now: Timestamp,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM access_tokens WHERE authorization = ?")
        .bind(id.row())
        .execute(&mut *conn)
        .await
        .context("forgetting the authorization's access tokens")?;
    sqlx::query("UPDATE authorizations SET ended_at = ? WHERE id = ? AND ended_at IS NULL")
        .bind(time_text(now))
        .bind(id.row())
        .execute(&mut *conn)
        .await
        .context("ending the authorization")?;
    Ok(())
}

fn listed_of(row: ListedRow) -> anyhow::Result<AuthorizationListed> {
    let (id, client_name, redirect_uri, allowed_at, ends_at) = row;
    Ok(AuthorizationListed {
        id: AuthorizationId::of_row(id),
        client_name,
        redirect_uri: stored_redirect_uri(&redirect_uri)?,
        allowed_at: parse_time(&allowed_at)?,
        ends_at: parse_time(&ends_at)?,
    })
}

/// The tokens Linewright issues to clients.
#[derive(Clone, Debug)]
pub struct Tokens {
    db: Database,
}

/// The tokens a token request is issued.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-3.2.3>
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Issued {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub refresh_token: String,
}

type AuthorizationRow = (i64, String, String, Option<String>);

impl Tokens {
    pub fn new(db: Database) -> Tokens {
        Tokens { db }
    }

    /// Answers `request` at `now`, for the MCP server of `server`, as
    /// [`exchange`] decides from what the database holds of what it
    /// presents: the tokens it is issued, or the error it is answered with.
    /// Reading and writing are one transaction, so that a code or a
    /// refresh token is used once.
    pub async fn answer(
        &self,
        request: &TokenRequest,
        server: &Server,
        now: Timestamp,
    ) -> anyhow::Result<Result<Issued, OAuthError>> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        forget_the_dead(&mut tx, now).await?;
        let (presented, stored) = match &request.grant {
            Grant::Code { code, .. } => {
                let presented = hashed(code);
                let stored = stored_code(&mut tx, &presented).await?;
                (presented, stored)
            }
            Grant::Refresh { token } => {
                let presented = hashed(token);
                let stored = stored_refresh(&mut tx, &presented).await?;
                (presented, stored)
            }
        };
        match exchange(request, &stored, now, server) {
            TokenDecision::Issue {
                authorization,
                used,
                access_until,
            } => {
                let id = authorization.row();
                if used == Used::Code {
                    sqlx::query("UPDATE codes SET used = 1 WHERE hash = ?")
                        .bind(&presented)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query(
                    "UPDATE refresh_tokens
                     SET state = CASE state WHEN ? THEN ? WHEN ? THEN ? ELSE state END
                     WHERE authorization = ?",
                )
                .bind(RefreshState::Current.name())
                .bind(used.turns(RefreshState::Current).name())
                .bind(RefreshState::Previous.name())
                .bind(used.turns(RefreshState::Previous).name())
                .bind(id)
                .execute(&mut *tx)
                .await?;
                let issued = Issued {
                    access_token: random_token(),
                    token_type: "Bearer",
                    expires_in: (access_until - now).num_seconds(),
                    refresh_token: random_token(),
                };
                sqlx::query(
                    "INSERT INTO refresh_tokens (hash, authorization, state) VALUES (?, ?, ?)",
                )
                .bind(hashed(&issued.refresh_token))
                .bind(id)
                .bind(RefreshState::Current.name())
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO access_tokens (hash, authorization, expires_at) VALUES (?, ?, ?)",
                )
                .bind(hashed(&issued.access_token))
                .bind(id)
                .bind(time_text(access_until))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(Ok(issued))
            }
            TokenDecision::End {
                authorization,
                error,
            } => {
                end_on(&mut tx, authorization, now).await?;
                tx.commit().await?;
                info!(
                    authorization = %authorization,
                    "ended an authorization: its client used a refresh token that had stopped working"
                );
                Ok(Err(error))
            }
            TokenDecision::Refuse(error) => Ok(Err(error)),
        }
    }
}

/// Forgets, at `now`, the codes and access tokens that expired, and the
/// refresh tokens of the authorizations that are over, which no token
/// request can use any more, in the transaction `conn` has open.
async fn forget_the_dead(conn: &mut sqlx::SqliteConnection, now: Timestamp) -> anyhow::Result<()> {
    let now = time_text(now);
    for forget in [
        "DELETE FROM codes WHERE expires_at <= ?1",
        "DELETE FROM access_tokens WHERE expires_at <= ?1",
        "DELETE FROM refresh_tokens WHERE authorization IN
            (SELECT id FROM authorizations WHERE ended_at IS NOT NULL OR ends_at <= ?1)",
    ] {
        sqlx::query(forget)
            .bind(&now)
            .execute(&mut *conn)
            .await
            .context("forgetting the codes and tokens that no longer work")?;
    }
    Ok(())
}

/// What the database holds of the code whose hash is `hash`.
async fn stored_code(conn: &mut sqlx::SqliteConnection, hash: &[u8]) -> anyhow::Result<Stored> {
    let row: Option<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT challenge, redirect_uri, expires_at, used, authorization FROM codes WHERE hash = ?",
    )
    .bind(hash)
    .fetch_optional(&mut *conn)
    .await
    .context("reading the code")?;
    let Some((challenge, redirect_uri, expires_at, used, authorization)) = row else {
        return Ok(Stored::Unknown);
    };
    Ok(Stored::Code {
        authorization: stored_authorization(conn, authorization).await?,
        challenge: stored_challenge(&challenge)?,
        redirect_uri: stored_redirect_uri(&redirect_uri)?,
        expires_at: parse_time(&expires_at)?,
        used: used != 0,
    })
}

/// What the database holds of the refresh token whose hash is `hash`.
async fn stored_refresh(conn: &mut sqlx::SqliteConnection, hash: &[u8]) -> anyhow::Result<Stored> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT state, authorization FROM refresh_tokens WHERE hash = ?")
            .bind(hash)
            .fetch_optional(&mut *conn)
            .await
            .context("reading the refresh token")?;
    let Some((state, authorization)) = row else {
        return Ok(Stored::Unknown);
    };
    Ok(Stored::Refresh {
        authorization: stored_authorization(conn, authorization).await?,
        state: RefreshState::named(&state)
            .with_context(|| format!("a refresh token has the unknown state {state:?}"))?,
    })
}

async fn stored_authorization(
    conn: &mut sqlx::SqliteConnection,
    id: i64,
) -> anyhow::Result<StoredAuthorization> {
    let (id, client, ends_at, ended_at): AuthorizationRow =
        sqlx::query_as("SELECT id, client, ends_at, ended_at FROM authorizations WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .context("reading the authorization")?;
    Ok(StoredAuthorization {
        id: AuthorizationId::of_row(id),
        client: ClientId::of(client),
        ends_at: parse_time(&ends_at)?,
        ended: ended_at.is_some(),
    })
}

/// The access tokens Linewright issued.
#[derive(Clone, Debug)]
pub struct AccessTokens {
    db: Database,
}

impl AccessTokens {
    pub fn new(db: Database) -> AccessTokens {
        AccessTokens { db }
    }

    /// Who the access token `bearer` acts as at `now`, as their record has
    /// them now: no one once it expired, its authorization ended, or its
    /// person is disabled.
    pub async fn resolve(&self, bearer: &str, now: Timestamp) -> anyhow::Result<Option<Caller>> {
        let mut conn = self.db.pool().acquire().await?;
        let row: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT t.expires_at, t.authorization, a.person FROM access_tokens t
             JOIN authorizations a ON a.id = t.authorization WHERE t.hash = ?",
        )
        .bind(hashed(bearer))
        .fetch_optional(&mut *conn)
        .await
        .context("reading the access token")?;
        let Some((expires_at, authorization, person)) = row else {
            return Ok(None);
        };
        let authorization = stored_authorization(&mut conn, authorization).await?;
        if !access_works(parse_time(&expires_at)?, &authorization, now) {
            return Ok(None);
        }
        let person = People::new(self.db.clone())
            .get(PersonId::of_row(person))
            .await?;
        Ok(person.and_then(Caller::of))
    }
}
