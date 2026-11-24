//! A fake of GitHub's OAuth sign-in and of the part of its REST API that
//! Linewright reads with the token: a page to choose one of its accounts,
//! which sends the browser back with a code; a token endpoint that
//! exchanges the code, checked as GitHub checks it, for a token, answering
//! an error with 200 OK as GitHub does; the user and their memberships,
//! read with the token; and the token's revocation.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use base64::Engine as _;
use serde::Deserialize;
use url::Url;

use crate::access::providers::GitHubEndpoints;
use crate::access::tokens::{random_token, s256};
use crate::html::escaped;
use crate::listening::Listening;

/// An account the fake signs in.
#[derive(Clone, Debug)]
pub struct FakeGitHubAccount {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    /// The organizations and teams it is a member of, each `org` or
    /// `org/team`, by the team's slug.
    pub memberships: Vec<String>,
}

/// The fake, serving until it is dropped or stopped.
pub struct FakeGitHub {
    address: SocketAddr,
    state: Arc<FakeState>,
    listening: Listening,
}

/// What the fake knows: its accounts, and the codes and tokens it gave.
struct FakeState {
    accounts: Vec<FakeGitHubAccount>,
    /// The codes not yet exchanged.
    codes: Mutex<HashMap<String, Granted>>,
    /// The tokens not yet revoked.
    tokens: Mutex<HashMap<String, Granted>>,
}

/// What a code, and then its token, was given for.
#[derive(Clone)]
struct Granted {
    account: FakeGitHubAccount,
    client_id: String,
    redirect_uri: String,
    challenge: String,
    /// Whether the person granted `read:org`.
    read_org: bool,
}

/// GitHub's own way of saying that a request needs a scope the token lacks,
/// or a token it does not know.
/// <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api>
fn refused(status: StatusCode, message: &str) -> Response {
    (
        status,
        json(&serde_json::json!({
            "message": message,
            "documentation_url": "https://docs.github.com/rest",
            "status": status.as_u16().to_string(),
        })),
    )
        .into_response()
}

impl FakeGitHub {
    /// Serves the fake with `accounts`.
    pub async fn serve(accounts: Vec<FakeGitHubAccount>) -> anyhow::Result<FakeGitHub> {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?;
        let state = Arc::new(FakeState {
            accounts,
            codes: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        });
        let router = Router::new()
            .route("/login/oauth/authorize", get(authorize))
            .route("/choose", get(choose))
            .route("/login/oauth/access_token", post(token))
            .route("/api/user", get(user))
            .route("/api/user/orgs", get(organizations))
            .route(
                "/api/orgs/{org}/teams/{team_slug}/memberships/{username}",
                get(team_membership),
            )
            .route("/api/applications/{client_id}/token", delete(revoke))
            .with_state(state.clone());
        let listening = Listening::spawn(socket, router, "serving the fake GitHub");
        Ok(FakeGitHub {
            address,
            state,
            listening,
        })
    }

    /// Its endpoints, for Linewright to sign in with it.
    pub fn endpoints(&self) -> GitHubEndpoints {
        let url = |path: &str| {
            Url::parse(&format!("http://{}{path}", self.address)).expect("the fake's URL parses")
        };
        GitHubEndpoints {
            authorize: url("/login/oauth/authorize"),
            token: url("/login/oauth/access_token"),
            api: url("/api/"),
        }
    }

    /// Where a browser sent to `authorize`, its authorization request,
    /// goes when it chooses the account of `login`, or turns the sign-in
    /// down with `None`.
    pub fn choosing(authorize: &Url, login: Option<&str>) -> Url {
        let mut url = authorize.clone();
        url.set_path("/choose");
        if let Some(login) = login {
            url.query_pairs_mut().append_pair("account", login);
        }
        url
    }

    /// How many of the tokens it gave are not revoked.
    pub fn tokens_not_revoked(&self) -> usize {
        self.state.tokens.lock().expect("the tokens' lock").len()
    }

    pub async fn stop(self) {
        self.listening.stop().await;
    }
}

/// An authorization request, as GitHub takes it.
/// <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#1-request-a-users-github-identity>
#[derive(Deserialize)]
struct Authorize {
    client_id: String,
    redirect_uri: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    #[serde(default)]
    scope: String,
    prompt: Option<String>,
    /// The account chosen, none to turn the sign-in down.
    account: Option<String>,
}

impl Authorize {
    /// Its query, without the account chosen.
    fn query(&self) -> String {
        let mut query = form_urlencoded::Serializer::new(String::new());
        query
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("state", &self.state)
            .append_pair("code_challenge", &self.code_challenge)
            .append_pair("code_challenge_method", &self.code_challenge_method)
            .append_pair("scope", &self.scope);
        if let Some(prompt) = &self.prompt {
            query.append_pair("prompt", prompt);
        }
        query.finish()
    }

