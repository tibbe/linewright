//! A fake of Google's sign-in, served over HTTP on a port of `127.0.0.1`,
//! for the tests and `serve-fake`: a page to choose one of its accounts,
//! which sends the browser back with a code, and a token endpoint that
//! exchanges the code, checked as Google checks it, for an ID token.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use serde::Deserialize;
use url::Url;

use crate::access::providers::GoogleEndpoints;
use crate::access::tokens::{random_token, s256};
use crate::clock::Clock;
use crate::html::escaped;
use crate::listening::Listening;

/// An account the fake signs in.
#[derive(Clone, Debug)]
pub struct FakeAccount {
    pub subject: String,
    pub email: String,
    pub email_verified: bool,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// The fake, serving until it is dropped or stopped.
pub struct FakeGoogle {
    address: SocketAddr,
    listening: Listening,
}

/// What the fake knows: its accounts, and the codes it gave.
struct FakeState {
    accounts: Vec<FakeAccount>,
    issuer: String,
    clock: Arc<dyn Clock>,
    /// The codes not yet exchanged.
    codes: Mutex<HashMap<String, Granted>>,
}

/// What a code was given for.
struct Granted {
    account: FakeAccount,
    client_id: String,
    redirect_uri: String,
    challenge: String,
    nonce: Option<String>,
}

impl FakeGoogle {
    /// Serves the fake with `accounts`, its ID tokens issued at the time
    /// `clock` tells.
    pub async fn serve(
        accounts: Vec<FakeAccount>,
        clock: Arc<dyn Clock>,
    ) -> anyhow::Result<FakeGoogle> {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?;
        let fake = Arc::new(FakeState {
            accounts,
            issuer: format!("http://{address}"),
            clock,
            codes: Mutex::new(HashMap::new()),
        });
        let router = Router::new()
            .route("/authorize", get(authorize))
            .route("/choose", get(choose))
            .route("/token", post(token))
            .with_state(fake);
        let listening = Listening::spawn(socket, router, "serving the fake Google");
        Ok(FakeGoogle { address, listening })
    }

    /// Its endpoints, for Linewright to sign in with it.
    pub fn endpoints(&self) -> GoogleEndpoints {
        let url = |path: &str| {
            Url::parse(&format!("http://{}{path}", self.address)).expect("the fake's URL parses")
        };
        GoogleEndpoints {
            authorize: url("/authorize"),
            token: url("/token"),
            issuers: vec![format!("http://{}", self.address)],
        }
    }

    /// Where a browser sent to `authorize`, its authorization request,
    /// goes when it chooses the account of `email`, or turns the sign-in
    /// down with `None`.
    pub fn choosing(authorize: &Url, email: Option<&str>) -> Url {
        let mut url = authorize.clone();
        url.set_path("/choose");
        if let Some(email) = email {
            url.query_pairs_mut().append_pair("account", email);
        }
        url
    }

    pub async fn stop(self) {
        self.listening.stop().await;
    }
}

/// An authorization request, as Google takes it.
/// <https://developers.google.com/identity/openid-connect/openid-connect#sendauthrequest>
#[derive(Deserialize)]
struct Authorize {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    nonce: Option<String>,
    /// The account chosen, none to turn the sign-in down.
    account: Option<String>,
}

/// The page to choose an account on.
async fn authorize(
    State(fake): State<Arc<FakeState>>,
    Query(request): Query<Authorize>,
) -> Response {
    if let Some(refused) = refused(&request) {
        return refused;
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &request.client_id)
        .append_pair("redirect_uri", &request.redirect_uri)
        .append_pair("response_type", &request.response_type)
        .append_pair("state", &request.state)
        .append_pair("code_challenge", &request.code_challenge)
        .append_pair("code_challenge_method", &request.code_challenge_method)
        .append_pair("nonce", request.nonce.as_deref().unwrap_or_default())
        .finish();
    let accounts: String = fake
        .accounts
        .iter()
        .map(|account| {
            let chosen = form_urlencoded::Serializer::new(query.clone())
                .append_pair("account", &account.email)
                .finish();
            format!(
                r#"<li><a href="/choose?{}">{} ({})</a></li>"#,
                escaped(&chosen),
                escaped(account.name.as_deref().unwrap_or("No name")),
                escaped(&account.email),
            )
        })
        .collect();
    Html(format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>Fake Google</title></head><body><h1>Choose an account</h1>\
         <p>A fake of Google's sign-in.</p><ul>{accounts}</ul>\
         <p><a href=\"/choose?{}\">Cancel</a></p></body></html>",
        escaped(&query)
    ))
    .into_response()
}

