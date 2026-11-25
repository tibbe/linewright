//! Signing in to the web UI with Google and GitHub (`docs/specs/access.md`,
//! Signing in and Sign in; `docs/specs/access.md`, Sign in), and what
//! each role is shown and allowed (`docs/specs/access.md`, Roles and
//! Actions), through the router, against fakes of Google and GitHub served
//! over HTTP.

use std::collections::BTreeMap;

use linewright_core::access::people::People;
use linewright_core::access::providers::fake::{
    FakeAccount, FakeGitHub, FakeGitHubAccount, FakeGoogle,
};
use linewright_core::access::providers::{
    ClientId, ClientSecret, GitHub, Google, Provider, Providers, SignUpDomains, SignUpOrgs,
};

use super::*;

/// The avatar Google gives The Boss.
const BOSS_AVATAR: &str = "https://lh3.googleusercontent.com/a/boss=s96-c";

/// The accounts the fake Google signs in.
fn accounts() -> Vec<FakeAccount> {
    let account = |subject: &str, email: &str, name: Option<&str>| FakeAccount {
        subject: subject.into(),
        email: email.into(),
        email_verified: true,
        name: name.map(Into::into),
        picture: None,
    };
    vec![
        account("2002", "nameless@example.com", None),
        account("2003", "mallory@elsewhere.example", Some("Mallory")),
        account(
            "subject-of-grace",
            "grace@example.com",
            Some("Grace Hopper"),
        ),
        FakeAccount {
            picture: Some(BOSS_AVATAR.into()),
            ..account("2005", "boss@elsewhere.example", Some("The Boss"))
        },
    ]
}

/// The avatar GitHub gives The Octocat.
const OCTOCAT_AVATAR: &str = "https://avatars.githubusercontent.com/u/583231?v=4";

/// The accounts the fake GitHub signs in.
fn github_accounts() -> Vec<FakeGitHubAccount> {
    vec![
        FakeGitHubAccount {
            id: 583_231,
            login: "OctoCat".into(),
            name: Some("The Octocat".into()),
            avatar_url: Some(OCTOCAT_AVATAR.into()),
            memberships: vec!["acme/core".into()],
        },
        FakeGitHubAccount {
            id: 1_000_001,
            login: "outsider".into(),
            name: None,
            avatar_url: None,
            memberships: vec!["elsewhere".into()],
        },
        FakeGitHubAccount {
            id: 1_000_002,
            login: "builder".into(),
            name: Some("Bob the Builder".into()),
            avatar_url: None,
            memberships: vec!["guild/builders".into()],
        },
    ]
}

/// The identity providers a [`Signing`] offers.
#[derive(Clone, Copy)]
enum Offering {
    Both,
    GitHubAlone,
}

/// The pages, signing people in with the fake Google, whose sign-up rule is
/// `example.com`, and the fake GitHub, whose sign-up rule is `acme` and
/// `guild/builders`, as
/// `offering` offers them, with `manager` as the `manager` setting.
pub(super) struct Signing {
    pub(super) pages: Pages,
    router: Router,
    _google: FakeGoogle,
    github: FakeGitHub,
}

pub(super) async fn signing(manager: Option<&str>) -> Signing {
    signing_offering(Offering::Both, manager).await
}

async fn signing_offering(offering: Offering, manager: Option<&str>) -> Signing {
    let pages = Pages::of(a_ticket()).await;
    let google = FakeGoogle::serve(accounts(), pages.sources.clock.clone())
        .await
        .unwrap_or_else(|e| panic!("serving the fake Google: {e:#}"));
    let github = FakeGitHub::serve(github_accounts())
        .await
        .unwrap_or_else(|e| panic!("serving the fake GitHub: {e:#}"));
    let github_settings = GitHub {
        client_id: ClientId::new("github-id".into()).unwrap(),
        secret: ClientSecret::new("github-s3cret".into()).unwrap(),
        signup: SignUpOrgs::of("acme")
            .unwrap()
            .and(&SignUpOrgs::of("guild/builders").unwrap()),
        endpoints: github.endpoints(),
    };
    let google_settings = Google {
        signup: SignUpDomains::of("example.com").unwrap(),
        endpoints: google.endpoints(),
        ..ui().providers.google().unwrap().clone()
    };
    let providers = match offering {
        Offering::Both => Providers::Both(google_settings, github_settings),
        Offering::GitHubAlone => Providers::GitHub(github_settings),
    };
    let manager = manager.map(|name| {
        linewright_core::access::people::ManagerName::parse(name, &providers)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
    });
    let ui = linewright_core::config::Ui {
        providers,
        manager,
        ..ui()
    };
    let router = linewright_web::router(pages.sources.clone(), &ui, pages.stopping.clone());
    Signing {
        pages,
        router,
        _google: google,
        github,
    }
}

