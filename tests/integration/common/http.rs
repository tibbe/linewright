/// The answer of `router`, in-process, to a request for `path` with
/// `host` as its `Host`, or none.
pub async fn routed(
    router: &axum::Router,
    method: hyper::Method,
    host: Option<&str>,
    path: &str,
) -> axum::response::Response {
    use tower::ServiceExt as _;
    let mut request = axum::http::Request::builder().method(method).uri(path);
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
/// none, and gives the answer as soon as its head arrives, as an event
/// stream's body does not end.
pub async fn send_over(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    method: hyper::Method,
    host: Option<&str>,
    path: &str,
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
