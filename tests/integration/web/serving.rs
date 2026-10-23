use super::*;

#[tokio::test]
async fn pages_run_only_linewrights_own_scripts_and_assets_are_kept_for_good() {
    let web = Web::start(with_ticket().await).await;

    let page = web.response("/items/1").await;
    let html = web.page("/items/1").await;
    let scripts: Vec<String> = select(&html, "script[src]")
        .into_iter()
        .map(|script| script.attr("src").unwrap().to_string())
        .collect();
    let stale = web.response("/assets/htmx-0000000000000000.min.js").await;

    assert_eq!(
        page.headers()["content-security-policy"],
        "default-src 'self'; img-src 'self' https://github.com https://avatars.githubusercontent.com"
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
        let asset = web.response(script).await;
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
    web.stop().await;
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
async fn a_page_its_stream_and_an_asset(web: &Web) -> [String; 3] {
    let html = web.page("/items/1").await;
    let script = select(&html, "script[src]")[0]
        .attr("src")
        .unwrap()
        .to_string();
    ["/items/1".into(), "/events/items/1".into(), script]
}

#[tokio::test]
async fn the_ui_answers_a_local_name_an_ip_address_or_one_of_ui_hosts() {
    let web = Web::start(with_ticket().await).await;
    let ui = web.listen(&["Linewright.Example.com"]).await;

    for path in a_page_its_stream_and_an_asset(&web).await {
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
    web.stop().await;
}

#[tokio::test]
async fn the_ui_refuses_any_other_host_or_none_as_misdirected_and_logs_it() {
    let web = Web::start(with_ticket().await).await;
    let ui = web.listen(&["linewright.example.com"]).await;
    let paths = a_page_its_stream_and_an_asset(&web).await;

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
    web.stop().await;
}
