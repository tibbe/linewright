//! Linewright as the OAuth authorization server of MCP clients
//! (`docs/specs/access.md`, The authorization server, Tokens, Consent,
//! Authorization error, Authorizations and End authorization;
//! `docs/specs/web/access.md`, Consent, Authorization error and
//! Authorizations), through the router, signing people in with the fake
//! Google.

use linewright_core::access::oauth::store::AccessTokens;

use linewright_core::access::providers::Provider;

use super::sign_in::{Answer, Browsing, Signing, signing};
use super::*;
use linewright_core::testing::oauth::*;

/// What `answer` holds, as JSON.
#[track_caller]
fn json(answer: &Answer) -> serde_json::Value {
    let body = answer.html.root_element().text().collect::<String>();
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

impl Browsing<'_> {
    /// Posts `body` as JSON to `path`, as a client does, from no page.
    async fn post_json(&mut self, path: &str, body: &serde_json::Value) -> Answer {
        let request = axum::http::Request::post(path).header("Content-Type", "application/json");
        self.send(request, body.to_string()).await
    }

    /// Posts the form `fields` to `path`, as a client does, from no page.
    async fn post_as_client(&mut self, path: &str, fields: &[(&str, &str)]) -> Answer {
        let mut form = form_urlencoded::Serializer::new(String::new());
        form.extend_pairs(fields);
        let request = axum::http::Request::post(path)
            .header("Content-Type", "application/x-www-form-urlencoded");
        self.send(request, form.finish()).await
    }

    /// Registers a client named `name`, with [`REDIRECT_URI`]: its ID.
    #[track_caller]
    pub(super) fn register<'b>(&'b mut self, name: &'b str) -> impl Future<Output = String> + 'b {
        let caller = std::panic::Location::caller();
        async move {
            let registered = self
                .post_json(
                    "/oauth/register",
                    &serde_json::json!({ "client_name": name, "redirect_uris": [REDIRECT_URI] }),
                )
                .await;
            assert_eq!(
                registered.status,
                201,
                "at {caller}: {}",
                registered.html.html()
            );
            json(&registered)["client_id"]
                .as_str()
                .unwrap_or_else(|| panic!("its client_id at {caller}"))
                .to_string()
        }
    }

    /// Sends the person to authorize `client` with `state`, as the client
    /// does: Consent as it shows.
    #[track_caller]
    pub(super) fn reach_consent<'b>(
        &'b mut self,
        client: &'b str,
        state: &'b str,
    ) -> impl Future<Output = Answer> + 'b {
        let caller = std::panic::Location::caller();
        async move {
            let sent = self.get(&authorize_path(client, REDIRECT_URI, state)).await;
            assert_eq!(sent.status, 303, "at {caller}: {}", sent.html.html());
            assert!(
                sent.location().starts_with("/oauth/authorize?"),
                "at {caller}: {}",
                sent.location()
            );
            let consent = self.get(sent.location()).await;
            assert_eq!(consent.status, 200, "at {caller}: {}", consent.html.html());
            consent
        }
    }

    /// Takes "Allow" on `consent`: Sign in as it then shows.
    #[track_caller]
    pub(super) fn allow<'b>(
        &'b mut self,
        consent: &'b Answer,
    ) -> impl Future<Output = Answer> + 'b {
        let caller = std::panic::Location::caller();
        async move {
            let allowed = self
                .post_fields("/oauth/authorize", &decision_fields(consent, "allow"))
                .await;
            assert_eq!(allowed.status, 303, "at {caller}: {}", allowed.html.html());
            let sign_in = self.get(allowed.location()).await;
            assert_eq!(sign_in.status, 200, "at {caller}: {}", sign_in.html.html());
            sign_in
        }
    }

    /// Sends the person to authorize `client` with `state`, allows it on
    /// Consent and signs in with Google as `email`: where the browser is
    /// sent back to the client.
    #[track_caller]
    pub(super) fn authorize<'b>(
        &'b mut self,
        client: &'b str,
        state: &'b str,
        email: &'b str,
    ) -> impl Future<Output = url::Url> + 'b {
        let caller = std::panic::Location::caller();
        async move {
            let consent = self.reach_consent(client, state).await;
            let sign_in = self.allow(&consent).await;
            let back = self.sign_in_on(&sign_in, email).await;
            assert_eq!(back.status, 303, "at {caller}: {}", back.html.html());
            url::Url::parse(back.location())
                .unwrap_or_else(|e| panic!("at {caller}: {e}: {}", back.location()))
        }
    }

    /// Takes `decision`, "allow" or "deny", on `consent`: where the browser
    /// is sent.
    #[track_caller]
    pub(super) fn decide<'b>(
        &'b mut self,
        consent: &'b Answer,
        decision: &'b str,
    ) -> impl Future<Output = url::Url> + 'b {
        let caller = std::panic::Location::caller();
        async move {
            let decided = self
                .post_fields("/oauth/authorize", &decision_fields(consent, decision))
                .await;
            assert_eq!(decided.status, 303, "at {caller}: {}", decided.html.html());
            url::Url::parse(decided.location()).unwrap()
        }
    }

    /// Exchanges `code` of `client` for tokens.
    pub(super) async fn exchange_code(&mut self, client: &str, code: &str) -> Answer {
        self.exchange_code_from(client, code, REDIRECT_URI).await
    }

    /// Exchanges `code` of `client`, sent back to `redirect_uri`, for
    /// tokens.
    async fn exchange_code_from(&mut self, client: &str, code: &str, redirect_uri: &str) -> Answer {
        self.post_as_client(
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", VERIFIER),
                ("client_id", client),
                ("resource", RESOURCE),
            ],
        )
        .await
    }

    /// Uses the refresh token `token` of `client`.
    async fn refresh(&mut self, client: &str, token: &str) -> Answer {
        self.post_as_client(
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", token),
                ("client_id", client),
            ],
        )
        .await
    }

    /// Posts the form `fields` to `path` from a page of the web UI.
    async fn post_fields(&mut self, path: &str, fields: &[(String, String)]) -> Answer {
        let fields: Vec<(&str, &str)> = fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        self.post(path, &fields).await
    }
}