    /// The answer to it where GitHub would refuse it, if GitHub would.
    fn refused(&self) -> Option<Response> {
        let why = if self.code_challenge_method != "S256" {
            "code_challenge_method is not S256"
        } else if self.client_id.is_empty() {
            "client_id is empty"
        } else {
            return None;
        };
        Some((StatusCode::BAD_REQUEST, why).into_response())
    }

    fn read_org(&self) -> bool {
        self.scope
            .split([' ', ','])
            .any(|scope| scope == "read:org")
    }
}

/// The page to choose an account on.
async fn authorize(
    State(fake): State<Arc<FakeState>>,
    Query(request): Query<Authorize>,
) -> Response {
    if let Some(refused) = request.refused() {
        return refused;
    }
    let query = request.query();
    let accounts: String = fake
        .accounts
        .iter()
        .map(|account| {
            let chosen = form_urlencoded::Serializer::new(query.clone())
                .append_pair("account", &account.login)
                .finish();
            format!(
                r#"<li><a href="/choose?{}">{} ({})</a></li>"#,
                escaped(&chosen),
                escaped(account.name.as_deref().unwrap_or("No name")),
                escaped(&account.login),
            )
        })
        .collect();
    let scope = if request.read_org() {
        "It asks to read your organizations and teams (read:org)."
    } else {
        "It asks for no scope."
    };
    Html(format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>Fake GitHub</title></head><body><h1>Choose an account</h1>\
         <p>A fake of GitHub's sign-in. {scope}</p><ul>{accounts}</ul>\
         <p><a href=\"/choose?{}\">Cancel</a></p></body></html>",
        escaped(&query)
    ))
    .into_response()
}

/// Sends the browser back with a code for the account it chose, or with
/// `access_denied` when it chose none.
/// <https://docs.github.com/en/apps/oauth-apps/maintaining-oauth-apps/troubleshooting-authorization-request-errors#access-denied>
async fn choose(State(fake): State<Arc<FakeState>>, Query(request): Query<Authorize>) -> Response {
    if let Some(refused) = request.refused() {
        return refused;
    }
    let Ok(mut back) = Url::parse(&request.redirect_uri) else {
        return (StatusCode::BAD_REQUEST, "redirect_uri is no URL").into_response();
    };
    let account = request.account.as_ref().and_then(|login| {
        fake.accounts
            .iter()
            .find(|account| account.login.eq_ignore_ascii_case(login))
            .cloned()
    });
    match account {
        Some(account) => {
            let code = random_token();
            fake.codes.lock().expect("the codes' lock").insert(
                code.clone(),
                Granted {
                    account,
                    client_id: request.client_id.clone(),
                    redirect_uri: request.redirect_uri.clone(),
                    challenge: request.code_challenge.clone(),
                    read_org: request.read_org(),
                },
            );
            back.query_pairs_mut()
                .append_pair("code", &code)
                .append_pair("state", &request.state);
        }
        None => {
            back.query_pairs_mut()
                .append_pair("error", "access_denied")
                .append_pair(
                    "error_description",
                    "The user has denied your application access.",
                )
                .append_pair("state", &request.state);
        }
    }
    Redirect::to(back.as_str()).into_response()
}

/// A token request.
/// <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#2-users-are-redirected-back-to-your-site-by-github>
#[derive(Deserialize)]
struct TokenRequest {
    client_id: String,
    client_secret: String,
    code: String,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
}

/// The token of a code, once, when the request matches what the code was
/// given for; else GitHub's error, with 200 OK, as GitHub answers it.
/// <https://docs.github.com/en/apps/oauth-apps/maintaining-oauth-apps/troubleshooting-oauth-app-access-token-request-errors>
async fn token(State(fake): State<Arc<FakeState>>, body: axum::body::Bytes) -> Response {
    let error = |error: &str, description: &str| {
        json(&serde_json::json!({
            "error": error,
            "error_description": description,
            "error_uri": "https://docs.github.com/apps/managing-oauth-apps/troubleshooting-oauth-app-access-token-request-errors/",
        }))
        .into_response()
    };
    let Ok(request) = serde_urlencoded::from_bytes::<TokenRequest>(&body) else {
        return error(
            "incorrect_client_credentials",
            "The client_id and/or client_secret passed are incorrect.",
        );
    };
    let granted = fake
        .codes
        .lock()
        .expect("the codes' lock")
        .remove(&request.code);
    let Some(granted) = granted else {
        return error(
            "bad_verification_code",
            "The code passed is incorrect or expired.",
        );
    };
    if request.client_id != granted.client_id || request.client_secret.is_empty() {
        return error(
            "incorrect_client_credentials",
            "The client_id and/or client_secret passed are incorrect.",
        );
    }
    if request
        .redirect_uri
        .is_some_and(|uri| uri != granted.redirect_uri)
    {
        return error(
            "redirect_uri_mismatch",
            "The redirect_uri MUST match the registered callback URL for this application.",
        );
    }
    if request.code_verifier.map(|verifier| s256(&verifier)) != Some(granted.challenge.clone()) {
        return error(
            "bad_verification_code",
            "The code passed is incorrect or expired.",
        );
    }
    let token = format!("gho_{}", random_token());
    let scope = if granted.read_org { "read:org" } else { "" };
    fake.tokens
        .lock()
        .expect("the tokens' lock")
        .insert(token.clone(), granted);
    json(&serde_json::json!({
        "access_token": token,
        "scope": scope,
        "token_type": "bearer",
    }))
    .into_response()
}

