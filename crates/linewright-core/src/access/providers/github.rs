//! GitHub, as Linewright's OAuth client of it: the person it signs in, read
//! from GitHub's API with the token it gives, which Linewright then revokes
//! (`docs/specs/access.md`, Signing in, GitHub's token).

use anyhow::{Context as _, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::warn;
use url::Url;

use super::{
    Begun, ClientId, ClientSecret, MemberOf, PROVIDER_TIMEOUT, SignUpOrgs, given_name, https_avatar,
};
use crate::access::people::{Claims, Vouched};
use crate::access::tokens::s256;
use crate::http::next_link;

/// GitHub, as Linewright's OAuth client of it.
#[derive(Clone, Debug)]
pub struct GitHub {
    pub client_id: ClientId,
    pub secret: ClientSecret,
    pub signup: SignUpOrgs,
    pub endpoints: GitHubEndpoints,
}

/// Where GitHub answers: GitHub itself, or a fake of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubEndpoints {
    /// Where a browser is sent to sign in.
    pub authorize: Url,
    /// Where a code is exchanged for a token.
    pub token: Url,
    /// The root of its REST API, ending in `/`.
    pub api: Url,
}

/// GitHub's own:
/// <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#web-application-flow>
impl Default for GitHubEndpoints {
    fn default() -> GitHubEndpoints {
        let url = |url| Url::parse(url).expect("GitHub's URL parses");
        GitHubEndpoints {
            authorize: url("https://github.com/login/oauth/authorize"),
            token: url("https://github.com/login/oauth/access_token"),
            api: url("https://api.github.com/"),
        }
    }
}

/// The version of GitHub's REST API Linewright reads.
/// <https://docs.github.com/en/rest/about-the-rest-api/api-versions>
const API_VERSION: &str = "2022-11-28";

/// The most pages of a list of memberships Linewright reads, 100 to a page.
const MOST_PAGES: usize = 10;

impl GitHub {
    /// GitHub of `client_id` `test` and the secret `s3cret`, with the
    /// sign-up rule `signup`, at GitHub's own endpoints.
    #[cfg(test)]
    pub(crate) fn for_tests(signup: SignUpOrgs) -> GitHub {
        GitHub {
            client_id: ClientId::new("test".into()).unwrap(),
            secret: ClientSecret::new("s3cret".into()).unwrap(),
            signup,
            endpoints: GitHubEndpoints::default(),
        }
    }