/// The form `decision`'s button on `consent` posts.
#[track_caller]
fn decision_fields(consent: &Answer, decision: &str) -> Vec<(String, String)> {
    let form = select(
        &consent.html,
        "form[method=post][action='/oauth/authorize']",
    );
    assert_eq!(form.len(), 1, "{}", consent.html.html());
    let mut fields: Vec<(String, String)> = select_in(form[0], "input[type=hidden]")
        .into_iter()
        .map(|input| {
            (
                input.attr("name").unwrap().to_string(),
                input.attr("value").unwrap().to_string(),
            )
        })
        .collect();
    let button = select_in(form[0], &format!("button[value={decision}]"));
    assert_eq!(button.len(), 1, "{}", consent.html.html());
    fields.push((
        button[0].attr("name").unwrap().to_string(),
        decision.to_string(),
    ));
    fields
}

/// The value of `name` in `url`'s query.
pub(super) fn query_value(url: &url::Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(named, _)| named == name)
        .map(|(_, value)| value.into_owned())
}

/// The tokens `answer`, a token request's, holds.
#[track_caller]
pub(super) fn tokens_of(answer: &Answer) -> Tokens {
    let issued = json(answer);
    Tokens {
        access: issued["access_token"].as_str().unwrap().to_string(),
        refresh: issued["refresh_token"].as_str().unwrap().to_string(),
    }
}

/// The identity, as the log gives it, of whom `access` acts as now, if
/// anyone.
#[track_caller]
fn acting_as<'b>(
    signing: &'b Signing,
    access: &'b str,
) -> impl Future<Output = Option<String>> + 'b {
    let caller = std::panic::Location::caller();
    async move {
        let sources = &signing.pages.sources;
        AccessTokens::new(sources.runs.database().clone())
            .resolve(access, sources.clock.now())
            .await
            .unwrap_or_else(|e| panic!("resolving the access token at {caller}: {e:#}"))
            .map(|caller| caller.person().logged())
    }
}

/// Consent's facts: each label and its value.
fn consent_facts(html: &Html) -> Vec<(String, String)> {
    let labels = select(html, ".consent dl > div > dt");
    let values = select(html, ".consent dl > div > dd");
    labels
        .into_iter()
        .map(text)
        .zip(values.into_iter().map(text))
        .collect()
}

