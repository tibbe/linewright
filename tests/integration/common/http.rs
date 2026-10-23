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
