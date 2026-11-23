//! Signing in and out of the web UI (`docs/specs/access.md`, Signing in and
//! Sign in): Sign in, the sign-in with each identity provider from it to
//! the provider's callback, and "Sign out".

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tracing::{info, warn};
use url::Url;

use linewright_core::access::oauth::store::AuthorizationRequests;
use linewright_core::access::people::{
    Claims, ManagerName, People, PersonId, Refusal, SignInDecision, SignUpRule, decide_sign_in,
};
use linewright_core::access::providers::{Begun, GitHub, Google, Provider, Providers};
use linewright_core::access::sessions::SessionCookie;
use linewright_core::access::sign_ins::{SIGN_IN_COOKIE, SignIns};
use linewright_core::api::Sources;
use linewright_core::domain::Timestamp;
use linewright_core::host::PublicUrl;

use super::caller::{CrossSite, session_cookie, sessions};
use super::card_page::CardPage;
use super::html::Html;
use super::icons::{Icon, IconSize};
use super::{HASHED, cookie_headers, cookie_values};

/// The path of Sign in.
pub(super) const SIGN_IN_PATH: &str = "/sign-in";

/// The path Sign in posts to, to sign in with the provider of the key
/// `{provider}`, such as `google`.
pub(super) const BEGIN_PATH: &str = "/auth/{provider}";

/// The path a provider sends the browser back to (`docs/specs/access.md`,
/// Signing in, Callbacks).
pub(super) const CALLBACK_PATH: &str = "/auth/{provider}/callback";

/// The path Sign in posts to, to sign in with `provider`.
pub(super) fn begin_path(provider: Provider) -> String {
    format!("/auth/{provider}")
}

/// The path "Sign out" posts to.
pub(super) const SIGN_OUT_PATH: &str = "/sign-out";

/// How the web UI signs people in.
pub(super) struct SignIn {
    pub(super) public_url: PublicUrl,
    pub(super) providers: Providers,
    pub(super) manager: Option<ManagerName>,
    pub(super) http: reqwest::Client,
}

impl SignIn {
    /// The provider whose key is `key`, such as `google`, when Linewright
    /// offers it.
    fn offered(&self, key: &str) -> Option<Offered<'_>> {
        match Provider::named(key)? {
            Provider::Google => self.providers.google().map(Offered::Google),
            Provider::GitHub => self.providers.github().map(Offered::GitHub),
        }
    }

    /// The URL `provider` sends the browser back to.
    fn callback(&self, provider: Provider) -> Url {
        self.public_url
            .join(&format!("{}/callback", begin_path(provider)))
    }

    /// Where to send the browser to sign in at `offered` for the sign-in
    /// `begun`.
    fn authorize_url(&self, offered: Offered<'_>, begun: &Begun) -> Url {
        let callback = self.callback(offered.provider());
        match offered {
            Offered::Google(google) => google.authorize_url(&callback, begun),
            Offered::GitHub(github) => github.authorize_url(&callback, begun),
        }
    }

    /// What `offered` says of the person it signed in, for the `code` it
    /// sent the browser back with for the sign-in `begun`.
    async fn finish(
        &self,
        offered: Offered<'_>,
        code: &str,
        begun: &Begun<'_>,
        now: Timestamp,
    ) -> anyhow::Result<Claims> {
        let callback = self.callback(offered.provider());
        match offered {
            Offered::Google(google) => google.finish(&self.http, code, &callback, begun, now).await,
            Offered::GitHub(github) => github.finish(&self.http, code, &callback, begun).await,
        }
    }
}

/// An identity provider Linewright offers, with its settings.
#[derive(Clone, Copy)]
enum Offered<'a> {
    Google(&'a Google),
    GitHub(&'a GitHub),
}

impl<'a> Offered<'a> {
    fn provider(self) -> Provider {
        match self {
            Offered::Google(_) => Provider::Google,
            Offered::GitHub(_) => Provider::GitHub,
        }
    }

    fn rule(self) -> SignUpRule<'a> {
        match self {
            Offered::Google(google) => SignUpRule::Google(&google.signup),
            Offered::GitHub(github) => SignUpRule::GitHub(&github.signup),
        }
    }
}

/// Why a browser is on Sign in, where there is a reason
/// (`docs/specs/access.md`, Sign in, The message), as its query gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reason {
    SignedOut,
    Ended,
}