/// `docs/specs/access.md`, The authorization server, Issuer, Resource
/// metadata and Server metadata.
#[test_log::test(tokio::test)]
async fn the_metadata_name_linewright_under_its_public_url() {
    let signing = signing(None).await;
    let mut client = Browsing::new(&signing);

    let server = client.get("/.well-known/oauth-authorization-server").await;
    let resource = client
        .get("/.well-known/oauth-protected-resource/mcp")
        .await;

    assert_eq!(server.status, 200);
    assert_eq!(json(&server)["issuer"], "https://127.0.0.1:8081");
    assert_eq!(resource.status, 200);
    assert_eq!(json(&resource)["resource"], RESOURCE);
}

/// `docs/specs/access.md`, The authorization server, Clients, Loopback and
/// Logged.
#[test_log::test(tokio::test)]
async fn a_client_registers_as_a_public_client_and_is_logged() {
    let signing = signing(None).await;
    let mut client = Browsing::new(&signing);
    let (log, _guard) = Log::capture();

    let registered = client
        .post_json(
            "/oauth/register",
            &serde_json::json!({
                "client_name": "My agent",
                "redirect_uris": [REDIRECT_URI, "https://agent.example/cb"],
                "token_endpoint_auth_method": "client_secret_basic",
            }),
        )
        .await;

    assert_eq!(registered.status, 201);
    let registered = json(&registered);
    assert!(registered["client_id"].is_string());
    assert_eq!(registered["client_secret"], serde_json::Value::Null);
    assert_eq!(registered["token_endpoint_auth_method"], "none");
    assert_eq!(
        registered["redirect_uris"],
        serde_json::json!([REDIRECT_URI, "https://agent.example/cb"])
    );
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("registered a client")
                && line.contains(registered["client_id"].as_str().unwrap())
                && line.contains(r#"name="My agent""#)),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, The authorization server, 1 and 2, When a
/// request ends, When an authorization begins and Logged, and Consent;
/// `docs/specs/web/access.md`, Consent.
#[test_log::test(tokio::test)]
async fn a_person_allows_a_client_then_signs_in_and_the_client_exchanges_its_code_once() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let (log, _guard) = Log::capture();

    let requested = browser
        .get(&authorize_path(&client, REDIRECT_URI, "the state"))
        .await;
    let consent_path = requested.location().to_string();
    let consent = browser.get(&consent_path).await;

    assert_eq!(
        text(select(&consent.html, ".consent h1")[0]),
        "Allow My agent to act as you?"
    );
    assert_eq!(
        consent_facts(&consent.html),
        [("Sends you back to".into(), "127.0.0.1".into())]
    );
    let warning = select(&consent.html, ".consent .alert");
    assert_eq!(warning.len(), 1, "{}", consent.html.html());
    assert_eq!(
        text(warning[0]),
        "This client is an app on this computer. Linewright cannot check which app."
    );
    assert!(has_classes(warning[0], &["alert-warning", "mb-6"]));
    let buttons = select(&consent.html, ".consent .grid.grid-cols-2 button");
    assert_eq!(
        buttons.iter().map(|b| text(*b)).collect::<Vec<_>>(),
        ["Allow", "Deny"]
    );
    assert!(has_classes(buttons[0], &["btn", "btn-sm", "btn-primary"]));
    assert!(has_classes(buttons[1], &["btn", "btn-sm"]));
    assert_eq!(select_in(buttons[0], "[data-icon=check]").len(), 1);
    assert_eq!(select_in(buttons[1], "[data-icon=x-mark]").len(), 1);
    assert!(
        consent.headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );

    let allowed = browser
        .post_fields("/oauth/authorize", &decision_fields(&consent, "allow"))
        .await;

    assert_eq!(allowed.status, 303, "{}", allowed.html.html());
    let cookie = allowed
        .set_cookies()
        .into_iter()
        .find(|set| set.starts_with("__Host-linewright-authorization="))
        .expect("the authorization's cookie")
        .to_string();
    assert!(
        cookie.ends_with("; Secure; HttpOnly; SameSite=Lax"),
        "{cookie}"
    );
    assert!((590..=600).contains(&max_age(&cookie)), "{cookie}");
    let handle = cookie.split(['=', ';']).nth(1).unwrap();
    assert!(
        !consent.html.html().contains(handle),
        "the handle Allow gives is not Consent's"
    );
    let sign_in = browser.get(allowed.location()).await;
    assert_eq!(sign_in.status, 200, "{}", sign_in.html.html());
    assert_eq!(
        text(select(&sign_in.html, ".sign-in h1")[0]),
        "Sign in to Linewright"
    );
    let back = browser.sign_in_on(&sign_in, "nameless@example.com").await;

    assert_eq!(back.status, 303, "{}", back.html.html());
    let sent = url::Url::parse(back.location()).unwrap();
    assert_eq!(
        format!("{}://{}{}", sent.scheme(), sent.authority(), sent.path()),
        REDIRECT_URI
    );
    assert_eq!(query_value(&sent, "state").as_deref(), Some("the state"));
    assert_eq!(
        query_value(&sent, "iss").as_deref(),
        Some("https://127.0.0.1:8081")
    );
    let code = query_value(&sent, "code").expect("a code");
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("allowed a client") && line.contains(&client)),
        "{logged:?}"
    );
    assert!(
        logged
            .iter()
            .any(|line| line.contains("authorized a client")
                && line.contains(r#"identity="google:nameless@example.com""#)
                && line.contains(&client)),
        "{logged:?}"
    );
    let authorizations = browser.get("/authorizations").await;
    assert_eq!(authorizations.status, 200, "the browser is signed in");
    assert_eq!(
        authorization_rows(&authorizations.html)[0][..2],
        ["My agent", "127.0.0.1"]
    );
    assert!(
        back.set_cookies()
            .iter()
            .any(|set| set.starts_with("__Host-linewright-authorization=;") && max_age(set) == 0),
        "the request's cookie is removed: {:?}",
        back.set_cookies()
    );
    let again = browser.begin_on(&sign_in, Provider::Google).await;
    assert_eq!(authorization_error(&again), ENDED);
    assert_eq!(
        authorization_error(&browser.get(&consent_path).await),
        ENDED
    );

    let exchanged = browser.exchange_code(&client, &code).await;

    assert_eq!(exchanged.status, 200, "{}", exchanged.html.html());
    assert_eq!(exchanged.headers["cache-control"], "no-store");
    let issued = json(&exchanged);
    assert_eq!(issued["token_type"], "Bearer");
    assert_eq!(issued["expires_in"], 3600);
    let tokens = tokens_of(&exchanged);
    assert_eq!(
        acting_as(&signing, &tokens.access).await.as_deref(),
        Some("google:nameless@example.com")
    );
    let twice = browser.exchange_code(&client, &code).await;
    assert_eq!(twice.status, 400);
    assert_eq!(json(&twice)["error"], "invalid_grant");
    assert_eq!(
        acting_as(&signing, &tokens.access).await,
        None,
        "a second use of the code ended the authorization"
    );
}