/// What a browser was answered: the status, the headers and the body.
pub(super) struct Answer {
    pub(super) status: u16,
    pub(super) headers: axum::http::HeaderMap,
    pub(super) html: Html,
}

impl Answer {
    pub(super) fn location(&self) -> &str {
        self.headers["location"].to_str().unwrap()
    }

    /// Each `Set-Cookie`.
    pub(super) fn set_cookies(&self) -> Vec<&str> {
        self.headers
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect()
    }
}

/// A browser on the web UI, keeping the cookies it is given.
pub(super) struct Browsing<'a> {
    router: &'a Router,
    cookies: BTreeMap<String, String>,
}

impl<'a> Browsing<'a> {
    pub(super) fn new(signing: &'a Signing) -> Browsing<'a> {
        Browsing {
            router: &signing.router,
            cookies: BTreeMap::new(),
        }
    }

    /// A browser that sends `cookie`, a `Cookie` header's value.
    pub(super) fn with_cookie(signing: &'a Signing, cookie: &str) -> Browsing<'a> {
        let cookies = cookie
            .split("; ")
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        Browsing {
            router: &signing.router,
            cookies,
        }
    }

    fn cookie(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub(super) async fn send(
        &mut self,
        request: axum::http::request::Builder,
        body: String,
    ) -> Answer {
        let request = request
            .header("Host", UI_HOST)
            .header("Cookie", self.cookie())
            .body(Body::from(body))
            .expect("a request builds");
        let Ok(response) = self.router.clone().oneshot(request).await;
        let headers = response.headers().clone();
        for set in headers.get_all("set-cookie") {
            let set = set.to_str().expect("a Set-Cookie is text");
            let (name, value) = set
                .split(';')
                .next()
                .and_then(|pair| pair.split_once('='))
                .expect("a Set-Cookie names its cookie");
            if set.contains("Max-Age=0") {
                self.cookies.remove(name);
            } else {
                self.cookies.insert(name.into(), value.into());
            }
        }
        let (status, answered) = status_and_body(response).await;
        Answer {
            status,
            headers,
            html: Html::parse_document(&answered),
        }
    }

    pub(super) async fn get(&mut self, path: &str) -> Answer {
        self.send(axum::http::Request::get(path), String::new())
            .await
    }

    /// Posts the form `fields` to `path` from a page of the web UI.
    pub(super) async fn post(&mut self, path: &str, fields: &[(&str, &str)]) -> Answer {
        let mut form = form_urlencoded::Serializer::new(String::new());
        form.extend_pairs(fields);
        let request = axum::http::Request::post(path)
            .header(SAME_ORIGIN.0, SAME_ORIGIN.1)
            .header("Content-Type", "application/x-www-form-urlencoded");
        self.send(request, form.finish()).await
    }

    /// Opens `path`, is sent to Sign in, and signs in there with Google,
    /// choosing the account of `email`, or turning the sign-in down with
    /// none: what the browser is answered as it is sent back.
    pub(super) async fn sign_in(&mut self, path: &str, email: Option<&str>) -> Answer {
        let authorize = self.begin(path, Provider::Google).await;
        self.choose(FakeGoogle::choosing(&authorize, email), Provider::Google)
            .await
    }

    /// Opens `path`, is sent to Sign in, and signs in there with GitHub,
    /// choosing the account of `login`, or turning the sign-in down with
    /// none: what the browser is answered as it is sent back.
    async fn sign_in_with_github(&mut self, path: &str, login: Option<&str>) -> Answer {
        let authorize = self.begin(path, Provider::GitHub).await;
        self.choose(FakeGitHub::choosing(&authorize, login), Provider::GitHub)
            .await
    }

    /// Opens `path`, is sent to Sign in or shown it in the screen's place,
    /// and begins a sign-in there with `provider`: the provider's URL it is
    /// sent to.
    async fn begin(&mut self, path: &str, provider: Provider) -> url::Url {
        let opened = self.get(path).await;
        let sign_in = match opened.status {
            303 => self.get(opened.location()).await,
            _ => opened,
        };
        assert_eq!(sign_in.status, 200, "{path}");
        let begun = self.begin_on(&sign_in, provider).await;
        assert_eq!(begun.status, 303, "{}", begun.html.html());
        url::Url::parse(begun.location()).expect("the provider's URL")
    }

    /// Begins a sign-in with `provider` on `sign_in`, Sign in as it shows,
    /// posting its form as it is: what the browser is answered.
    pub(super) async fn begin_on(&mut self, sign_in: &Answer, provider: Provider) -> Answer {
        let action = format!("/auth/{provider}");
        let form = select(&sign_in.html, &format!("form[action='{action}']"));
        assert_eq!(form.len(), 1, "{}", sign_in.html.html());
        let fields: Vec<(String, String)> = select_in(form[0], "input[type=hidden]")
            .into_iter()
            .map(|input| {
                (
                    input.attr("name").unwrap().to_string(),
                    input.attr("value").unwrap().to_string(),
                )
            })
            .collect();
        let fields: Vec<(&str, &str)> = fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        self.post(&action, &fields).await
    }

    /// Signs in with Google on `sign_in`, Sign in as it shows, choosing the
    /// account of `email`: what the browser is answered as it is sent back.
    pub(super) async fn sign_in_on(&mut self, sign_in: &Answer, email: &str) -> Answer {
        let begun = self.begin_on(sign_in, Provider::Google).await;
        self.choose_at_google(&begun, email).await
    }

    /// Chooses the account of `email` at Google, where `begun`, a sign-in
    /// begun with Google, sent the browser: what the browser is answered as
    /// it is sent back.
    pub(super) async fn choose_at_google(&mut self, begun: &Answer, email: &str) -> Answer {
        assert_eq!(begun.status, 303, "{}", begun.html.html());
        let authorize = url::Url::parse(begun.location()).expect("the provider's URL");
        self.choose(
            FakeGoogle::choosing(&authorize, Some(email)),
            Provider::Google,
        )
        .await
    }

    /// Goes to `chosen`, where `provider` sends the browser back to its
    /// callback, and follows it there.
    async fn choose(&mut self, chosen: url::Url, provider: Provider) -> Answer {
        let chosen = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("an HTTP client")
            .get(chosen)
            .send()
            .await
            .unwrap_or_else(|e| panic!("choosing at the fake {}: {e}", provider.name()));
        assert_eq!(chosen.status(), 303);
        let back = url::Url::parse(chosen.headers()["location"].to_str().unwrap())
            .expect("the callback's URL");
        assert_eq!(back.path(), format!("/auth/{provider}/callback"));
        self.get(&format!(
            "{}?{}",
            back.path(),
            back.query().unwrap_or_default()
        ))
        .await
    }
}

/// The account at the foot of the navigation, as a reader reads it: its
/// name, its role and its avatar's image, or the letter in its place.
fn account(html: &Html) -> (String, String, String) {
    let account = select(html, "nav .account")[0];
    let avatar = match select_in(account, ".avatar img").first() {
        Some(image) => image.attr("src").unwrap_or_default().to_string(),
        None => text(select_in(account, ".avatar-placeholder")[0]),
    };
    (
        text(select_in(account, ".account-name")[0]),
        text(select_in(account, ".account-role")[0]),
        avatar,
    )
}

/// What Sign in says, and whether it says it as an error.
fn message(html: &Html) -> Option<(String, bool)> {
    let card = select(html, ".sign-in")[0];
    select_in(card, ".alert")
        .first()
        .map(|alert| (text(*alert), has_classes(*alert, &["alert-error"])))
}

#[test_log::test(tokio::test)]
async fn a_screen_opened_without_a_sign_in_goes_to_sign_in_and_back_to_it_after() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let (log, _guard) = Log::capture();

    let sent = browser.get("/runs?state=failed").await;
    assert_eq!(sent.status, 303);
    assert_eq!(sent.location(), "/sign-in?return=%2Fruns%3Fstate%3Dfailed");
    let sign_in = browser.get(sent.location()).await;
    assert_eq!(
        text(select(&sign_in.html, ".sign-in h1")[0]),
        "Sign in to Linewright"
    );
    assert_eq!(
        text(select(&sign_in.html, ".sign-in .repo-name")[0]),
        "owner/widgets"
    );
    assert_eq!(message(&sign_in.html), None);
    let back = browser
        .sign_in("/runs?state=failed", Some("nameless@example.com"))
        .await;

    assert_eq!(back.status, 303);
    assert_eq!(back.location(), "/runs?state=failed");
    let session = back
        .set_cookies()
        .into_iter()
        .find(|set| set.starts_with("__Host-linewright-session="))
        .expect("the session's cookie");
    let attributes: Vec<&str> = session.split("; ").skip(1).collect();
    assert_eq!(
        attributes,
        [
            "Path=/",
            "Max-Age=5184000",
            "Secure",
            "HttpOnly",
            "SameSite=Lax"
        ]
    );
    let runs = browser.get("/runs?state=failed").await;
    assert_eq!(runs.status, 200);
    assert_eq!(
        account(&runs.html),
        ("nameless@example.com".into(), "viewer".into(), "N".into())
    );
    let signed_in = log.lines("INFO");
    assert!(
        signed_in.iter().any(|line| line.contains("signed in")
            && line.contains(r#"identity="google:nameless@example.com""#)
            && line.contains(r#"role="viewer""#)),
        "{signed_in:?}"
    );
}

/// `docs/specs/access.md`, Sign in, Identity providers, Google's button and
/// GitHub's button.
#[test_log::test(tokio::test)]
async fn sign_in_offers_google_then_github_as_daisyuis_login_buttons() {
    let both = signing(None).await;
    let github_alone = signing_offering(Offering::GitHubAlone, None).await;

    let offered = |html: &Html| {
        select(html, ".sign-in form")
            .into_iter()
            .map(|form| form.attr("action").unwrap_or_default().to_string())
            .collect::<Vec<_>>()
    };
    let sign_in = Browsing::new(&both).get("/sign-in").await;
    assert_eq!(offered(&sign_in.html), ["/auth/google", "/auth/github"]);
    let google = select(&sign_in.html, "form[action='/auth/google'] button")[0];
    assert!(
        has_classes(
            google,
            &[
                "btn",
                "btn-md",
                "bg-white",
                "text-black",
                "border-[#e5e5e5]"
            ]
        ),
        "{}",
        google.html()
    );
    assert_eq!(text(google), "Sign in with Google");
    let g = select_in(google, "img:first-child")[0];
    assert_eq!(g.attr("data-logo"), Some("google"));
    assert!(has_classes(g, &["size-4", "shrink-0"]), "{}", g.html());
    assert_eq!(g.attr("alt"), Some(""));
    assert_eq!(
        (g.attr("width"), g.attr("height")),
        (Some("16"), Some("16"))
    );
    let src = g.attr("src").unwrap_or_default();
    let svg = src
        .strip_prefix("data:image/svg+xml,")
        .unwrap_or_else(|| panic!("{src}"));
    let spec_g = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/specs/assets/google-g.svg"
    ))
    .unwrap();
    assert_eq!(
        percent_encoding::percent_decode_str(svg)
            .decode_utf8()
            .unwrap(),
        spec_g.trim()
    );
    let github = select(&sign_in.html, "form[action='/auth/github'] button")[0];
    assert!(
        has_classes(
            github,
            &["btn", "btn-md", "bg-black", "text-white", "border-black"]
        ),
        "{}",
        github.html()
    );
    assert_eq!(text(github), "Sign in with GitHub");
    let mark = select_in(github, "svg:first-child")[0];
    assert_eq!(mark.attr("data-logo"), Some("github"));
    assert_eq!(mark.attr("fill"), Some("white"));
    assert_eq!(mark.attr("aria-hidden"), Some("true"));
    assert!(
        has_classes(mark, &["size-4", "shrink-0"]),
        "{}",
        mark.html()
    );
    let alone = Browsing::new(&github_alone).get("/sign-in").await;
    assert_eq!(offered(&alone.html), ["/auth/github"]);
    let mut not_offered = Browsing::new(&github_alone);
    assert_eq!(
        not_offered
            .post("/auth/google", &[("return", "/")])
            .await
            .status,
        404
    );
}

/// `docs/specs/access.md`, Signing in: GitHub's identity shown as its login,
/// a member of a team of an organization its sign-up rule names, and its
/// token revoked once read.
#[test_log::test(tokio::test)]
async fn a_member_signs_in_with_github_by_their_login_and_its_token_is_revoked() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let (log, _guard) = Log::capture();

    let back = browser
        .sign_in_with_github("/runs?state=failed", Some("OctoCat"))
        .await;

    assert_eq!(back.status, 303, "{}", back.html.html());
    assert_eq!(back.location(), "/runs?state=failed");
    let runs = browser.get("/runs?state=failed").await;
    assert_eq!(
        account(&runs.html),
        ("The Octocat".into(), "viewer".into(), OCTOCAT_AVATAR.into())
    );
    assert_eq!(signing.github.tokens_not_revoked(), 0);
    let signed_in = log.lines("INFO");
    assert!(
        signed_in.iter().any(|line| line.contains("signed in")
            && line.contains(r#"identity="github:OctoCat""#)
            && line.contains(r#"role="viewer""#)),
        "{signed_in:?}"
    );
    let mut outsider = Browsing::new(&signing);
    let refused = outsider
        .sign_in_with_github("/runs", Some("outsider"))
        .await;
    assert_eq!(refused.status, 403);
    assert_eq!(
        message(&refused.html),
        Some(("outsider has no access to this Linewright.".into(), true))
    );
    assert_eq!(signing.github.tokens_not_revoked(), 0);
}

/// `docs/specs/access.md`, Signing in, A team's members: a member of a team
/// the sign-up rule names signs in, read from the team's membership.
#[test_log::test(tokio::test)]
async fn a_member_of_a_team_the_rule_names_signs_in_with_github() {
    let signing = signing(None).await;
    let mut builder = Browsing::new(&signing);

    let back = builder.sign_in_with_github("/runs", Some("builder")).await;

    assert_eq!(back.status, 303, "{}", back.html.html());
    let runs = builder.get("/runs").await;
    assert_eq!(account(&runs.html).0, "Bob the Builder");
}

/// `docs/specs/access.md`, Signing in, 2a, at GitHub.
#[test_log::test(tokio::test)]
async fn a_sign_in_github_turns_down_fails() {
    let signing = signing(None).await;
    let mut turned_down = Browsing::new(&signing);

    let failed = turned_down.sign_in_with_github("/runs", None).await;

    assert_eq!(
        message(&failed.html),
        Some(("Signing in with GitHub failed.".into(), true))
    );
}

/// `docs/specs/access.md`, Signing in, 3b: the `manager` setting naming a
/// GitHub login, ignoring case.
#[test_log::test(tokio::test)]
async fn the_login_manager_names_signs_in_with_github_as_a_manager() {
    let signing = signing(Some("github:octocat")).await;
    let mut browser = Browsing::new(&signing);

    browser.sign_in_with_github("/", Some("OctoCat")).await;

    let board = browser.get("/").await;
    assert_eq!(
        account(&board.html),
        (
            "The Octocat".into(),
            "manager".into(),
            OCTOCAT_AVATAR.into()
        )
    );
}

/// `docs/specs/access.md`, Signing in, 3c, as each refusal is shown.
#[test_log::test(tokio::test)]
async fn a_refused_sign_in_says_why_on_sign_in_and_is_logged() {
    let signing = signing(None).await;
    let mut browser = Browsing::new(&signing);
    let (log, _guard) = Log::capture();

    let refused = browser
        .sign_in("/runs", Some("mallory@elsewhere.example"))
        .await;

    assert_eq!(refused.status, 403);
    let said = "mallory@elsewhere.example has no access to this Linewright.";
    assert_eq!(message(&refused.html), Some((said.to_string(), true)));
    assert!(!browser.cookies.contains_key("__Host-linewright-session"));
    assert_eq!(browser.get("/runs").await.status, 303);
    let logged = log.lines("INFO");
    assert!(
        logged.iter().any(|line| line.contains("refused a sign-in")
            && line.contains(r#"identity="google:mallory@elsewhere.example""#)
            && line.contains(said)),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, Signing in, 2a.
#[test_log::test(tokio::test)]
async fn a_sign_in_the_provider_refuses_or_that_does_not_come_back_to_its_browser_fails() {
    let signing = signing(None).await;
    let mut turned_down = Browsing::new(&signing);

    let failed = turned_down.sign_in("/runs", None).await;

    assert_eq!(
        message(&failed.html),
        Some(("Signing in with Google failed.".into(), true))
    );
    let mut began = Browsing::new(&signing);
    let begun = began.post("/auth/google", &[("return", "/")]).await;
    let authorize = url::Url::parse(begun.location()).unwrap();
    let state: String = authorize
        .query_pairs()
        .find(|(name, _)| name == "state")
        .unwrap()
        .1
        .into();
    let mut another = Browsing::new(&signing);
    let elsewhere = another
        .get(&format!("/auth/google/callback?code=any&state={state}"))
        .await;
    assert_eq!(
        message(&elsewhere.html),
        Some(("Signing in with Google failed.".into(), true))
    );
    assert!(!another.cookies.contains_key("__Host-linewright-session"));
}

/// `docs/specs/access.md`, Signing in, 3b, whatever the sign-up rule says;
/// People, How a person is shown, with their avatar.
#[test_log::test(tokio::test)]
async fn the_identity_manager_names_signs_in_as_a_manager() {
    let signing = signing(Some("google:boss@elsewhere.example")).await;
    let mut browser = Browsing::new(&signing);

    let back = browser.sign_in("/", Some("boss@elsewhere.example")).await;

    assert_eq!(back.status, 303);
    let board = browser.get("/").await;
    assert_eq!(
        account(&board.html),
        ("The Boss".into(), "manager".into(), BOSS_AVATAR.into())
    );
}

/// Sign in, as a screen of a browser whose sign-in ended shows it in its
/// place: what it says, and the path it returns to after the sign-in.
fn in_place_of_the_screen(answer: &Answer) -> (Option<(String, bool)>, String) {
    assert_eq!(answer.status, 200, "{}", answer.html.html());
    let return_to = select(&answer.html, ".sign-in form input[name=return]")[0]
        .attr("value")
        .expect("the path to return to")
        .to_string();
    (message(&answer.html), return_to)
}

/// The events a stream sends, as their text, until it ends.
async fn sent_until_it_ends(response: axum::response::Response) -> String {
    let mut body = response.into_body();
    let mut sent = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(frame)) = body.frame().await {
            if let Ok(data) = frame.into_data() {
                sent.extend_from_slice(&data);
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the stream still runs");
    String::from_utf8(sent).expect("a stream sends text")
}

/// What the event stream at `path` sends to the browser of `cookie`, until
/// it ends.
async fn streamed(signing: &Signing, path: &str, cookie: &str) -> String {
    let request = axum::http::Request::get(path)
        .header("Host", UI_HOST)
        .header("Cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let Ok(response) = signing.router.clone().oneshot(request).await;
    assert_eq!(response.status().as_u16(), 200);
    sent_until_it_ends(response).await
}

/// `docs/specs/access.md`, Signing in, Signed in and An open screen; Sign
/// in, The message.
#[test_log::test(tokio::test)]
async fn after_30_days_a_screen_shows_sign_in_saying_the_sign_in_ended() {
    let signing = signing(None).await;
    let cookie = signed_in_as(&signing.pages.sources, Role::Viewer, "Grace Hopper").await;
    let mut browser = Browsing::with_cookie(&signing, &cookie);

    let fresh = browser.get("/runs").await;
    signing.pages.clock.advance(chrono::TimeDelta::days(30));
    let ended = browser.get("/runs?state=failed").await;

    assert_eq!(fresh.status, 200);
    assert_eq!(
        in_place_of_the_screen(&ended),
        (
            Some(("Your sign-in ended.".into(), false)),
            "/runs?state=failed".into()
        )
    );
    assert!(ended.set_cookies().is_empty(), "{:?}", ended.set_cookies());
    let mut another_tab = Browsing::with_cookie(&signing, &cookie);
    assert_eq!(
        in_place_of_the_screen(&another_tab.get("/").await),
        (Some(("Your sign-in ended.".into(), false)), "/".into())
    );
    assert_eq!(
        streamed(&signing, "/events/runs", &cookie).await,
        "event: signed-out\n\n"
    );
    let back = browser.sign_in("/runs", Some("grace@example.com")).await;
    assert_eq!(back.location(), "/runs");
    assert_eq!(browser.get("/runs").await.status, 200);
}

/// `docs/specs/shell.md`, The account; `docs/specs/access.md`, Signing in,
/// Signed in, An open screen and Logged.
#[test_log::test(tokio::test)]
async fn sign_out_in_the_accounts_menu_signs_every_tab_of_the_browser_out() {
    let signing = signing(None).await;
    let cookie = signed_in_as(&signing.pages.sources, Role::Viewer, "Grace Hopper").await;
    let mut browser = Browsing::with_cookie(&signing, &cookie);
    let mut another_tab = Browsing::with_cookie(&signing, &cookie);
    let board = browser.get("/").await;
    let sign_out = select(&board.html, "nav .account button[form]")[0];
    assert_eq!(text(sign_out), "Sign out");
    let form = select(
        &board.html,
        &format!("form#{}", sign_out.attr("form").unwrap()),
    )[0];
    assert_eq!(
        (form.attr("method"), form.attr("action")),
        (Some("post"), Some("/sign-out"))
    );
    let mut cross_site = Browsing::with_cookie(&signing, &cookie);
    let refused = cross_site
        .send(
            axum::http::Request::post("/sign-out").header(CROSS_SITE.0, CROSS_SITE.1),
            String::new(),
        )
        .await;
    assert_eq!(refused.status, 403);
    assert_eq!(another_tab.get("/runs").await.status, 200);
    let (log, _guard) = Log::capture();

    let out = browser.post("/sign-out", &[]).await;

    assert_eq!(out.status, 303);
    assert_eq!(out.location(), "/sign-in?reason=signed-out");
    let sign_in = browser.get(out.location()).await;
    assert_eq!(
        message(&sign_in.html),
        Some(("You signed out.".into(), false))
    );
    assert_eq!(
        in_place_of_the_screen(&another_tab.get("/runs").await),
        (Some(("You signed out.".into(), false)), "/runs".into())
    );
    assert_eq!(
        in_place_of_the_screen(&browser.get("/").await),
        (Some(("You signed out.".into(), false)), "/".into())
    );
    assert_eq!(browser.post("/sign-out", &[]).await.status, 303);
    let logged = log.lines("INFO");
    assert!(
        logged.iter().any(|line| line.contains("signed out")
            && line.contains(r#"identity="google:grace@example.com""#)),
        "{logged:?}"
    );
    let back = browser.sign_in("/runs", Some("grace@example.com")).await;
    assert_eq!(back.location(), "/runs");
    assert_eq!(browser.get("/runs").await.status, 200);
    assert_eq!(
        in_place_of_the_screen(&another_tab.get("/runs").await).0,
        Some(("You signed out.".into(), false)),
        "a tab that still sends the cookie that signed out"
    );
}

/// `docs/specs/access.md`, People, Disabling; Signing in, An open screen;
/// Sign in, The message.
#[test_log::test(tokio::test)]
async fn a_screen_of_a_disabled_person_shows_sign_in_saying_so() {
    let signing = signing(None).await;
    let cookie = signed_in_as(&signing.pages.sources, Role::Viewer, "Grace Hopper").await;
    let mut browser = Browsing::with_cookie(&signing, &cookie);
    let people = People::new(signing.pages.sources.runs.database().clone());
    let grace = people
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|person| person.shown_as == "grace@example.com")
        .unwrap();

    assert_eq!(browser.get("/runs").await.status, 200);
    people
        .disable(grace.id, None, signing.pages.clock.now())
        .await
        .unwrap();
    let disabled = browser.get("/runs").await;
    let stream = streamed(&signing, "/events/runs", &cookie).await;
    people.enable(grace.id).await.unwrap();
    let enabled = browser.get("/runs").await;

    assert_eq!(
        in_place_of_the_screen(&disabled),
        (
            Some(("grace@example.com is disabled.".into(), true)),
            "/runs".into()
        )
    );
    assert_eq!(
        (enabled.status, enabled.location()),
        (303, "/sign-in?return=%2Fruns"),
        "a sign-in that disabling ended, once enabled again"
    );
    assert_eq!(stream, "event: signed-out\n\n");
}

/// `docs/specs/access.md`, Roles, At each request; Signing in, An open
/// screen.
#[test_log::test(tokio::test)]
async fn an_event_stream_says_its_reader_signed_out_and_ends() {
    let signing = signing(None).await;
    let cookie = signed_in_as(&signing.pages.sources, Role::Viewer, "Grace Hopper").await;
    let mut browser = Browsing::with_cookie(&signing, &cookie);
    let stream = |path: &str| {
        let request = axum::http::Request::get(path)
            .header("Host", UI_HOST)
            .header("Cookie", &cookie)
            .body(Body::empty())
            .unwrap();
        signing.router.clone().oneshot(request)
    };
    let Ok(runs) = stream("/events/runs").await;
    let Ok(run) = stream("/events/runs/1").await;
    assert_eq!((runs.status().as_u16(), run.status().as_u16()), (200, 200));

    browser.post("/sign-out", &[]).await;

    for response in [runs, run] {
        let sent = sent_until_it_ends(response).await;
        assert!(
            sent.ends_with("\n\nevent: signed-out\n\n") || sent == "event: signed-out\n\n",
            "{sent:?}"
        );
    }
    assert_eq!(
        streamed(&signing, "/events/runs", &cookie).await,
        "event: signed-out\n\n",
        "a stream asked for once signed out"
    );
    let mut no_one = Browsing::new(&signing);
    assert_eq!(no_one.post("/events/runs", &[]).await.status, 401);
}

/// `docs/specs/access.md`, Roles, At each request: a role changed, or a
/// person disabled, between two requests of one sign-in.
#[test_log::test(tokio::test)]
async fn each_request_is_taken_as_the_persons_record_has_them_then() {
    let pages = Pages::of(a_ticket()).await;
    let viewer = pages.as_viewer().await;
    let back_door = back_door(&work_dir(&pages.dir)).await;
    let set = |change: &'static str| {
        let back_door = back_door.clone();
        async move {
            sqlx::query(change).execute(&back_door).await.unwrap();
        }
    };
    let buttons = |html: &Html| {
        select(html, "#buttons button")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>()
    };

    let before = viewer.page("/benchmarks").await;
    set("UPDATE people SET role = 'contributor' WHERE shown_as = 'vera@example.com'").await;
    let contributing = viewer.page("/benchmarks").await;
    set("UPDATE people SET state = 'disabled' WHERE shown_as = 'vera@example.com'").await;
    let disabled = viewer.page("/benchmarks").await;

    assert_eq!(buttons(&before), Vec::<String>::new());
    assert_eq!(buttons(&contributing), ["New"]);
    assert_eq!(
        message(&disabled),
        Some(("vera@example.com is disabled.".into(), true))
    );
}

/// `docs/specs/access.md`, Actions: what the reader's role does not allow,
/// and an access level that is too low.
#[test_log::test(tokio::test)]
async fn a_viewer_is_shown_no_button_of_an_action_it_may_not_do_and_is_refused_it() {
    let pages = Pages::of(a_ticket()).await;
    let viewer = pages.as_viewer().await;
    let contributor = pages.as_contributor().await;
    let new = |html: &Html| {
        select(html, "#buttons button")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>()
    };

    assert_eq!(new(&viewer.page("/benchmarks").await), Vec::<String>::new());
    assert_eq!(new(&contributor.page("/benchmarks").await), ["New"]);
    let (log, _guard) = Log::capture();
    let (status, answer) = post(
        &viewer,
        "/benchmarks",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;

    assert_eq!(status, 403, "{}", answer.html());
    assert!(
        answer.html().contains("This needs the act access level."),
        "{}",
        answer.html()
    );
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("Make a benchmark: refused")
                && line.contains(r#"identity="google:vera@example.com""#)
                && line.contains("This needs the act access level.")),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, Actions, Logged.
#[test_log::test(tokio::test)]
async fn an_action_that_changes_something_logs_the_person() {
    let pages = Pages::of(a_ticket()).await;
    let contributor = pages.as_contributor().await;
    let (log, _guard) = Log::capture();

    let (status, answer) = post(
        &contributor,
        "/benchmarks",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;

    assert_eq!(status, 200, "{}", answer.html());
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("Make a benchmark: done")
                && line.contains(r#"identity="google:carl@example.com""#)
                && line.contains(r#"action="create_benchmark""#)),
        "{logged:?}"
    );
}
