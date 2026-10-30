use super::*;

/// The pages of #1, ready for Linewright.
async fn pages() -> Pages {
    Pages::of(a_ticket()).await
}

#[tokio::test]
async fn pages_run_only_linewrights_own_scripts_and_assets_are_kept_for_good() {
    let pages = pages().await;

    let page = pages.response("/items/1").await;
    let html = pages.page("/items/1").await;
    let scripts: Vec<String> = select(&html, "script[src]")
        .into_iter()
        .map(|script| script.attr("src").unwrap().to_string())
        .collect();
    let stale = pages.response("/assets/htmx-0000000000000000.min.js").await;

    assert_eq!(
        page.headers()["content-security-policy"],
        "default-src 'self'; img-src 'self' data: https://github.com https://avatars.githubusercontent.com"
    );
    assert_eq!(select(&html, "script:not([src])").len(), 0);
    let named: Vec<(&str, &str)> = scripts
        .iter()
        .map(|script| {
            let name = script.strip_prefix("/assets/").unwrap();
            let (hashed, extension) = name.split_once('.').unwrap();
            let (stem, _hash) = hashed.rsplit_once('-').unwrap();
            (stem, extension)
        })
        .collect();
    assert_eq!(
        named,
        [("htmx", "min.js"), ("hx-sse", "js"), ("linewright", "js")]
    );
    for script in &scripts {
        let asset = pages.response(script).await;
        assert_eq!(asset.status(), 200, "{script}");
        assert_eq!(
            asset.headers()["content-type"],
            "text/javascript; charset=utf-8"
        );
        assert!(
            asset.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
    }
    assert_eq!(stale.status(), 404);
}

/// The status of the answer to a GET of `path` on `listener` with `host`
/// as its `Host`, or none.
async fn status_on(listener: &web::Listener, host: Option<&str>, path: &str) -> u16 {
    let stream = tokio::net::TcpStream::connect(listener.local_address())
        .await
        .unwrap();
    send_over(stream, hyper::Method::GET, host, path)
        .await
        .status()
        .as_u16()
}

/// A page, its event stream and an asset of it.
async fn a_page_its_stream_and_an_asset(pages: &Pages) -> [String; 3] {
    let html = pages.page("/items/1").await;
    let script = select(&html, "script[src]")[0]
        .attr("src")
        .unwrap()
        .to_string();
    ["/items/1".into(), "/events/items/1".into(), script]
}

#[tokio::test]
async fn the_ui_answers_a_local_name_an_ip_address_or_one_of_ui_hosts() {
    let pages = pages().await;
    let ui = pages.listen(&["Linewright.Example.com"]).await;

    for path in a_page_its_stream_and_an_asset(&pages).await {
        for host in [
            "localhost",
            "LOCALHOST.:8081",
            "127.0.0.1",
            "[::1]:8081",
            "192.0.2.7",
            "[2001:db8::7]",
            "LINEWRIGHT.example.COM.",
            "linewright.example.com:443",
        ] {
            assert_eq!(
                status_on(&ui, Some(host), &path).await,
                200,
                "{host} {path}"
            );
        }
    }
    ui.stop().await;
}

#[tokio::test]
async fn the_ui_refuses_any_other_host_or_none_as_misdirected_and_logs_it() {
    let pages = pages().await;
    let ui = pages.listen(&["linewright.example.com"]).await;
    let paths = a_page_its_stream_and_an_asset(&pages).await;

    for path in paths {
        for (host, reason) in [
            (Some("evil.example"), "ui-hosts"),
            (Some("localhost.evil.example"), "ui-hosts"),
            (Some("www.linewright.example.com"), "ui-hosts"),
            (None, "no Host"),
        ] {
            let (log, _guard) = Log::capture();

            let status = status_on(&ui, host, &path).await;

            assert_eq!(status, 421, "{host:?} {path}");
            let refused = log.lines("WARN");
            assert_eq!(refused.len(), 1, "{host:?} {path}: {refused:?}");
            assert!(
                refused[0].contains(&format!("host={}", host.unwrap_or("none"))),
                "{}",
                refused[0]
            );
            assert!(refused[0].contains(reason), "{}", refused[0]);
        }
    }
    ui.stop().await;
}

#[tokio::test]
async fn a_pages_stream_passes_through_a_proxy_unbuffered_unchanged_and_uncached() {
    let pages = pages().await;

    let response = pages.response("/events/items/1").await;

    assert_eq!(response.status(), 200);
    assert_passes_through_a_proxy(response.headers());
}

// The keep-alive is a comment line (WHATWG HTML, 9.2.6), sent once a stream
// has been quiet for 15 seconds (docs/specs/operation.md, Running: Behind a
// proxy).
#[tokio::test]
async fn a_quiet_pages_stream_sends_a_comment_15_seconds_after_its_last_event_and_none_before() {
    let pages = pages().await;
    // Paused once serving, so that time moves on only when nothing else can.
    tokio::time::pause();
    let response = pages.response("/events/items/1").await;
    let mut body = response.into_body();
    let mut next = async || {
        let frame = body.frame().await.unwrap().unwrap();
        String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
    };

    let regions = next().await;
    let sent = tokio::time::Instant::now();
    let first = next().await;
    let first_after = sent.elapsed();
    let second = next().await;
    let second_after = sent.elapsed();

    assert!(regions.starts_with("data: "), "{regions}");
    assert_eq!([first, second], [":\n\n", ":\n\n"]);
    // Tokio's timer rounds each deadline up to the next millisecond.
    let tick = Duration::from_millis(1);
    let quiet = Duration::from_secs(15);
    assert!(
        (quiet..=quiet + tick).contains(&first_after),
        "{first_after:?}"
    );
    assert!(
        (quiet * 2..=quiet * 2 + tick * 2).contains(&second_after),
        "{second_after:?}"
    );
}