/// `docs/specs/access.md`, The authorization server, Loopback: a request
/// may give a loopback redirect URI with another port than the client
/// registered, and Linewright sends the browser back there.
/// <https://www.rfc-editor.org/rfc/rfc8252#section-7.3>
#[test_log::test(tokio::test)]
async fn a_loopback_redirect_uri_takes_the_port_the_request_gives() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let on_its_port = "http://127.0.0.1:61234/cb";
    let requested = browser
        .get(&authorize_path(&client, on_its_port, "s"))
        .await;
    assert_eq!(requested.status, 303, "{}", requested.html.html());
    let consent = browser.get(requested.location()).await;
    assert_eq!(consent.status, 200, "{}", consent.html.html());
    let sign_in = browser.allow(&consent).await;

    let back = browser.sign_in_on(&sign_in, "nameless@example.com").await;

    assert_eq!(back.status, 303, "{}", back.html.html());
    let sent = url::Url::parse(back.location()).unwrap();
    assert_eq!(
        format!("{}://{}{}", sent.scheme(), sent.authority(), sent.path()),
        on_its_port
    );
    let code = query_value(&sent, "code").expect("a code");
    let exchanged = browser
        .exchange_code_from(&client, &code, on_its_port)
        .await;
    assert_eq!(exchanged.status, 200, "{}", exchanged.html.html());
}

/// The `Max-Age` of the `Set-Cookie` `cookie`.
#[track_caller]
fn max_age(cookie: &str) -> i64 {
    cookie
        .split("; ")
        .find_map(|attribute| attribute.strip_prefix("Max-Age="))
        .and_then(|age| age.parse().ok())
        .unwrap_or_else(|| panic!("no Max-Age: {cookie}"))
}

