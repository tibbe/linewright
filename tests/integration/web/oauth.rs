//! Linewright as the OAuth authorization server of MCP clients
//! (`docs/specs/access.md`, The authorization server, Tokens, Consent,
//! Authorization error, Authorizations and End authorization;
//! `docs/specs/web/access.md`, Consent, Authorization error and
//! Authorizations), through the router, signing people in with the fake
//! Google.

use linewright::access::oauth::store::AccessTokens;

use super::sign_in::{Answer, Browsing, Signing, signing};
use super::*;

/// The redirect URI the client of these tests registers.
const REDIRECT_URI: &str = "http://127.0.0.1:5000/cb";

/// RFC 7636's example: its verifier and its challenge.
/// <https://www.rfc-editor.org/rfc/rfc7636#appendix-B>
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// The MCP server's URI under the tests' public URL.
const RESOURCE: &str = "http://127.0.0.1:8081/mcp";

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

    /// Sends the person to authorize `client` with `state`, signs them in
    /// with Google as `email`, and gives Consent as it shows.
    #[track_caller]
    pub(super) fn reach_consent<'b>(
        &'b mut self,
        client: &'b str,
        state: &'b str,
        email: &'b str,
    ) -> impl Future<Output = Answer> + 'b {
        let caller = std::panic::Location::caller();
        async move {
            let path = authorize_path(client, REDIRECT_URI, state);
            let back = self.sign_in(&path, Some(email)).await;
            assert_eq!(back.status, 303, "at {caller}: {}", back.html.html());
            assert!(
                back.location().starts_with("/oauth/authorize?"),
                "at {caller}: {}",
                back.location()
            );
            let consent = self.get(back.location()).await;
            assert_eq!(consent.status, 200, "at {caller}: {}", consent.html.html());
            consent
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
        self.post_as_client(
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
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

/// The path that sends a person to authorize `client`, as a client sends it.
fn authorize_path(client: &str, redirect_uri: &str, state: &str) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
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

/// The tokens a token request was issued.
pub(super) struct Tokens {
    pub(super) access: String,
    pub(super) refresh: String,
}

impl Tokens {
    #[track_caller]
    pub(super) fn of(answer: &Answer) -> Tokens {
        let issued = json(answer);
        Tokens {
            access: issued["access_token"].as_str().unwrap().to_string(),
            refresh: issued["refresh_token"].as_str().unwrap().to_string(),
        }
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

/// Authorizes a new client named `name` for the person whose browser
/// `cookie` signs in to `sources`, by the back door: the client's ID and
/// its tokens.
#[track_caller]
pub(super) fn authorized_by_back_door<'b>(
    sources: &'b Sources,
    cookie: &'b str,
    name: &'b str,
) -> impl Future<Output = (String, Tokens)> + 'b {
    use linewright::access::oauth::clients::Clients;
    use linewright::access::oauth::store::{
        Answered, AuthorizationRequests, Decision, Tokens as Issuing,
    };
    use linewright::access::oauth::{
        AuthorizeRequest, RedirectUri, Server, TokenForm, TokenRequest, check_authorize,
    };
    use linewright::access::sessions::{Resolved, SESSION_COOKIE, SessionCookie, Sessions};
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
        let checked = check_authorize(&request, Some(&client), &server)
            .unwrap_or_else(|e| panic!("authorizing at {caller}: {e:?}"));
        let requests = AuthorizationRequests::new(database.clone());
        let secret = requests.begin(&checked, now).await.unwrap();
        requests
            .signed_in(&secret, person.person().id, now)
            .await
            .unwrap();
        let Some(Answered::Allowed { code, .. }) = requests
            .answer(&secret, person.person().id, Decision::Allow, &server, now)
            .await
            .unwrap()
        else {
            panic!("not allowed at {caller}");
        };
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
    assert_eq!(json(&server)["issuer"], "http://127.0.0.1:8081");
    assert_eq!(resource.status, 200);
    assert_eq!(json(&resource)["resource"], RESOURCE);
}

/// `docs/specs/access.md`, The authorization server, Clients, Redirect URIs
/// and Logged.
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

/// `docs/specs/access.md`, The authorization server, 1 to 6, Consent and
/// Tokens; `docs/specs/web/access.md`, Consent.
#[test_log::test(tokio::test)]
async fn a_person_signs_in_and_allows_a_client_which_exchanges_its_code_once() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let (log, _guard) = Log::capture();

    let consent = browser
        .reach_consent(&client, "the state", "nameless@example.com")
        .await;

    assert_eq!(
        text(select(&consent.html, ".consent h1")[0]),
        "Allow My agent to act as you?"
    );
    assert_eq!(
        consent_facts(&consent.html),
        [
            ("Signed in as".into(), "nameless@example.com".into()),
            ("Sends you back to".into(), "127.0.0.1".into()),
            ("Access".into(), "viewer".into()),
        ]
    );
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

    let sent = browser.decide(&consent, "allow").await;

    assert_eq!(
        format!("{}://{}{}", sent.scheme(), sent.authority(), sent.path()),
        REDIRECT_URI
    );
    assert_eq!(query_value(&sent, "state").as_deref(), Some("the state"));
    assert_eq!(
        query_value(&sent, "iss").as_deref(),
        Some("http://127.0.0.1:8081")
    );
    let code = query_value(&sent, "code").expect("a code");
    let logged = log.lines("INFO");
    assert!(
        logged.iter().any(|line| line.contains("allowed a client")
            && line.contains(r#"identity="google:nameless@example.com""#)
            && line.contains(&client)),
        "{logged:?}"
    );

    let exchanged = browser.exchange_code(&client, &code).await;
    let again = browser.exchange_code(&client, &code).await;

    assert_eq!(exchanged.status, 200, "{}", exchanged.html.html());
    assert_eq!(exchanged.headers["cache-control"], "no-store");
    let issued = json(&exchanged);
    assert_eq!(issued["token_type"], "Bearer");
    assert_eq!(issued["expires_in"], 3600);
    let tokens = Tokens::of(&exchanged);
    assert_eq!(
        acting_as(&signing, &tokens.access).await.as_deref(),
        Some("google:nameless@example.com")
    );
    assert_eq!(again.status, 400);
    assert_eq!(json(&again)["error"], "invalid_grant");
}

/// `docs/specs/access.md`, The authorization server: each authorization
/// takes steps 3 and 4, whatever the browser is signed in to.
#[test_log::test(tokio::test)]
async fn a_signed_in_browser_still_signs_in_for_an_authorization() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    browser.sign_in("/", Some("nameless@example.com")).await;

    let sent = browser
        .get(&authorize_path(&client, REDIRECT_URI, "s"))
        .await;

    assert_eq!(sent.status, 303);
    assert!(
        sent.location().starts_with("/sign-in?return="),
        "{}",
        sent.location()
    );
    let sign_in = url::Url::parse("http://127.0.0.1:8081")
        .unwrap()
        .join(sent.location())
        .unwrap();
    let request = query_value(&sign_in, "return").expect("the request to return to");
    let consent = browser.get(&request).await;
    assert_eq!(consent.status, 303, "Consent waits for the sign-in");
    assert!(consent.location().starts_with("/sign-in?return="));
}

/// `docs/specs/access.md`, The authorization server, 3 and 4: Consent is
/// for the person who signed in for the request last.
#[test_log::test(tokio::test)]
async fn consent_is_for_whoever_signed_in_for_the_request_last() {
    let signing = signing(Some("google:boss@elsewhere.example")).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let first = browser
        .reach_consent(&client, "s", "nameless@example.com")
        .await;
    let request = decision_fields(&first, "allow")[0].1.clone();
    let signed_out = browser.post("/sign-out", &[]).await;
    assert_eq!(signed_out.status, 303, "{}", signed_out.html.html());

    let back = browser
        .sign_in(
            &format!("/oauth/authorize?waiting={request}"),
            Some("boss@elsewhere.example"),
        )
        .await;
    let consent = browser.get(back.location()).await;

    assert_eq!(consent.status, 200, "{}", consent.html.html());
    assert_eq!(
        consent_facts(&consent.html)[0],
        ("Signed in as".into(), "boss@elsewhere.example".into())
    );
}

/// `docs/specs/access.md`, The authorization server, 4a.
#[test_log::test(tokio::test)]
async fn a_person_denies_a_client_which_is_sent_access_denied() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let consent = browser
        .reach_consent(&client, "s", "nameless@example.com")
        .await;
    let (log, _guard) = Log::capture();

    let sent = browser.decide(&consent, "deny").await;

    assert_eq!(
        query_value(&sent, "error").as_deref(),
        Some("access_denied")
    );
    assert_eq!(query_value(&sent, "state").as_deref(), Some("s"));
    assert_eq!(query_value(&sent, "code"), None);
    let logged = log.lines("INFO");
    assert!(
        logged.iter().any(|line| line.contains("denied a client")
            && line.contains(r#"identity="google:nameless@example.com""#)),
        "{logged:?}"
    );
    let authorizations = browser.get("/authorizations").await;
    assert_eq!(
        select(&authorizations.html, "#authorizations tbody tr").len(),
        0
    );
}

/// `docs/specs/access.md`, Consent; `docs/specs/web/access.md`: Allow and
/// Deny are taken only from Linewright's own pages.
#[test_log::test(tokio::test)]
async fn a_decision_from_another_site_is_refused() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let consent = browser
        .reach_consent(&client, "s", "nameless@example.com")
        .await;
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
}

/// `docs/specs/access.md`, The authorization server, 2a, and Authorization
/// error; `docs/specs/web/access.md`, Authorization error.
#[test_log::test(tokio::test)]
async fn an_unknown_client_or_redirect_uri_shows_authorization_error_and_sends_nothing_back() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;

    for (path, message) in [
        (
            authorize_path("no-such-client", REDIRECT_URI, "s"),
            "Linewright does not know this client.",
        ),
        (
            authorize_path(&client, "http://127.0.0.1:5001/cb", "s"),
            "This is not a redirect URI of this client.",
        ),
    ] {
        let error = browser.get(&path).await;

        assert_eq!(error.status, 400, "{path}");
        assert!(!error.headers.contains_key("location"));
        assert_eq!(
            text(select(&error.html, ".authorization-error h1")[0]),
            "Linewright cannot authorize this client"
        );
        let alert = select(&error.html, ".authorization-error .alert")[0];
        assert_eq!(text(alert), message);
        assert!(has_classes(alert, &["alert-error"]));
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

/// `docs/specs/access.md`, Tokens: rotation, a lost response, and a refresh
/// token that has stopped working ending its authorization.
#[test_log::test(tokio::test)]
async fn refresh_tokens_rotate_with_the_grace_of_a_lost_response() {
    let signing = signing(None).await;
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (id, first) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;
    let mut client = Browsing::new(&signing);
    let lost = client.refresh(&id, &first.refresh).await;
    assert_eq!(lost.status, 200, "{}", lost.html.html());

    let retried = client.refresh(&id, &first.refresh).await;

    assert_eq!(retried.status, 200, "a lost response is retried");
    let retried = Tokens::of(&retried);
    assert_ne!(retried.refresh, Tokens::of(&lost).refresh);
    let replaced = client.refresh(&id, &retried.refresh).await;
    assert_eq!(replaced.status, 200, "{}", replaced.html.html());
    let replaced = Tokens::of(&replaced);
    let (log, _guard) = Log::capture();
    let dead = client.refresh(&id, &first.refresh).await;
    assert_eq!(
        dead.status, 400,
        "the first stopped working once its replacement was used"
    );
    assert_eq!(json(&dead)["error"], "invalid_grant");
    assert_eq!(acting_as(&signing, &replaced.access).await, None);
    assert_eq!(client.refresh(&id, &replaced.refresh).await.status, 400);
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("ended an authorization")),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, Tokens: the refresh token that a lost response
/// held stopped working, and using it ends its authorization.
#[test_log::test(tokio::test)]
async fn the_refresh_token_of_a_lost_response_ends_its_authorization() {
    let signing = signing(None).await;
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (id, first) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;
    let mut client = Browsing::new(&signing);
    let lost = Tokens::of(&client.refresh(&id, &first.refresh).await);
    let retried = Tokens::of(&client.refresh(&id, &first.refresh).await);

    let dead = client.refresh(&id, &lost.refresh).await;

    assert_eq!(dead.status, 400);
    assert_eq!(acting_as(&signing, &retried.access).await, None);
}

/// `docs/specs/access.md`, Tokens: the previous refresh token stops working
/// once the client first uses the one that replaced it.
#[test_log::test(tokio::test)]
async fn the_previous_refresh_token_stops_working_once_its_replacement_is_used() {
    let signing = signing(None).await;
    let person = signed_in_as(&signing.pages.sources, Role::Viewer, "Vera Viewer").await;
    let (id, first) = authorized_by_back_door(&signing.pages.sources, &person, "My agent").await;
    let mut client = Browsing::new(&signing);
    let second = Tokens::of(&client.refresh(&id, &first.refresh).await);
    let third = Tokens::of(&client.refresh(&id, &second.refresh).await);

    let dead = client.refresh(&id, &first.refresh).await;

    assert_eq!(dead.status, 400);
    assert_eq!(acting_as(&signing, &third.access).await, None);
}

/// `docs/specs/access.md`, Tokens: an access token stops working one hour
/// after Linewright issues it.
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

/// `docs/specs/access.md`, People, Disabling: a request the person signed
/// in for, but did not answer, is gone with their authorizations.
#[test_log::test(tokio::test)]
async fn disabling_a_person_forgets_the_requests_they_signed_in_for() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let consent = browser
        .reach_consent(&client, "s", "nameless@example.com")
        .await;
    let manager = signing.pages.cookie.clone();
    let mut manager = Browsing::with_cookie(&signing, &manager);
    manager.post("/people/2/disable", &[]).await;
    manager.post("/people/2/enable", &[]).await;
    browser.sign_in("/", Some("nameless@example.com")).await;

    let answered = browser
        .post_fields("/oauth/authorize", &decision_fields(&consent, "allow"))
        .await;

    assert_eq!(answered.status, 400, "{}", answered.html.html());
    assert!(!answered.headers.contains_key("location"));
}
