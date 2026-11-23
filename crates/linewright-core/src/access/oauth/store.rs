//! The authorization server's records in the database: the authorization
//! requests that have not ended, the authorizations, and the codes and
//! tokens, each kept only as its SHA-256, and found by it.

use anyhow::Context as _;
use chrono::TimeDelta;
use tracing::info;
use url::Url;

use super::clients::Registration;
use super::{
    AuthorizationId, CODE_LASTS, Challenge, Checked, ClientId, Grant, OAuthError, RedirectUri,
    RefreshState, Server, Stored, StoredAuthorization, TokenDecision, TokenRequest, Used,
    access_works, authorization_ends, exchange,
};
use crate::access::Caller;
use crate::access::people::{People, PersonId};
use crate::access::tokens::{hashed, host_cookie, random_token};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;

/// How long an authorization request lasts from when its client makes it,
/// whatever happens to it in between (`docs/specs/access.md`, The
/// authorization server, When a request ends).
const LASTS: TimeDelta = TimeDelta::minutes(10);

/// The name of the cookie that holds the handle "Allow" gives a request,
/// which only its sign-in takes, so that a client that reads Consent's
/// handle cannot skip Consent.
pub const AUTHORIZATION_COOKIE: &str = "__Host-linewright-authorization";

/// The authorization requests that have not ended: each asked, waiting for
/// Consent and found by the handle that [`AuthorizationRequests::begin`]
/// gives, and once allowed also waiting for its sign-in, found by the
/// handle that [`AuthorizationRequests::allow`] gives.
#[derive(Clone, Debug)]
pub struct AuthorizationRequests {
    db: Database,
}

/// An authorization request, as Consent shows it: its client, and where it
/// sends the browser back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub client: ClientId,
    pub client_name: Option<String>,
    /// The host of the client's ID, for a client whose ID is the URL of its
    /// metadata document.
    pub client_host: Option<String>,
    pub redirect_uri: RedirectUri,
    pub state: Option<String>,
}

/// The handle "Allow" gave a request, for its browser's
/// [`AUTHORIZATION_COOKIE`], and how long the request still lasts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub handle: String,
    pub lasts: TimeDelta,
}

impl Allowed {
    /// The `Set-Cookie` that keeps its handle in the browser as long as
    /// the request lasts.
    pub fn set_cookie(&self) -> String {
        host_cookie(AUTHORIZATION_COOKIE, &self.handle, self.lasts)
    }

    /// The `Set-Cookie` that removes it from the browser.
    pub fn removed() -> String {
        host_cookie(AUTHORIZATION_COOKIE, "", TimeDelta::zero())
    }
}

/// An authorization request that its sign-in ended: the authorization it
/// began, and the code for its client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub asked: Asked,
    pub authorization: AuthorizationId,
    pub code: String,
}

/// A request: its client, the client's name and registration, its redirect
/// URI, PKCE challenge, state, and when it ends.
type RequestRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    String,
);

/// The query of the request whose handle's hash, as the column `$by`
/// keeps it, it binds.
macro_rules! request_by {
    ($by:literal) => {
        concat!(
            "SELECT r.client, c.name, c.registration, r.redirect_uri, r.challenge, r.state,
                r.expires_at
             FROM authorization_requests r JOIN clients c ON c.id = r.client WHERE r.",
            $by,
            " = ?"
        )
    };
}

/// The request whose handle is `handle`, by the hash `query` (of
/// `request_by!`) finds it by, in `conn`: with its PKCE challenge and when
/// it ends, if it has not ended at `now`.
async fn read_request(
    conn: &mut sqlx::SqliteConnection,
    query: &'static str,
    handle: &str,
    now: Timestamp,
) -> anyhow::Result<Option<(Asked, Challenge, Timestamp)>> {
    let row: Option<RequestRow> = sqlx::query_as(query)
        .bind(hashed(handle))
        .fetch_optional(&mut *conn)
        .await
        .context("reading the authorization request")?;
    let Some((client, client_name, registration, redirect_uri, challenge, state, expires_at)) = row
    else {
        return Ok(None);
    };
    let expires_at = parse_time(&expires_at)?;
    if expires_at <= now {
        return Ok(None);
    }
    let client_host = match Registration::named(&registration) {
        Some(Registration::MetadataDocument) => Some(
            Url::parse(&client)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string))
                .with_context(|| format!("the client ID {client:?} is not a URL with a host"))?,
        ),
        Some(Registration::Dynamic) => None,
        None => anyhow::bail!("a client has the unknown registration {registration:?}"),
    };
    let asked = Asked {
        client: ClientId::of(client),
        client_name,
        client_host,
        redirect_uri: stored_redirect_uri(&redirect_uri)?,
        state,
    };
    Ok(Some((asked, stored_challenge(&challenge)?, expires_at)))
}

impl AuthorizationRequests {
    pub fn new(db: Database) -> AuthorizationRequests {
        AuthorizationRequests { db }
    }

