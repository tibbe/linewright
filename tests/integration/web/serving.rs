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