impl Reason {
    /// Its value in the query.
    fn value(self) -> &'static str {
        match self {
            Reason::SignedOut => "signed-out",
            Reason::Ended => "ended",
        }
    }

    /// The reason whose value in the query is `value`, if one is.
    fn of(value: &str) -> Option<Reason> {
        [Reason::SignedOut, Reason::Ended]
            .into_iter()
            .find(|reason| reason.value() == value)
    }

    fn message(self) -> &'static str {
        match self {
            Reason::SignedOut => "You signed out.",
            Reason::Ended => "Your sign-in ended.",
        }
    }
}

/// A path of the web UI for a browser to return to once signed in: one
/// that starts with a single `/`, so that it leads nowhere else, and of
/// the characters a `Location` header holds as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReturnPath(String);

impl ReturnPath {
    /// `path` where it is one, and else the board's.
    pub(super) fn parse(path: Option<&str>) -> ReturnPath {
        let path = path.filter(|path| {
            path.starts_with('/')
                && !path.starts_with("//")
                && !path.starts_with("/\\")
                && path.chars().all(|c| c.is_ascii_graphic())
        });
        ReturnPath(path.unwrap_or("/").to_string())
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The path of Sign in, for a browser to come back to `return_to` once it
/// signs in, saying `reason` where there is one.
pub(super) fn sign_in_path(return_to: &ReturnPath, reason: Option<Reason>) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    if return_to.as_str() != "/" {
        query.append_pair("return", return_to.as_str());
    }
    if let Some(reason) = reason {
        query.append_pair("reason", reason.value());
    }
    super::with_query(SIGN_IN_PATH, &query.finish())
}

#[derive(Template, WebTemplate)]
#[template(path = "sign_in.html")]
struct SignInPage {
    card: CardPage,
    message: Option<Message>,
    /// Google's button, where Linewright offers Google.
    google: Option<GoogleButton>,
    /// GitHub's button, where Linewright offers GitHub.
    github: Option<GitHubButton>,
    return_to: ReturnPath,
}

/// Google's own button, as Google gives it for each theme.
struct GoogleButton {
    action: String,
    light: &'static str,
    dark: &'static str,
}

/// GitHub's button, with its mark.
struct GitHubButton {
    action: String,
    /// Black in the light theme and white in the dark one.
    mark: Html,
}

/// What Sign in says of why the person is on it.
struct Message {
    text: String,
    /// Whether it is a refusal or a failure, rather than "You signed out."
    /// or "Your sign-in ended.".
    error: bool,
}

impl SignInPage {
    fn new(
        sign_in: &SignIn,
        sources: &Sources,
        return_to: ReturnPath,
        message: Option<Message>,
    ) -> Self {
        let path = |name| HASHED.names[name].as_str();
        SignInPage {
            card: CardPage::new("Sign in", "sign-in", "Sign in to Linewright", sources),
            message,
            google: sign_in
                .providers
                .offer(Provider::Google)
                .then(|| GoogleButton {
                    action: begin_path(Provider::Google),
                    light: path("google-sign-in-light.svg"),
                    dark: path("google-sign-in-dark.svg"),
                }),
            github: sign_in
                .providers
                .offer(Provider::GitHub)
                .then(|| GitHubButton {
                    action: begin_path(Provider::GitHub),
                    mark: Icon::GitHubMark
                        .svg_classed(&IconSize::Inline, "text-black dark:text-white"),
                }),
            return_to,
        }
    }
}

/// Sign in, saying why the browser is on it where its query gives a
/// reason. Of each of the query's `return` and `reason`, the first counts,
/// and a value that is no reason is none.
pub(super) async fn sign_in_page(
    Extension(sign_in): Extension<Arc<SignIn>>,
    State(sources): State<Sources>,
    RawQuery(query): RawQuery,
) -> Response {
    let pairs: Vec<(String, String)> = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .into_owned()
        .collect();
    let first = |name: &str| {
        pairs
            .iter()
            .find(|(named, _)| named == name)
            .map(|(_, value)| value.as_str())
    };
    let (return_to, reason) = (first("return"), first("reason").and_then(Reason::of));
    let message = reason.map(|reason| Message {
        text: reason.message().to_string(),
        error: false,
    });
    SignInPage::new(&sign_in, &sources, ReturnPath::parse(return_to), message).into_response()
}

#[derive(Deserialize)]
struct SignInForm {
    #[serde(rename = "return")]
    return_to: Option<String>,
}

/// Begins a sign-in with the provider of the key `key`, for a request
/// from Sign in: sends the browser to the provider, tied to it by
/// [`SIGN_IN_COOKIE`].
pub(super) async fn begin(
    Extension(sign_in): Extension<Arc<SignIn>>,
    State(sources): State<Sources>,
    Path(key): Path<String>,
    cross_site: Option<Extension<CrossSite>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(refused) = refused_from_another_site(cross_site, "a sign-in") {
        return refused;
    }
    let Some(offered) = sign_in.offered(&key) else {
        return super::no_such_page().await;
    };
    let provider = offered.provider();
    let form: SignInForm =
        serde_urlencoded::from_bytes(&body).unwrap_or(SignInForm { return_to: None });
    let return_to = ReturnPath::parse(form.return_to.as_deref());
    let browser = sign_in_cookie(&headers);
    let pending = SignIns::new(sources.runs.database().clone())
        .begin(
            provider,
            return_to.as_str(),
            browser.as_deref(),
            sources.clock.now(),
        )
        .await;
    let pending = match pending {
        Ok(pending) => pending,
        Err(e) => return failed(&sign_in, &sources, provider, return_to, &e),
    };
    let authorize = sign_in.authorize_url(offered, &pending.begun());
    (
        [(header::SET_COOKIE, pending.set_cookie())],
        Redirect::to(authorize.as_str()),
    )
        .into_response()
}

/// The browser's [`SIGN_IN_COOKIE`], if it sent one.
fn sign_in_cookie(headers: &HeaderMap) -> Option<String> {
    cookie_values(cookie_headers(headers), SIGN_IN_COOKIE)
        .into_iter()
        .next()
}

/// What a provider sends the browser back with.
/// <https://developers.google.com/identity/openid-connect/openid-connect#confirmxsrftoken>
/// <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#2-users-are-redirected-back-to-your-site-by-github>
#[derive(Deserialize)]
pub(super) struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

/// Finishes the sign-in with the provider of the key `key` that the
/// browser began: signs it in and sends it where it was going, or says on
/// Sign in why not.
pub(super) async fn callback(
    Extension(sign_in): Extension<Arc<SignIn>>,
    State(sources): State<Sources>,
    Path(key): Path<String>,
    Query(callback): Query<Callback>,
    headers: HeaderMap,
) -> Response {
    let now = sources.clock.now();
    let Some(offered) = sign_in.offered(&key) else {
        return super::no_such_page().await;
    };
    let provider = offered.provider();
    let browser = sign_in_cookie(&headers);
    let pending = match &callback.state {
        Some(state) => {
            SignIns::new(sources.runs.database().clone())
                .take(provider, state, browser.as_deref(), now)
                .await
        }
        None => Ok(None),
    };
    let pending = match pending {
        Ok(Some(pending)) => pending,
        Ok(None) => {
            let e = anyhow::anyhow!("its state names no sign-in this browser began that waits");
            return failed(&sign_in, &sources, provider, ReturnPath::parse(None), &e);
        }
        Err(e) => {
            return failed(&sign_in, &sources, provider, ReturnPath::parse(None), &e);
        }
    };
    let return_to = ReturnPath::parse(Some(&pending.return_to));
    let finished = match (&callback.code, &callback.error) {
        (Some(code), None) => sign_in.finish(offered, code, &pending.begun(), now).await,
        (_, Some(error)) => Err(anyhow::anyhow!(
            "{} sent back the error {error:?}",
            provider.name()
        )),
        (None, None) => Err(anyhow::anyhow!("{} sent back no code", provider.name())),
    };
    let decided = match finished {
        Ok(claims) => sign_in_as_decided(&sign_in, &sources, &offered.rule(), &claims).await,
        Err(e) => Err(e),
    };
    let decided = match decided {
        Ok(Ok((cookie, person))) => match signed_in_for(&sources, &return_to, person, now).await {
            Ok(()) => Ok(Ok(cookie)),
            Err(e) => {
                if let Err(ending) = sessions(&sources).end(&cookie).await {
                    warn!(
                        error = format!("{ending:#}"),
                        "ending a sign-in whose cookie the browser never got failed"
                    );
                }
                Err(e)
            }
        },
        Ok(Err(refusal)) => Ok(Err(refusal)),
        Err(e) => Err(e),
    };
    match decided {
        Ok(Ok(cookie)) => (
            [(header::SET_COOKIE, cookie.set_cookie())],
            Redirect::to(return_to.as_str()),
        )
            .into_response(),
        Ok(Err(refusal)) => {
            let message = Message {
                text: refusal.to_string(),
                error: true,
            };
            let page = SignInPage::new(&sign_in, &sources, return_to, Some(message));
            (StatusCode::FORBIDDEN, page).into_response()
        }
        Err(e) => failed(&sign_in, &sources, provider, return_to, &e),
    }
}

/// Records that `person` signed in at `now` for the authorization request
/// whose Consent `return_to` is, if it is one's: each authorization takes
/// its own sign-in (`docs/specs/access.md`, The authorization server, 3).
async fn signed_in_for(
    sources: &Sources,
    return_to: &ReturnPath,
    person: PersonId,
    now: Timestamp,
) -> anyhow::Result<()> {
    let Some(secret) = super::authorization_server::waiting_request(return_to) else {
        return Ok(());
    };
    AuthorizationRequests::new(sources.runs.database().clone())
        .signed_in(secret, person, now)
        .await
}

/// Signs a browser in as the person of `claims`, as Linewright decides
/// (`docs/specs/access.md`, Signing in, 3a to 3e), giving its cookie and
/// the person, or else why Linewright refuses, which it logs.
async fn sign_in_as_decided(
    sign_in: &SignIn,
    sources: &Sources,
    rule: &SignUpRule<'_>,
    claims: &Claims,
) -> anyhow::Result<Result<(SessionCookie, PersonId), Refusal>> {
    let now = sources.clock.now();
    let people = People::new(sources.runs.database().clone());
    let found = people.find_by_identity(&claims.identity()).await?;
    let person = match decide_sign_in(claims, rule, sign_in.manager.as_ref(), found.as_ref()) {
        SignInDecision::Refuse(refusal) => {
            info!(identity = claims.logged(), reason = %refusal, "refused a sign-in");
            return Ok(Err(refusal));
        }
        SignInDecision::Add { role } => people.add(claims, role, now).await?,
        SignInDecision::SignIn {
            person,
            make_manager,
        } => {
            people
                .record_sign_in(person, claims, make_manager, now)
                .await?
        }
    };
    let cookie = sessions(sources).start(person.id, now).await?;
    info!(
        person = %person.id,
        identity = person.logged(),
        role = person.role.name(),
        "signed in"
    );
    Ok(Ok((cookie, person.id)))
}

/// Sign in, saying that signing in with `provider` failed for `e`, which it
/// logs (`docs/specs/access.md`, Signing in, 2a).
fn failed(
    sign_in: &SignIn,
    sources: &Sources,
    provider: Provider,
    return_to: ReturnPath,
    e: &anyhow::Error,
) -> Response {
    warn!(
        provider = provider.name(),
        error = format!("{e:#}"),
        "a sign-in failed"
    );
    let message = Message {
        text: format!("Signing in with {} failed.", provider.name()),
        error: true,
    };
    let page = SignInPage::new(sign_in, sources, return_to, Some(message));
    (StatusCode::BAD_GATEWAY, page).into_response()
}

/// Signs the browser out, for a request from the web UI's own pages, and
/// sends it to Sign in saying so, whether or not it still signed someone
/// in.
pub(super) async fn sign_out(
    State(sources): State<Sources>,
    cross_site: Option<Extension<CrossSite>>,
    headers: HeaderMap,
) -> Response {
    if let Some(refused) = refused_from_another_site(cross_site, "a sign-out") {
        return refused;
    }
    if let Some(cookie) = session_cookie(&headers) {
        match sessions(&sources).end(&cookie).await {
            Ok(Some(person)) => {
                info!(person = %person.id, identity = person.logged(), "signed out");
            }
            Ok(None) => {}
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        }
    }
    (
        [(header::SET_COOKIE, SessionCookie::removed())],
        Redirect::to(&sign_in_path(
            &ReturnPath::parse(None),
            Some(Reason::SignedOut),
        )),
    )
        .into_response()
}

/// The refusal, which it logs, of `what`, such as "a sign-out", for a
/// request that did not come from the web UI's own pages, if it is one.
pub(super) fn refused_from_another_site(
    cross_site: Option<Extension<CrossSite>>,
    what: &str,
) -> Option<Response> {
    let Extension(CrossSite { reason }) = cross_site?;
    warn!(reason, "refused {what} from another site");
    let said = format!("{what} is taken only from the web UI's own pages");
    Some((StatusCode::FORBIDDEN, said).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn a_browser_returns_only_to_a_path_of_the_web_ui() {
        for (given, kept) in [
            (Some("/runs?state=failed"), "/runs?state=failed"),
            (Some("/runs/1%2Ftriage-1"), "/runs/1%2Ftriage-1"),
            (Some("/"), "/"),
            (None, "/"),
            (Some("https://evil.example/"), "/"),
            (Some("//evil.example/"), "/"),
            (Some("/\\evil.example/"), "/"),
            (Some("runs"), "/"),
            (Some("/runs\n"), "/"),
            (Some("/runs é"), "/"),
            (Some("/é"), "/"),
        ] {
            assert_eq!(ReturnPath::parse(given).as_str(), kept, "{given:?}");
        }
    }
}