/// What Authorization error says on `answer`, which must be it.
#[track_caller]
fn authorization_error(answer: &Answer) -> String {
    assert_eq!(answer.status, 400, "{}", answer.html.html());
    assert!(!answer.headers.contains_key("location"));
    assert_eq!(
        text(select(&answer.html, ".authorization-error h1")[0]),
        "Linewright cannot authorize this client"
    );
    let alert = select(&answer.html, ".authorization-error .alert")[0];
    assert!(has_classes(alert, &["alert-error"]));
    text(alert)
}

/// The message of Authorization error for a request that has ended.
const ENDED: &str = "This authorization request has ended. Start again from the client.";

/// `docs/specs/access.md`, Consent, Facts and The warning.
#[test_log::test(tokio::test)]
async fn consent_names_a_metadata_document_clients_host_and_warns_only_of_a_loopback_redirect_uri()
{
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = "https://agent.example/oauth/client.json";
    metadata_document_client(
        &signing.pages.sources,
        client,
        "Doc agent",
        "https://back.example/cb",
    )
    .await;

    let sent = browser
        .get(&authorize_path(client, "https://back.example/cb", "s"))
        .await;
    assert_eq!(sent.status, 303, "{}", sent.html.html());
    let consent = browser.get(sent.location()).await;

    assert_eq!(consent.status, 200, "{}", consent.html.html());
    assert_eq!(
        text(select(&consent.html, ".consent h1")[0]),
        "Allow Doc agent to act as you?"
    );
    assert_eq!(
        consent_facts(&consent.html),
        [
            ("Client".into(), "agent.example".into()),
            ("Sends you back to".into(), "back.example".into()),
        ]
    );
    assert_eq!(
        select(&consent.html, ".consent .alert").len(),
        0,
        "no warning"
    );
}

/// `docs/specs/access.md`, The authorization server, 2: whatever the
/// browser is signed in to, the sign-in signs it in as the person who
/// signs in.
#[test_log::test(tokio::test)]
async fn a_signed_in_browser_still_signs_in_for_an_authorization() {
    let signing = signing(Some("google:boss@elsewhere.example")).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    browser.sign_in("/", Some("boss@elsewhere.example")).await;
    let consent = browser.reach_consent(&client, "s").await;

    let sign_in = browser.allow(&consent).await;
    let back = browser.sign_in_on(&sign_in, "nameless@example.com").await;

    assert_eq!(back.status, 303, "{}", back.html.html());
    let code = query_value(&url::Url::parse(back.location()).unwrap(), "code").expect("a code");
    let tokens = tokens_of(&browser.exchange_code(&client, &code).await);
    assert_eq!(
        acting_as(&signing, &tokens.access).await.as_deref(),
        Some("google:nameless@example.com")
    );
    let own = browser.get("/authorizations").await;
    assert_eq!(authorization_rows(&own.html).len(), 1, "nameless's own");
}

/// `docs/specs/access.md`, Signing in, 3c, and The authorization server,
/// 2: a refused sign-in says why on Sign in, which is still for the
/// authorization.
#[test_log::test(tokio::test)]
async fn a_refused_sign_in_shows_sign_in_still_for_the_authorization() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let consent = browser.reach_consent(&client, "s").await;
    let sign_in = browser.allow(&consent).await;

    let refused = browser
        .sign_in_on(&sign_in, "mallory@elsewhere.example")
        .await;

    assert_eq!(refused.status, 403, "{}", refused.html.html());
    assert_eq!(
        text(select(&refused.html, ".sign-in .alert")[0]),
        "mallory@elsewhere.example has no access to this Linewright."
    );
    let back = browser.sign_in_on(&refused, "nameless@example.com").await;
    assert_eq!(back.status, 303, "{}", back.html.html());
    assert!(
        query_value(&url::Url::parse(back.location()).unwrap(), "code").is_some(),
        "{}",
        back.location()
    );
}

