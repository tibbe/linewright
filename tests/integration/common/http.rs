/// The answer of `router`, in-process, to a request for `path` with
/// `host` as its `Host`, or none.
pub async fn routed(
    router: &axum::Router,
    method: hyper::Method,
    host: Option<&str>,
    path: &str,
) -> axum::response::Response {
    routed_with_cookie(router, method, host, path, "").await
}

/// As [`routed`], from a browser holding the cookies `cookie`, or none
/// where it is empty.
pub async fn routed_with_cookie(
    router: &axum::Router,
    method: hyper::Method,
    host: Option<&str>,
    path: &str,
    cookie: &str,
) -> axum::response::Response {
    use tower::ServiceExt as _;
    let mut request = axum::http::Request::builder().method(method).uri(path);
    if !cookie.is_empty() {
        request = request.header("Cookie", cookie);
    }
    if let Some(host) = host {
        request = request.header("Host", host);
    }
    let request = request.body(axum::body::Body::empty()).unwrap();
    router.clone().oneshot(request).await.unwrap()
}

/// The status of `response` and its whole body.
pub async fn status_and_body(response: axum::response::Response) -> (u16, String) {
    use http_body_util::BodyExt as _;
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// Sends a request for `path` over `stream`, with `host` as its `Host`, or
/// none, and `headers` besides, and gives the answer as soon as its head
/// arrives.
pub async fn send_over(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    method: hyper::Method,
    host: Option<&str>,
    path: &str,
    headers: &[(&str, &str)],
) -> hyper::Response<hyper::body::Incoming> {
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .unwrap();
    tokio::spawn(connection);
    let mut request = hyper::Request::builder().method(method).uri(path);
    if let Some(host) = host {
        request = request.header("Host", host);
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(http_body_util::Empty::<axum::body::Bytes>::new())
        .unwrap();
    sender.send_request(request).await.unwrap()
}

/// That `headers` are an event stream's that asks a reverse proxy in front to
/// pass each event on as it is sent, unchanged, and to keep no copy
/// (docs/specs/operation.md, Running: Behind a proxy): `no-store` and
/// `no-transform` (RFC 9111, section 5.2.2), with nothing else that allows a
/// copy, and nginx's `X-Accel-Buffering: no`
/// (<https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_buffering>).
pub fn assert_passes_through_a_proxy(headers: &axum::http::HeaderMap) {
    let all = |name| -> Vec<&str> {
        headers
            .get_all(name)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect()
    };
    assert_eq!(all("content-type"), ["text/event-stream"]);
    assert_eq!(all("cache-control"), ["no-store, no-transform"]);
    assert_eq!(all("x-accel-buffering"), ["no"]);
}

/// The host of [`public_url`].
pub const UI_HOST: &str = "127.0.0.1";

/// The web UI's public URL in tests, `http://UI_HOST:8081`.
pub fn public_url() -> linewright::host::PublicUrl {
    linewright::host::PublicUrl::parse(&format!("http://{UI_HOST}:8081"))
        .expect("a loopback address is a public URL")
}

/// The web UI on `address`, at [`public_url`], signing people in with
/// Google.
#[track_caller]
pub fn ui_on(address: &str) -> linewright::web::Ui {
    use linewright::access::providers::{ClientId, ClientSecret, Google, Providers, SignUpDomains};
    linewright::web::Ui {
        address: address.parse().unwrap(),
        public_url: public_url(),
        providers: Providers::Google(Google {
            client_id: ClientId::new("google-id".into()).unwrap(),
            secret: ClientSecret::new("google-s3cret".into()).unwrap(),
            signup: SignUpDomains::default(),
            endpoints: linewright::access::providers::GoogleEndpoints::default(),
        }),
        manager: None,
    }
}

/// The web UI as tests drive it in-process, at [`public_url`].
pub fn ui() -> linewright::web::Ui {
    ui_on("127.0.0.1:8081")
}

/// A browser signed in by the back door as a new, enabled person of `role`
/// whose name is `name`, signed in with Google as `EMAIL`, the name's first
/// word lower-cased `@example.com`, as a [`FakeClock`] starts, without
/// reading the clock of `sources`: the `Cookie` it sends.
#[track_caller]
pub fn signed_in_as(
    sources: &linewright::api::Sources,
    role: linewright::access::Role,
    name: &str,
) -> impl Future<Output = String> + use<> {
    use linewright::access::people::{Claims, Identity, People};
    use linewright::access::providers::Provider;
    use linewright::access::sessions::{SESSION_COOKIE, Sessions};
    let caller = std::panic::Location::caller();
    let database = sources.runs.database().clone();
    let now = super::at(super::STARTED);
    let first = name.split(' ').next().unwrap().to_lowercase();
    let claims = Claims {
        identity: Identity {
            provider: Provider::Google,
            subject: format!("subject-of-{first}"),
        },
        shown_as: format!("{first}@example.com"),
        name: Some(name.to_string()),
        avatar: None,
        email_verified: true,
    };
    let name = name.to_string();
    async move {
        let person = People::new(database.clone())
            .add(&claims, role, now)
            .await
            .unwrap_or_else(|e| panic!("adding {name} at {caller}: {e:#}"));
        let cookie = Sessions::new(database)
            .start(person.id, now)
            .await
            .unwrap_or_else(|e| panic!("signing {name} in at {caller}: {e:#}"));
        format!("{SESSION_COOKIE}={}", cookie.value())
    }
}