    /// Keeps `checked`, which its client made at `now`, for 10 minutes,
    /// forgetting those that have ended, and gives the handle that Consent
    /// finds it by.
    pub async fn begin(&self, checked: &Checked, now: Timestamp) -> anyhow::Result<String> {
        let handle = random_token();
        sqlx::query("DELETE FROM authorization_requests WHERE expires_at <= ?")
            .bind(time_text(now))
            .execute(self.db.pool())
            .await
            .context("forgetting the authorization requests that ended")?;
        sqlx::query(
            "INSERT INTO authorization_requests
                (hash, client, redirect_uri, challenge, state, expires_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(hashed(&handle))
        .bind(checked.client.as_str())
        .bind(checked.redirect_uri.as_str())
        .bind(checked.challenge.as_str())
        .bind(&checked.state)
        .bind(time_text(now + LASTS))
        .execute(self.db.pool())
        .await
        .context("keeping the authorization request")?;
        Ok(handle)
    }

    /// The request Consent's `handle` finds, if it has not ended at `now`.
    pub async fn asked(&self, handle: &str, now: Timestamp) -> anyhow::Result<Option<Asked>> {
        let mut conn = self.db.pool().acquire().await?;
        let read = read_request(&mut conn, request_by!("hash"), handle, now).await?;
        Ok(read.map(|(asked, _, _)| asked))
    }

    /// Whether `allowed`, the handle "Allow" gave, finds a request that has
    /// not ended at `now`.
    pub async fn is_allowed(&self, allowed: &str, now: Timestamp) -> anyhow::Result<bool> {
        let mut conn = self.db.pool().acquire().await?;
        let read = read_request(&mut conn, request_by!("allowed_hash"), allowed, now).await?;
        Ok(read.is_some())
    }

    /// Allows the request Consent's `handle` finds, if it has not ended at
    /// `now`: it then waits for its sign-in, which only the new handle it
    /// gives finds, in place of any that an earlier "Allow" gave.
    pub async fn allow(
        &self,
        handle: &str,
        now: Timestamp,
    ) -> anyhow::Result<Option<(Asked, Allowed)>> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let Some((asked, _, expires_at)) =
            read_request(&mut tx, request_by!("hash"), handle, now).await?
        else {
            return Ok(None);
        };
        let allowed = Allowed {
            handle: random_token(),
            lasts: expires_at - now,
        };
        sqlx::query("UPDATE authorization_requests SET allowed_hash = ? WHERE hash = ?")
            .bind(hashed(&allowed.handle))
            .bind(hashed(handle))
            .execute(&mut *tx)
            .await
            .context("allowing the authorization request")?;
        tx.commit().await?;
        Ok(Some((asked, allowed)))
    }

    /// Ends the request Consent's `handle` finds, its client denied, and
    /// gives it, if it had not ended at `now`.
    pub async fn deny(&self, handle: &str, now: Timestamp) -> anyhow::Result<Option<Asked>> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let denied = read_request(&mut tx, request_by!("hash"), handle, now)
            .await?
            .map(|(asked, _, _)| asked);
        sqlx::query("DELETE FROM authorization_requests WHERE hash = ?")
            .bind(hashed(handle))
            .execute(&mut *tx)
            .await
            .context("ending the authorization request")?;
        tx.commit().await?;
        Ok(denied)
    }

    /// Ends the allowed request that `allowed`, the handle "Allow" gave,
    /// finds, with `person` signed in for it at `now`, if it had not ended
    /// and they are enabled: records the authorization it begins, for the
    /// MCP server of `server`, which ends as [`authorization_ends`] says,
    /// and the code its client exchanges for its tokens. One transaction
    /// reads and writes, so that the request ends once and disabling the
    /// person cannot come between.
    pub async fn sign_in(
        &self,
        allowed: &str,
        person: PersonId,
        server: &Server,
        now: Timestamp,
    ) -> anyhow::Result<Option<SignedIn>> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let Some((asked, challenge, _)) =
            read_request(&mut tx, request_by!("allowed_hash"), allowed, now).await?
        else {
            return Ok(None);
        };
        let enabled: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM people WHERE id = ? AND state = 'enabled'")
                .bind(person.row())
                .fetch_optional(&mut *tx)
                .await
                .context("reading the person")?;
        if enabled.is_none() {
            return Ok(None);
        }
        sqlx::query("DELETE FROM authorization_requests WHERE allowed_hash = ?")
            .bind(hashed(allowed))
            .execute(&mut *tx)
            .await
            .context("ending the authorization request")?;
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO authorizations
                (person, client, redirect_uri, resource, allowed_at, ends_at)
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(person.row())
        .bind(asked.client.as_str())
        .bind(asked.redirect_uri.as_str())
        .bind(server.resource().as_str())
        .bind(time_text(now))
        .bind(time_text(authorization_ends(now)))
        .fetch_one(&mut *tx)
        .await
        .context("recording the authorization")?;
        let code = random_token();
        sqlx::query(
            "INSERT INTO codes
                (hash, authorization, challenge, redirect_uri, expires_at, used)
             VALUES (?, ?, ?, ?, ?, 0)",
        )
        .bind(hashed(&code))
        .bind(id)
        .bind(challenge.as_str())
        .bind(asked.redirect_uri.as_str())
        .bind(time_text(now + CODE_LASTS))
        .execute(&mut *tx)
        .await
        .context("keeping the code")?;
        tx.commit().await?;
        Ok(Some(SignedIn {
            asked,
            authorization: AuthorizationId::of_row(id),
            code,
        }))
    }
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

/// Ends every authorization of `person` at `now`, in the transaction `conn`
/// has open.
pub(crate) async fn end_all_of(
    conn: &mut sqlx::SqliteConnection,
    person: PersonId,
    now: Timestamp,
) -> anyhow::Result<()> {
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
    /// refresh token is used once, and a second use of it ends its
    /// authorization.
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
                match used {
                    Used::Code => sqlx::query("UPDATE codes SET used = 1 WHERE hash = ?"),
                    Used::Refresh => {
                        sqlx::query("UPDATE refresh_tokens SET state = ? WHERE hash = ?")
                            .bind(RefreshState::Used.name())
                    }
                }
                .bind(&presented)
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
                    why = %error.error_description,
                    "ended an authorization"
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
        drop(conn);
        if !access_works(parse_time(&expires_at)?, &authorization, now) {
            return Ok(None);
        }
        let person = People::new(self.db.clone())
            .get(PersonId::of_row(person))
            .await?;
        Ok(person.and_then(Caller::of))
    }
}