    /// Where to send a browser to sign in with GitHub and come back to
    /// `callback`, for the sign-in `begun`, with S256 PKCE, asking for
    /// `read:org` while the sign-up rule names an organization or a team,
    /// and for no scope otherwise.
    /// <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#1-request-a-users-github-identity>
    pub fn authorize_url(&self, callback: &Url, begun: &Begun) -> Url {
        let mut url = self.endpoints.authorize.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("client_id", self.client_id.as_str())
                .append_pair("redirect_uri", callback.as_str())
                .append_pair("state", begun.state)
                .append_pair("code_challenge", &s256(begun.verifier))
                .append_pair("code_challenge_method", "S256")
                .append_pair("prompt", "select_account");
            if !self.signup.is_empty() {
                query.append_pair("scope", "read:org");
            }
        }
        url
    }

    /// Exchanges `code`, which GitHub sent back to `callback` for the
    /// sign-in `begun`, for a token; reads the person with it, and their
    /// memberships while the sign-up rule names any; then revokes it, even
    /// when a read fails or the caller stops waiting.
    pub async fn finish(
        &self,
        http: &reqwest::Client,
        code: &str,
        callback: &Url,
        begun: &Begun<'_>,
    ) -> anyhow::Result<Claims> {
        let github = self.clone();
        let http = http.clone();
        let (code, callback, verifier) = (
            code.to_string(),
            callback.clone(),
            begun.verifier.to_string(),
        );
        tokio::spawn(async move {
            let token = github.token(&http, &code, &callback, &verifier).await?;
            let read = github.read(&http, &token).await;
            if let Err(e) = github.revoke(&http, &token).await {
                warn!(error = format!("{e:#}"), "could not revoke GitHub's token");
            }
            read
        })
        .await
        .context("signing in with GitHub")?
    }

    /// The token GitHub gives for `code`, or the error it answers, which
    /// it answers with 200 OK.
    /// <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#2-users-are-redirected-back-to-your-site-by-github>
    async fn token(
        &self,
        http: &reqwest::Client,
        code: &str,
        callback: &Url,
        verifier: &str,
    ) -> anyhow::Result<String> {
        let form = serde_urlencoded::to_string([
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.secret.expose()),
            ("code", code),
            ("redirect_uri", callback.as_str()),
            ("code_verifier", verifier),
        ])
        .context("writing the token request")?;
        let response = http
            .post(self.endpoints.token.clone())
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .body(form)
            .timeout(PROVIDER_TIMEOUT)
            .send()
            .await
            .context("asking GitHub for a token")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("reading GitHub's token response")?;
        if !status.is_success() {
            bail!("GitHub answered the token request with {status}: {body}");
        }
        match serde_json::from_str(&body).context("reading GitHub's token response")? {
            TokenAnswer::Refused {
                error,
                error_description,
            } => bail!(
                "GitHub refused the code with {error}: {}",
                error_description.unwrap_or_default()
            ),
            TokenAnswer::Granted { access_token } => Ok(access_token),
        }
    }

    /// The person `token` is GitHub's token of, and their memberships while
    /// the sign-up rule names any.
    async fn read(&self, http: &reqwest::Client, token: &str) -> anyhow::Result<Claims> {
        let (user, _): (User, _) = self
            .get(http, token, self.api("user"))
            .await
            .context("reading the user")?;
        let memberships = if self.signup.is_empty() {
            vec![]
        } else {
            let organizations: Vec<Organization> = self
                .list(http, token, "user/orgs")
                .await
                .context("reading the user's organizations")?;
            let teams: Vec<Team> = self
                .list(http, token, "user/teams")
                .await
                .context("reading the user's teams")?;
            organizations
                .iter()
                .map(|organization| MemberOf::organization(&organization.login))
                .chain(
                    teams
                        .iter()
                        .map(|team| MemberOf::team(&team.organization.login, &team.slug)),
                )
                .collect()
        };
        Ok(Claims {
            subject: user.id.to_string(),
            shown_as: user.login,
            name: given_name(user.name),
            avatar: https_avatar(user.avatar_url),
            vouched: Vouched::GitHub { memberships },
        })
    }

    /// The URL of `path` in GitHub's API.
    fn api(&self, path: &str) -> Url {
        self.endpoints
            .api
            .join(path)
            .expect("a path joins GitHub's API")
    }

    /// Every page of the list at `path` in GitHub's API, each named by the
    /// `Link` of the one before, up to [`MOST_PAGES`]; never a page outside
    /// GitHub's API, which would be sent the token.
    /// <https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api>
    async fn list<T: DeserializeOwned>(
        &self,
        http: &reqwest::Client,
        token: &str,
        path: &str,
    ) -> anyhow::Result<Vec<T>> {
        let mut url = self.api(path);
        url.query_pairs_mut().append_pair("per_page", "100");
        let mut all = vec![];
        for _ in 0..MOST_PAGES {
            let (page, next): (Vec<T>, _) = self.get(http, token, url).await?;
            all.extend(page);
            let Some(next) = next else {
                return Ok(all);
            };
            url = Url::parse(&next).with_context(|| format!("reading the next page {next}"))?;
            if !self.in_api(&url) {
                bail!("GitHub names its next page at {url}, outside its API");
            }
        }
        bail!("the list is longer than {MOST_PAGES} pages")
    }

    /// Whether `url` is in GitHub's API: of its origin, and under its path.
    fn in_api(&self, url: &Url) -> bool {
        let api = &self.endpoints.api;
        url.origin() == api.origin() && url.path().starts_with(api.path())
    }

    /// What GitHub's API answers `url` with `token`, and the next page its
    /// `Link` names, if it names one.
    async fn get<T: DeserializeOwned>(
        &self,
        http: &reqwest::Client,
        token: &str,
        url: Url,
    ) -> anyhow::Result<(T, Option<String>)> {
        let response = api_request(http.get(url.clone()).bearer_auth(token))
            .send()
            .await
            .with_context(|| format!("asking GitHub for {url}"))?;
        let status = response.status();
        let next = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|link| link.to_str().ok())
            .and_then(next_link);
        let body = response
            .text()
            .await
            .with_context(|| format!("reading GitHub's answer for {url}"))?;
        if !status.is_success() {
            bail!("GitHub answered {url} with {status}: {body}");
        }
        let read = serde_json::from_str(&body)
            .with_context(|| format!("reading GitHub's answer for {url}"))?;
        Ok((read, next))
    }

    /// Revokes `token`, so that nothing can use it after the sign-in.
    /// <https://docs.github.com/en/rest/apps/oauth-applications#delete-an-app-token>
    async fn revoke(&self, http: &reqwest::Client, token: &str) -> anyhow::Result<()> {
        let mut url = self.endpoints.api.clone();
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("GitHub's API is no base URL"))?
            .pop_if_empty()
            .extend(["applications", self.client_id.as_str(), "token"]);
        let response = api_request(
            http.delete(url)
                .basic_auth(self.client_id.as_str(), Some(self.secret.expose()))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::json!({ "access_token": token }).to_string()),
        )
        .send()
        .await
        .context("asking GitHub to revoke the token")?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "GitHub answered the revocation with {status}: {}",
                response.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }
}