/// `docs/specs/access.md`, The authorization server, 1 and 2: a sign-in
/// that has only Consent's handle, which the client can read, completes
/// nothing, so that the client cannot skip Consent.
#[test_log::test(tokio::test)]
async fn a_sign_in_with_only_the_handle_of_consent_completes_nothing() {
    let signing = signing(None).await;
    let mut client_side = Browsing::new(&signing);
    let client = client_side.register("My agent").await;
    let sent = client_side
        .get(&authorize_path(&client, REDIRECT_URI, "s"))
        .await;
    let consent_url = url::Url::parse("https://127.0.0.1:8081")
        .unwrap()
        .join(sent.location())
        .unwrap();
    let (_, asked) = consent_url.query_pairs().next().expect("Consent's handle");
    let with_asked = format!("__Host-linewright-authorization={asked}");
    let mut victim = Browsing::with_cookie(&signing, &with_asked);
    let consent = victim.get(sent.location()).await;
    let sign_in = victim.allow(&consent).await;
    let mut skipping = Browsing::with_cookie(&signing, &with_asked);

    let begun = skipping.begin_on(&sign_in, Provider::Google).await;

    assert_eq!(authorization_error(&begun), ENDED);
    let allowed = victim.sign_in_on(&sign_in, "nameless@example.com").await;
    assert_eq!(
        allowed.status, 303,
        "the request still waits for its own sign-in"
    );
    assert!(allowed.location().starts_with(REDIRECT_URI));
}

/// `docs/specs/access.md`, The authorization server, When a request ends:
/// 10 minutes after the client makes it, whatever happened in between.
#[test_log::test(tokio::test)]
async fn a_request_ends_ten_minutes_after_the_client_makes_it() {
    let signing = signing(None).await;
    let clock = signing.pages.clock.clone();
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let sent = browser
        .get(&authorize_path(&client, REDIRECT_URI, "s"))
        .await;
    let consent_path = sent.location().to_string();
    clock.advance(chrono::TimeDelta::minutes(4));
    let consent = browser.get(&consent_path).await;
    assert_eq!(consent.status, 200, "{}", consent.html.html());
    clock.advance(chrono::TimeDelta::minutes(5));
    let allowed = browser
        .post_fields("/oauth/authorize", &decision_fields(&consent, "allow"))
        .await;
    let cookie = allowed
        .set_cookies()
        .into_iter()
        .find(|set| set.starts_with("__Host-linewright-authorization="))
        .expect("the authorization's cookie")
        .to_string();
    assert!(
        (50..=60).contains(&max_age(&cookie)),
        "it lasts as long as the request: {cookie}"
    );
    let sign_in = browser.get(allowed.location()).await;
    let begun = browser.begin_on(&sign_in, Provider::Google).await;
    clock.advance(chrono::TimeDelta::minutes(1));

    let again = browser.get(&consent_path).await;
    let allowed_again = browser
        .post_fields("/oauth/authorize", &decision_fields(&consent, "allow"))
        .await;
    let back = browser
        .choose_at_google(&begun, "nameless@example.com")
        .await;

    assert_eq!(authorization_error(&again), ENDED);
    assert_eq!(authorization_error(&allowed_again), ENDED);
    assert_eq!(authorization_error(&back), ENDED);
}

/// `docs/specs/access.md`, The authorization server, When a request ends
/// and Logged.
#[test_log::test(tokio::test)]
async fn a_person_denies_a_client_which_is_sent_access_denied_and_the_request_ends() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let sent = browser
        .get(&authorize_path(&client, REDIRECT_URI, "s"))
        .await;
    let consent_path = sent.location().to_string();
    let consent = browser.get(&consent_path).await;
    let (log, _guard) = Log::capture();

    let denied = browser.decide(&consent, "deny").await;

    assert_eq!(
        query_value(&denied, "error").as_deref(),
        Some("access_denied")
    );
    assert_eq!(query_value(&denied, "state").as_deref(), Some("s"));
    assert_eq!(query_value(&denied, "code"), None);
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("denied a client") && line.contains(&client)),
        "{logged:?}"
    );
    assert_eq!(
        authorization_error(&browser.get(&consent_path).await),
        ENDED
    );
    let allowed = browser
        .post_fields("/oauth/authorize", &decision_fields(&consent, "allow"))
        .await;
    assert_eq!(authorization_error(&allowed), ENDED);
}

