//! An MCP client's authorization, as the authorization server gives it
//! (`docs/specs/access.md`, The authorization server).

use form_urlencoded::Serializer;

use super::http::ui;
use crate::api::Sources;

/// The redirect URI the client of these tests registers.
pub const REDIRECT_URI: &str = "http://127.0.0.1:5000/cb";

/// RFC 7636's example: its verifier and its challenge.
/// <https://www.rfc-editor.org/rfc/rfc7636#appendix-B>
pub const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
pub const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// The MCP server's URI under the tests' public URL.
pub const RESOURCE: &str = "http://127.0.0.1:8081/mcp";

/// The path that sends a person to authorize `client`, as a client sends it.
pub fn authorize_path(client: &str, redirect_uri: &str, state: &str) -> String {
    let mut query = Serializer::new(String::new());
    query.extend_pairs([
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", redirect_uri),
        ("code_challenge", CHALLENGE),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("resource", RESOURCE),
    ]);
    format!("/oauth/authorize?{}", query.finish())
}

/// The tokens a token request was issued.
pub struct Tokens {
    pub access: String,
    pub refresh: String,
}

/// Authorizes a new client named `name` for the person whose browser
/// `cookie` signs in to `sources`, by the back door: the client's ID and
/// its tokens.
#[track_caller]
pub fn authorized_by_back_door<'b>(
    sources: &'b Sources,
    cookie: &'b str,
    name: &'b str,
) -> impl Future<Output = (String, Tokens)> + 'b {
    use crate::access::oauth::clients::{Clients, Found};
    use crate::access::oauth::store::{AuthorizationRequests, Tokens as Issuing};
    use crate::access::oauth::{
        AuthorizeRequest, RedirectUri, Server, TokenForm, TokenRequest, check_authorize,
    };
    use crate::access::sessions::{Resolved, SESSION_COOKIE, SessionCookie, Sessions};
    let caller = std::panic::Location::caller();
    async move {
        let database = sources.runs.database().clone();
        let now = sources.clock.now();
        let session = SessionCookie::of(
            cookie
                .strip_prefix(&format!("{SESSION_COOKIE}="))
                .unwrap_or_else(|| panic!("a session cookie at {caller}"))
                .to_string(),
        );
        let Ok(Resolved::Caller(person)) =
            Sessions::new(database.clone()).resolve(&session, now).await
        else {
            panic!("no one signed in at {caller}");
        };
        let server = Server::new(ui().public_url);
        let client = Clients::new(database.clone())
            .register(
                Some(name.to_string()).filter(|name| !name.is_empty()),
                vec![RedirectUri::parse(REDIRECT_URI).unwrap()],
                now,
            )
            .await
            .unwrap_or_else(|e| panic!("registering {name} at {caller}: {e:#}"));
        let path = authorize_path(client.id.as_str(), REDIRECT_URI, "s");
        let request = AuthorizeRequest::from_query(path.split_once('?').unwrap().1);
        let checked = check_authorize(&request, &Found::Client(client.clone()), &server)
            .unwrap_or_else(|e| panic!("authorizing at {caller}: {e:?}"));
        let requests = AuthorizationRequests::new(database.clone());
        let handle = requests.begin(&checked, now).await.unwrap();
        let (_, allowed) = requests
            .allow(&handle, now)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("not allowed at {caller}"));
        let Some(signed_in) = requests
            .sign_in(&allowed.handle, person.person().id, &server, now)
            .await
            .unwrap()
        else {
            panic!("not signed in at {caller}");
        };
        let code = signed_in.code;
        let form: TokenForm = serde_urlencoded::from_str(
            &serde_urlencoded::to_string([
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", REDIRECT_URI),
                ("code_verifier", VERIFIER),
                ("client_id", client.id.as_str()),
            ])
            .unwrap(),
        )
        .unwrap();
        let issued = Issuing::new(database)
            .answer(&TokenRequest::parse(form).unwrap(), &server, now)
            .await
            .unwrap()
            .unwrap_or_else(|e| panic!("issuing tokens at {caller}: {e}"));
        let tokens = Tokens {
            access: issued.access_token,
            refresh: issued.refresh_token,
        };
        (client.id.as_str().to_string(), tokens)
    }
}

/// Keeps the client named `name`, whose metadata document at `id` gives
/// `redirect_uri`, as though Linewright had just fetched it into `sources`.
#[track_caller]
pub fn metadata_document_client<'b>(
    sources: &'b Sources,
    id: &'b str,
    name: &'b str,
    redirect_uri: &'b str,
) -> impl Future<Output = ()> + 'b {
    let caller = std::panic::Location::caller();
    async move {
        let now = sources.clock.now();
        sqlx::query(
        "INSERT INTO clients (id, registration, name, redirect_uris, registered_at, fresh_until)
         VALUES (?, 'metadata-document', ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(serde_json::json!([redirect_uri]).to_string())
    .bind(crate::database::time_text(now))
    .bind(crate::database::time_text(now + chrono::TimeDelta::days(1)))
    .execute(sources.runs.database().pool())
    .await
    .unwrap_or_else(|e| panic!("keeping the client {id} at {caller}: {e}"));
    }
}