/// `request` to GitHub's REST API, as it asks to be sent:
/// <https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api>
fn api_request(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION)
        .header(reqwest::header::USER_AGENT, "linewright")
        .timeout(PROVIDER_TIMEOUT)
}

/// GitHub's answer to a token request: an error, or the token.
#[derive(Deserialize)]
#[serde(untagged)]
enum TokenAnswer {
    Refused {
        error: String,
        error_description: Option<String>,
    },
    Granted {
        access_token: String,
    },
}

/// What Linewright reads of `GET /user`.
/// <https://docs.github.com/en/rest/users/users#get-the-authenticated-user>
#[derive(Deserialize)]
struct User {
    id: u64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// What Linewright reads of an organization of `GET /user/orgs`.
/// <https://docs.github.com/en/rest/orgs/orgs#list-organizations-for-the-authenticated-user>
#[derive(Deserialize)]
struct Organization {
    login: String,
}

/// What Linewright reads of a team of `GET /user/teams`.
/// <https://docs.github.com/en/rest/teams/teams#list-teams-for-the-authenticated-user>
#[derive(Deserialize)]
struct Team {
    slug: String,
    organization: Organization,
}

#[cfg(test)]
mod tests {
    use std::panic::Location;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::IntoResponse;

    use super::*;
    use crate::access::people::{Claims, Vouched};
    use crate::access::providers::{Begun, MemberOf};

    /// GitHub's documented answer to a token request:
    /// <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#2-users-are-redirected-back-to-your-site-by-github>
    const TOKEN: &str = r#"{"access_token":"gho_16C7e42F292c6912E7710c838347Ae178B4a", "scope":"repo,gist", "token_type":"bearer"}"#;

    /// GitHub's documented answer to a code that is wrong or expired, which
    /// it gives with 200 OK:
    /// <https://docs.github.com/en/apps/oauth-apps/maintaining-oauth-apps/troubleshooting-oauth-app-access-token-request-errors#bad-verification-code>
    const BAD_CODE: &str = r#"{"error":"bad_verification_code","error_description":"The code passed is incorrect or expired.","error_uri":"https://docs.github.com/apps/managing-oauth-apps/troubleshooting-oauth-app-access-token-request-errors/#bad-verification-code"}"#;