/// `docs/specs/access.md`, Consent; `docs/specs/web/access.md`: Allow and
/// Deny are taken only from Linewright's own pages.
#[test_log::test(tokio::test)]
async fn a_decision_from_another_site_is_refused() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let consent = browser.reach_consent(&client, "s").await;
    let fields = decision_fields(&consent, "allow");
    let mut form = form_urlencoded::Serializer::new(String::new());
    form.extend_pairs(&fields);

    let refused = browser
        .send(
            axum::http::Request::post("/oauth/authorize")
                .header(CROSS_SITE.0, CROSS_SITE.1)
                .header("Content-Type", "application/x-www-form-urlencoded"),
            form.finish(),
        )
        .await;

    assert_eq!(refused.status, 403);
    assert!(!refused.headers.contains_key("location"));
    let allowed = browser.post_fields("/oauth/authorize", &fields).await;
    assert_eq!(allowed.status, 303, "the request still waits");
    assert!(allowed.location().starts_with("/sign-in?"));
}

/// `docs/specs/access.md`, Authorization error; `docs/specs/web/access.md`,
/// Authorization error. A metadata document on a loopback address cannot
/// be fetched.
#[test_log::test(tokio::test)]
async fn an_unknown_client_unusable_metadata_document_or_redirect_uri_shows_authorization_error_and_sends_nothing_back()
 {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;

    for (path, message) in [
        (
            authorize_path("no-such-client", REDIRECT_URI, "s"),
            "Linewright does not know this client.",
        ),
        (
            authorize_path("https://127.0.0.1/client", REDIRECT_URI, "s"),
            "Linewright cannot use this client's metadata document.",
        ),
        (
            authorize_path(&client, "http://127.0.0.1:5000/other", "s"),
            "This is not a redirect URI of this client.",
        ),
        (
            "/oauth/authorize?waiting=no-such-request".to_string(),
            ENDED,
        ),
    ] {
        let error = browser.get(&path).await;

        assert_eq!(authorization_error(&error), message, "{path}");
    }
}

/// `docs/specs/access.md`, The authorization server, Grants.
#[test_log::test(tokio::test)]
async fn a_request_linewright_refuses_is_sent_back_to_its_client() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;

    let plain = authorize_path(&client, REDIRECT_URI, "s").replace("S256", "plain");

    let sent = browser.get(&plain).await;

    assert_eq!(sent.status, 303);
    let sent = url::Url::parse(sent.location()).unwrap();
    assert_eq!(sent.path(), "/cb");
    assert_eq!(
        query_value(&sent, "error").as_deref(),
        Some("invalid_request")
    );
    assert_eq!(query_value(&sent, "state").as_deref(), Some("s"));
}

/// `docs/specs/access.md`, The authorization server, A second use and An
/// ended authorization: each use of a refresh token gives a new one, and a
/// second use of one ends its authorization.
#[test_log::test(tokio::test)]
async fn a_refresh_token_works_once_and_a_second_use_ends_its_authorization() {
    let signing = signing(None).await;
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (id, first) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;
    let mut client = Browsing::new(&signing);
    let second = client.refresh(&id, &first.refresh).await;
    assert_eq!(second.status, 200, "{}", second.html.html());
    let second = tokens_of(&second);
    assert_ne!(second.refresh, first.refresh);
    let (log, _guard) = Log::capture();

    let again = client.refresh(&id, &first.refresh).await;

    assert_eq!(again.status, 400);
    assert_eq!(json(&again)["error"], "invalid_grant");
    assert_eq!(acting_as(&signing, &second.access).await, None);
    assert_eq!(client.refresh(&id, &second.refresh).await.status, 400);
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("ended an authorization")),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, The authorization server, An access token.
#[test_log::test(tokio::test)]
async fn an_access_token_lasts_an_hour() {
    let signing = signing(None).await;
    let clock = signing.pages.clock.clone();
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (_, tokens) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;

    clock.advance(chrono::TimeDelta::minutes(59));
    assert!(acting_as(&signing, &tokens.access).await.is_some());
    clock.advance(chrono::TimeDelta::minutes(1));

    assert_eq!(acting_as(&signing, &tokens.access).await, None);
}

/// Authorizations' rows: each cell's text but the buttons'.
fn authorization_rows(html: &Html) -> Vec<Vec<String>> {
    select(html, "#authorizations tbody tr")
        .into_iter()
        .map(|row| {
            select_in(row, "th, td")
                .into_iter()
                .filter(|cell| select_in(*cell, "button").is_empty())
                .map(text)
                .collect()
        })
        .collect()
}