/// What the token of the request's `Authorization` was given for, if it is
/// a token the fake gave and did not revoke.
fn granted(fake: &FakeState, headers: &HeaderMap) -> Option<Granted> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|token| {
            fake.tokens
                .lock()
                .expect("the tokens' lock")
                .get(token)
                .cloned()
        })
}

/// GitHub's answer to a request without a token it knows.
fn bad_credentials() -> Response {
    refused(StatusCode::UNAUTHORIZED, "Bad credentials")
}

/// The organizations of `account`, each once, from its memberships of
/// them and of their teams.
fn organizations_of(account: &FakeGitHubAccount) -> Vec<&str> {
    let mut organizations: Vec<&str> = vec![];
    for membership in &account.memberships {
        let organization = membership.split('/').next().unwrap_or_default();
        if !organizations.contains(&organization) {
            organizations.push(organization);
        }
    }
    organizations
}

/// `GET /user`, as GitHub answers it.
/// <https://docs.github.com/en/rest/users/users#get-the-authenticated-user>
async fn user(State(fake): State<Arc<FakeState>>, headers: HeaderMap) -> Response {
    let Some(Granted { account, .. }) = granted(&fake, &headers) else {
        return bad_credentials();
    };
    json(&serde_json::json!({
        "login": account.login,
        "id": account.id,
        "avatar_url": account.avatar_url,
        "html_url": format!("https://github.com/{}", account.login),
        "type": "User",
        "site_admin": false,
        "name": account.name,
    }))
    .into_response()
}

/// `GET /user/orgs`, which lists every membership with `read:org`, and
/// without it only the public ones, of which the fake's accounts have none.
/// <https://docs.github.com/en/rest/orgs/orgs#list-organizations-for-the-authenticated-user>
async fn organizations(State(fake): State<Arc<FakeState>>, headers: HeaderMap) -> Response {
    let Some(granted) = granted(&fake, &headers) else {
        return bad_credentials();
    };
    let listed = if granted.read_org {
        organizations_of(&granted.account)
    } else {
        vec![]
    };
    let organizations: Vec<serde_json::Value> = listed
        .into_iter()
        .map(|login| serde_json::json!({ "login": login }))
        .collect();
    json(&serde_json::Value::Array(organizations)).into_response()
}

/// `GET /orgs/{org}/teams/{team_slug}/memberships/{username}`, which needs
/// `read:org`: an active membership for a team among the account's
/// memberships, and 404 for any other team or person.
/// <https://docs.github.com/en/rest/teams/members#get-team-membership-for-a-user>
async fn team_membership(
    State(fake): State<Arc<FakeState>>,
    Path((organization, slug, username)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(granted) = granted(&fake, &headers) else {
        return bad_credentials();
    };
    if !granted.read_org {
        return refused(
            StatusCode::FORBIDDEN,
            "Resource not accessible by integration",
        );
    }
    let account = &granted.account;
    let member = account.login.eq_ignore_ascii_case(&username)
        && account.memberships.iter().any(|membership| {
            membership.split_once('/').is_some_and(|(org, team)| {
                org.eq_ignore_ascii_case(&organization) && team.eq_ignore_ascii_case(&slug)
            })
        });
    if !member {
        return refused(StatusCode::NOT_FOUND, "Not Found");
    }
    json(&serde_json::json!({
        "url": format!("https://api.github.com/teams/1/memberships/{}", account.login),
        "role": "member",
        "state": "active",
    }))
    .into_response()
}

#[derive(Deserialize)]
struct Revoke {
    access_token: String,
}

/// Revokes a token, for the client of the request's basic authentication.
/// <https://docs.github.com/en/rest/apps/oauth-applications#delete-an-app-token>
async fn revoke(
    State(fake): State<Arc<FakeState>>,
    Path(client_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let credentials = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .and_then(|decoded| String::from_utf8(decoded).ok());
    let authenticated = credentials
        .as_deref()
        .and_then(|credentials| credentials.split_once(':'))
        .is_some_and(|(id, secret)| id == client_id && !secret.is_empty());
    if !authenticated {
        return refused(StatusCode::NOT_FOUND, "Not Found");
    }
    let Ok(request) = serde_json::from_slice::<Revoke>(&body) else {
        return refused(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request.");
    };
    let mut tokens = fake.tokens.lock().expect("the tokens' lock");
    match tokens.get(&request.access_token) {
        Some(granted) if granted.client_id == client_id => {
            tokens.remove(&request.access_token);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => refused(StatusCode::NOT_FOUND, "Not Found"),
    }
}

/// `value` as a JSON body.
fn json(value: &serde_json::Value) -> impl IntoResponse + use<> {
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        value.to_string(),
    )
}