    /// The examples of GitHub's REST API description
    /// (<https://github.com/github/rest-api-description>) for `GET /user`,
    /// `GET /user/orgs` and `GET /user/teams`.
    const USER: &str = include_str!("fixtures/github-user.json");
    const ORGS: &str = include_str!("fixtures/github-user-orgs.json");
    const TEAMS: &str = include_str!("fixtures/github-user-teams.json");

    /// A request the fake was sent: its method, its path and query, its
    /// `Authorization` and its body.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Sent {
        method: Method,
        path: String,
        authorization: Option<String>,
        body: String,
    }

    /// What GitHub answers a request, by its method, its path and query,
    /// and the origin GitHub is served at.
    type Answer = fn(&Method, &str, &str) -> (StatusCode, HeaderMap, String);

    /// GitHub, answering each request by `answer`, and the requests it was
    /// sent.
    #[track_caller]
    fn github(answer: Answer) -> impl Future<Output = (GitHubEndpoints, Arc<Mutex<Vec<Sent>>>)> {
        let caller = Location::caller();
        async move {
            let sent = Arc::new(Mutex::new(vec![]));
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|e| panic!("serving GitHub at {caller}: {e}"));
            let address = socket
                .local_addr()
                .unwrap_or_else(|e| panic!("serving GitHub at {caller}: {e}"));
            let origin = format!("http://{address}");
            let router = Router::new().fallback({
                let sent = sent.clone();
                move |method: Method, uri: Uri, headers: HeaderMap, body: String| async move {
                    let path = uri.path_and_query().map(ToString::to_string).unwrap();
                    sent.lock().unwrap().push(Sent {
                        method: method.clone(),
                        path: path.clone(),
                        authorization: headers
                            .get("authorization")
                            .map(|value| value.to_str().unwrap().to_string()),
                        body,
                    });
                    answer(&method, &path, &origin).into_response()
                }
            });
            tokio::spawn(async move { axum::serve(socket, router).await });
            let url = |path: &str| Url::parse(&format!("http://{address}{path}")).unwrap();
            let endpoints = GitHubEndpoints {
                authorize: url("/login/oauth/authorize"),
                token: url("/login/oauth/access_token"),
                api: url("/api/"),
            };
            (endpoints, sent)
        }
    }

    /// GitHub answering as its documentation shows.
    fn documented(method: &Method, path: &str, _: &str) -> (StatusCode, HeaderMap, String) {
        let body = match (method.as_str(), path.split('?').next().unwrap()) {
            ("POST", "/login/oauth/access_token") => TOKEN,
            ("GET", "/api/user") => USER,
            ("GET", "/api/user/orgs") => ORGS,
            ("GET", "/api/user/teams") => TEAMS,
            ("DELETE", "/api/applications/Iv1.8a61f9b3a7aba766/token") => {
                return (StatusCode::NO_CONTENT, HeaderMap::new(), String::new());
            }
            _ => return (StatusCode::NOT_FOUND, HeaderMap::new(), String::new()),
        };
        (StatusCode::OK, HeaderMap::new(), body.to_string())
    }

    #[track_caller]
    fn client(endpoints: GitHubEndpoints, signup: &[&str]) -> GitHub {
        GitHub {
            client_id: ClientId::new("Iv1.8a61f9b3a7aba766".into()).unwrap(),
            endpoints,
            ..GitHub::for_tests(SignUpOrgs::of_all(signup))
        }
    }

    /// A sign-in begun with the code verifier of RFC 7636, Appendix B.
    const BEGUN: Begun = Begun {
        state: "state",
        verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        nonce: "unused",
    };

    #[track_caller]
    fn callback() -> Url {
        Url::parse("https://lw.example.com/auth/github/callback").unwrap()
    }

    /// The `Authorization` GitHub's API is sent with the token of [`TOKEN`].
    const BEARER: &str = "Bearer gho_16C7e42F292c6912E7710c838347Ae178B4a";

    /// The revocation of [`TOKEN`]'s token, by the client ID and secret.
    /// <https://docs.github.com/en/rest/apps/oauth-applications#delete-an-app-token>
    fn revoked() -> Sent {
        Sent {
            method: Method::DELETE,
            path: "/api/applications/Iv1.8a61f9b3a7aba766/token".into(),
            authorization: Some("Basic SXYxLjhhNjFmOWIzYTdhYmE3NjY6czNjcmV0".into()),
            body: r#"{"access_token":"gho_16C7e42F292c6912E7710c838347Ae178B4a"}"#.into(),
        }
    }

    fn read(path: &str) -> Sent {
        Sent {
            method: Method::GET,
            path: path.into(),
            authorization: Some(BEARER.into()),
            body: String::new(),
        }
    }

    #[test_log::test(tokio::test)]
    async fn a_finished_sign_in_gives_the_user_and_their_memberships_and_revokes_the_token() {
        let (endpoints, sent) = github(documented).await;

        let claims = client(endpoints, &["github/justice-league"])
            .finish(&reqwest::Client::new(), "a1b2c3", &callback(), &BEGUN)
            .await
            .unwrap();

        assert_eq!(
            claims,
            Claims {
                subject: "1".into(),
                shown_as: "octocat".into(),
                name: Some("monalisa octocat".into()),
                avatar: Some(
                    Url::parse("https://github.com/images/error/octocat_happy.gif").unwrap()
                ),
                vouched: Vouched::GitHub {
                    memberships: vec![
                        MemberOf::organization("github"),
                        MemberOf::team("github", "justice-league"),
                    ]
                },
            }
        );
        let sent = sent.lock().unwrap().clone();
        let form: Vec<(String, String)> = serde_urlencoded::from_str(&sent[0].body).unwrap();
        assert_eq!(
            form,
            [
                ("client_id", "Iv1.8a61f9b3a7aba766"),
                ("client_secret", "s3cret"),
                ("code", "a1b2c3"),
                (
                    "redirect_uri",
                    "https://lw.example.com/auth/github/callback"
                ),
                (
                    "code_verifier",
                    "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
                ),
            ]
            .map(|(name, value)| (name.to_string(), value.to_string()))
        );
        assert_eq!(
            sent[1..],
            [
                read("/api/user"),
                read("/api/user/orgs?per_page=100"),
                read("/api/user/teams?per_page=100"),
                revoked(),
            ]
        );
    }

    #[test_log::test(tokio::test)]
    async fn without_a_sign_up_rule_no_membership_is_read_and_the_token_is_revoked() {
        let (endpoints, sent) = github(documented).await;

        let claims = client(endpoints, &[])
            .finish(&reqwest::Client::new(), "a1b2c3", &callback(), &BEGUN)
            .await
            .unwrap();

        assert_eq!(
            claims.vouched,
            Vouched::GitHub {
                memberships: vec![]
            }
        );
        assert_eq!(sent.lock().unwrap()[1..], [read("/api/user"), revoked()]);
    }

    #[test_log::test(tokio::test)]
    async fn an_error_github_answers_with_200_fails_the_sign_in_with_its_text() {
        let (endpoints, sent) = github(|method, path, origin| {
            if path == "/login/oauth/access_token" {
                (StatusCode::OK, HeaderMap::new(), BAD_CODE.into())
            } else {
                documented(method, path, origin)
            }
        })
        .await;

        let failed = client(endpoints, &["github"])
            .finish(&reqwest::Client::new(), "a1b2c3", &callback(), &BEGUN)
            .await
            .unwrap_err();

        assert!(
            format!("{failed:#}").contains("The code passed is incorrect or expired."),
            "{failed:#}"
        );
        assert_eq!(sent.lock().unwrap().len(), 1);
    }

    #[test_log::test(tokio::test)]
    async fn a_read_that_fails_fails_the_sign_in_and_the_token_is_revoked_all_the_same() {
        let (endpoints, sent) = github(|method, path, origin| {
            if path == "/api/user/teams?per_page=100" {
                let body = r#"{"message":"Bad credentials","documentation_url":"https://docs.github.com/rest","status":"401"}"#;
                (StatusCode::UNAUTHORIZED, HeaderMap::new(), body.into())
            } else {
                documented(method, path, origin)
            }
        })
        .await;

        let failed = client(endpoints, &["github"])
            .finish(&reqwest::Client::new(), "a1b2c3", &callback(), &BEGUN)
            .await
            .unwrap_err();

        assert!(
            format!("{failed:#}").contains("Bad credentials"),
            "{failed:#}"
        );
        assert_eq!(sent.lock().unwrap().last(), Some(&revoked()));
    }

    /// The answer to `GET /user/orgs` in two pages, the first naming the
    /// second, at `next_origin`, in its `Link`:
    /// <https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api#using-link-headers>
    fn paged(
        method: &Method,
        path: &str,
        origin: &str,
        next_origin: &str,
    ) -> (StatusCode, HeaderMap, String) {
        match path {
            "/api/user/orgs?per_page=100" => {
                let mut headers = HeaderMap::new();
                let link = format!(
                    "<{next_origin}/api/user/orgs?per_page=100&page=2>; rel=\"next\", \
                     <{next_origin}/api/user/orgs?per_page=100&page=2>; rel=\"last\""
                );
                headers.insert("link", link.parse().unwrap());
                (StatusCode::OK, headers, "[]".into())
            }
            "/api/user/orgs?per_page=100&page=2" => (StatusCode::OK, HeaderMap::new(), ORGS.into()),
            _ => documented(method, path, origin),
        }
    }

    #[test_log::test(tokio::test)]
    async fn the_memberships_are_read_from_every_page() {
        let (endpoints, sent) =
            github(|method, path, origin| paged(method, path, origin, origin)).await;

        let claims = client(endpoints, &["github"])
            .finish(&reqwest::Client::new(), "a1b2c3", &callback(), &BEGUN)
            .await
            .unwrap();

        assert_eq!(
            claims.vouched,
            Vouched::GitHub {
                memberships: vec![
                    MemberOf::organization("github"),
                    MemberOf::team("github", "justice-league"),
                ]
            }
        );
        assert_eq!(sent.lock().unwrap().last(), Some(&revoked()));
    }

    /// The token goes to GitHub's API alone, whatever a `Link` names.
    #[test_log::test(tokio::test)]
    async fn a_next_page_elsewhere_than_githubs_api_fails_the_sign_in_unread() {
        let (endpoints, sent) =
            github(|method, path, origin| paged(method, path, origin, "https://evil.example"))
                .await;

        let failed = client(endpoints, &["github"])
            .finish(&reqwest::Client::new(), "a1b2c3", &callback(), &BEGUN)
            .await
            .unwrap_err();

        assert!(format!("{failed:#}").contains("evil.example"), "{failed:#}");
        assert_eq!(sent.lock().unwrap().last(), Some(&revoked()));
    }

    #[test_log::test]
    fn the_authorization_request_asks_for_read_org_only_with_a_sign_up_rule() {
        let query = |signup: &[&str]| -> Vec<(String, String)> {
            let url = client(GitHubEndpoints::default(), signup).authorize_url(&callback(), &BEGUN);
            assert_eq!(
                url.as_str().split('?').next(),
                Some("https://github.com/login/oauth/authorize")
            );
            url.query_pairs().into_owned().collect()
        };
        let pairs = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect()
        };

        assert_eq!(
            query(&["acme"]),
            pairs(&[
                ("client_id", "Iv1.8a61f9b3a7aba766"),
                (
                    "redirect_uri",
                    "https://lw.example.com/auth/github/callback"
                ),
                ("state", "state"),
                (
                    "code_challenge",
                    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
                ),
                ("code_challenge_method", "S256"),
                ("prompt", "select_account"),
                ("scope", "read:org"),
            ])
        );
        assert_eq!(
            query(&[]),
            pairs(&[
                ("client_id", "Iv1.8a61f9b3a7aba766"),
                (
                    "redirect_uri",
                    "https://lw.example.com/auth/github/callback"
                ),
                ("state", "state"),
                (
                    "code_challenge",
                    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
                ),
                ("code_challenge_method", "S256"),
                ("prompt", "select_account"),
            ])
        );
    }
}