/// `docs/specs/access.md`, Authorizations and End authorization;
/// `docs/specs/shell.md`, The account; `docs/specs/web/access.md`,
/// Authorizations.
#[test_log::test(tokio::test)]
async fn a_person_sees_their_authorizations_and_ends_one() {
    let signing = signing(None).await;
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (_, tokens) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;
    let (_, later) = authorized_by_back_door(&signing.pages.sources, &person, "").await;
    let mut browser = Browsing::with_cookie(&signing, &person);

    let page = browser.get("/authorizations").await;

    assert_eq!(page.status, 200, "{}", page.html.html());
    let menu = select(&page.html, ".account .menu a[href='/authorizations']");
    assert_eq!(menu.len(), 1);
    assert_eq!(text(menu[0]), "Authorizations");
    assert_eq!(select_in(menu[0], "[data-icon=key]").len(), 1);
    assert_eq!(
        text(select(&page.html, "title")[0]),
        "Authorizations · Linewright"
    );
    assert!(
        select(&page.html, "#crumbs[hidden]").len() == 1,
        "no breadcrumbs"
    );
    assert_eq!(
        select(&page.html, "#authorizations .card-title [data-icon=key]").len(),
        1
    );
    assert_eq!(
        select(&page.html, "#authorizations thead th")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["Client", "Sends back to", "Allowed", "Ends", ""]
    );
    let rows = authorization_rows(&page.html);
    assert_eq!(
        rows.iter().map(|row| row[..2].to_vec()).collect::<Vec<_>>(),
        [
            ["an unnamed client", "127.0.0.1"],
            ["My agent", "127.0.0.1"],
        ],
        "newest first"
    );
    assert!(rows[1][3].starts_with("2026-10-31"), "{:?}", rows[1]);
    let ends = confirmations(&page.html);
    assert_eq!(
        ends[1],
        Confirmation {
            asking: "End".into(),
            question: "End the authorization of My agent? My agent can no longer act as you."
                .into(),
            turn_down: "Cancel".into(),
            action: "/authorizations/1/end".into(),
            fields: vec![],
        }
    );
    assert_eq!(
        select(
            &page.html,
            "#authorizations tbody button [data-icon=link-slash]"
        )
        .len(),
        2
    );
    let (log, _guard) = Log::capture();

    let ended = browser.post("/authorizations/1/end", &[]).await;

    assert_eq!(ended.status, 200, "{}", ended.html.html());
    assert_eq!(acting_as(&signing, &tokens.access).await, None);
    assert!(acting_as(&signing, &later.access).await.is_some());
    let after = browser.get("/authorizations").await;
    assert_eq!(authorization_rows(&after.html).len(), 1);
    let logged = log.lines("INFO");
    assert!(
        logged.iter().any(
            |line| line.contains("End one of one's own authorizations: done")
                && line.contains(r#"action="end_authorization""#)
                && line.contains(r#"identity="google:vera@example.com""#)
        ),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, Out of scope: seeing or ending another person's
/// authorizations.
#[test_log::test(tokio::test)]
async fn a_person_cannot_end_anothers_authorization() {
    let signing = signing(None).await;
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (_, tokens) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;
    let manager = signing.pages.cookie.clone();
    let mut manager = Browsing::with_cookie(&signing, &manager);

    let refused = manager.post("/authorizations/1/end", &[]).await;
    let page = manager.get("/authorizations").await;

    assert_eq!(refused.status, 404, "{}", refused.html.html());
    assert_eq!(authorization_rows(&page.html).len(), 0);
    assert!(acting_as(&signing, &tokens.access).await.is_some());
}

/// `docs/specs/access.md`, People, Disabling.
#[test_log::test(tokio::test)]
async fn disabling_a_person_ends_their_authorizations() {
    let signing = signing(None).await;
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (id, tokens) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;
    let mut browser = Browsing::new(&signing);
    let manager = signing.pages.cookie.clone();
    let mut manager = Browsing::with_cookie(&signing, &manager);

    let disabled = manager.post("/people/2/disable", &[]).await;

    assert_eq!(disabled.status, 200, "{}", disabled.html.html());
    assert_eq!(acting_as(&signing, &tokens.access).await, None);
    manager.post("/people/2/enable", &[]).await;
    assert_eq!(acting_as(&signing, &tokens.access).await, None);
    assert_eq!(browser.refresh(&id, &tokens.refresh).await.status, 400);
}
