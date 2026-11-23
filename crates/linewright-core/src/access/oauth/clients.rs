//! The clients Linewright knows (`docs/specs/access.md`, The authorization
//! server, Clients): those it registered, and those whose metadata document
//! it fetched, kept for as long as the document stays fresh.

use anyhow::Context as _;
use hyper_util::client::proxy::matcher::Matcher;
use tracing::{info, warn};

use super::metadata_document::{self, DocumentUrl};
use super::store::stored_redirect_uri;
use super::{Client, ClientId, RedirectUri, client_name};
use crate::access::tokens::random_token;
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;

/// The clients, in the database.
#[derive(Clone, Debug)]
pub struct Clients {
    db: Database,
}

/// A client's name, redirect URIs, and until when its metadata document
/// is fresh, where it has one.
type ClientRow = (Option<String>, String, Option<String>);

/// How Linewright came to know a client.
#[derive(Clone, Copy)]
enum Registration {
    Dynamic,
    MetadataDocument,
}

impl Registration {
    fn name(self) -> &'static str {
        match self {
            Registration::Dynamic => "dynamic",
            Registration::MetadataDocument => "metadata-document",
        }
    }
}

/// Whether a client whose metadata document is fresh until `fresh_until`,
/// or that has none, needs no fetch at `now`.
fn fresh(fresh_until: Option<Timestamp>, now: Timestamp) -> bool {
    fresh_until.is_none_or(|until| until > now)
}

impl Clients {
    pub fn new(db: Database) -> Clients {
        Clients { db }
    }

    /// Registers a public client named `name`, with `redirect_uris`, at
    /// `now`, giving it an ID of its own; and logs it.
    pub async fn register(
        &self,
        name: Option<String>,
        redirect_uris: Vec<RedirectUri>,
        now: Timestamp,
    ) -> anyhow::Result<Client> {
        let client = Client {
            id: ClientId::of(random_token()),
            name,
            redirect_uris,
        };
        self.keep(&client, Registration::Dynamic, now, None)
            .await
            .context("registering a client")?;
        info!(
            client = %client.id,
            name = client_name(client.name.as_deref()),
            "registered a client"
        );
        Ok(client)
    }

    /// The client `id` names at `now`, if Linewright knows it: one it
    /// registered, or the one whose metadata document `id` is the URL of,
    /// fetched again once it is no longer fresh. A fetch that fails, which
    /// it logs, names no client.
    pub async fn find(&self, id: &str, now: Timestamp) -> anyhow::Result<Option<Client>> {
        let row: Option<ClientRow> =
            sqlx::query_as("SELECT name, redirect_uris, fresh_until FROM clients WHERE id = ?")
                .bind(id)
                .fetch_optional(self.db.pool())
                .await
                .context("reading a client")?;
        let known = row.is_some();
        if let Some((name, uris, fresh_until)) = row {
            let fresh_until = fresh_until.as_deref().map(parse_time).transpose()?;
            if fresh(fresh_until, now) {
                return Ok(Some(Client {
                    id: ClientId::of(id.to_string()),
                    name,
                    redirect_uris: uris_of(&uris)?,
                }));
            }
        }
        let Some(url) = DocumentUrl::parse(id) else {
            return Ok(None);
        };
        let (client, fresh_for) = match metadata_document::fetch(&url, &Matcher::from_env()).await {
            Ok(fetched) => fetched,
            Err(e) => {
                warn!(
                    client = id,
                    error = format!("{e:#}"),
                    "fetching a client's metadata document failed"
                );
                return Ok(None);
            }
        };
        self.keep(
            &client,
            Registration::MetadataDocument,
            now,
            Some(now + fresh_for),
        )
        .await
        .context("keeping a client's metadata document")?;
        if !known {
            info!(
                client = %client.id,
                name = client_name(client.name.as_deref()),
                "registered a client by its metadata document"
            );
        }
        Ok(Some(client))
    }

    /// Keeps `client`, known by `registration` at `now`, and fresh until
    /// `fresh_until` where it has a metadata document, in place of what
    /// was kept of it.
    async fn keep(
        &self,
        client: &Client,
        registration: Registration,
        now: Timestamp,
        fresh_until: Option<Timestamp>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO clients (id, registration, name, redirect_uris, registered_at, fresh_until)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                name = excluded.name,
                redirect_uris = excluded.redirect_uris,
                fresh_until = excluded.fresh_until",
        )
        .bind(client.id.as_str())
        .bind(registration.name())
        .bind(&client.name)
        .bind(uris_text(&client.redirect_uris))
        .bind(time_text(now))
        .bind(fresh_until.map(time_text))
        .execute(self.db.pool())
        .await?;
        Ok(())
    }
}

/// `uris` as the database keeps them: a JSON array.
fn uris_text(uris: &[RedirectUri]) -> String {
    serde_json::Value::from(uris.iter().map(RedirectUri::as_str).collect::<Vec<_>>()).to_string()
}

/// The redirect URIs `text`, as [`uris_text`] gives them, holds.
fn uris_of(text: &str) -> anyhow::Result<Vec<RedirectUri>> {
    let uris: Vec<String> = serde_json::from_str(text).context("reading redirect URIs")?;
    uris.iter().map(|uri| stored_redirect_uri(uri)).collect()
}