/// Sends the browser back with a code for the account it chose, or with
/// `access_denied` when it chose none.
async fn choose(State(fake): State<Arc<FakeState>>, Query(request): Query<Authorize>) -> Response {
    if let Some(refused) = refused(&request) {
        return refused;
    }
    let Ok(mut back) = Url::parse(&request.redirect_uri) else {
        return (StatusCode::BAD_REQUEST, "redirect_uri is no URL").into_response();
    };
    let account = request.account.as_ref().and_then(|email| {
        fake.accounts
            .iter()
            .find(|account| &account.email == email)
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
                    nonce: request.nonce.clone().filter(|nonce| !nonce.is_empty()),
                },
            );
            back.query_pairs_mut()
                .append_pair("code", &code)
                .append_pair("state", &request.state);
        }
        None => {
            back.query_pairs_mut()
                .append_pair("error", "access_denied")
                .append_pair("state", &request.state);
        }
    }
    Redirect::to(back.as_str()).into_response()
}

/// The answer to an authorization request Google would refuse, if it is one.
fn refused(request: &Authorize) -> Option<Response> {
    let why = if request.response_type != "code" {
        "response_type is not code"
    } else if request.code_challenge_method != "S256" {
        "code_challenge_method is not S256"
    } else if request.client_id.is_empty() {
        "client_id is empty"
    } else {
        return None;
    };
    Some((StatusCode::BAD_REQUEST, why).into_response())
}

/// A token request.
/// <https://developers.google.com/identity/openid-connect/openid-connect#exchangecode>
#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    code_verifier: String,
}

/// The ID token of a code, once, when the request matches what the code was
/// given for.
async fn token(State(fake): State<Arc<FakeState>>, body: axum::body::Bytes) -> Response {
    let invalid = |error: &str| {
        (
            StatusCode::BAD_REQUEST,
            json(&serde_json::json!({ "error": error })),
        )
            .into_response()
    };
    let Ok(request) = serde_urlencoded::from_bytes::<TokenRequest>(&body) else {
        return invalid("invalid_request");
    };
    let granted = fake
        .codes
        .lock()
        .expect("the codes' lock")
        .remove(&request.code);
    let Some(granted) = granted else {
        return invalid("invalid_grant");
    };
    if request.grant_type != "authorization_code"
        || request.client_id != granted.client_id
        || request.client_secret.is_empty()
        || request.redirect_uri != granted.redirect_uri
        || s256(&request.code_verifier) != granted.challenge
    {
        return invalid("invalid_grant");
    }
    let now = fake.clock.now().timestamp();
    let account = granted.account;
    let mut claims = serde_json::json!({
        "iss": fake.issuer,
        "azp": granted.client_id,
        "aud": granted.client_id,
        "sub": account.subject,
        "email": account.email,
        "email_verified": account.email_verified,
        "iat": now,
        "exp": now + 3600,
    });
    for (name, value) in [
        ("nonce", granted.nonce),
        ("name", account.name),
        ("picture", account.picture),
    ] {
        if let Some(value) = value {
            claims[name] = value.into();
        }
    }
    let part = |value: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
    };
    let id_token = format!(
        "{}.{}.fake-signature",
        part(&serde_json::json!({"alg": "RS256", "kid": "fake", "typ": "JWT"})),
        part(&claims)
    );
    json(&serde_json::json!({
        "access_token": random_token(),
        "expires_in": 3599,
        "scope": "openid email profile",
        "token_type": "Bearer",
        "id_token": id_token,
    }))
    .into_response()
}

/// `value` as a JSON body.
fn json(value: &serde_json::Value) -> impl IntoResponse + use<> {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
}
