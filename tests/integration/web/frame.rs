use super::*;

/// Each hold's words, and where it links.
fn holds(html: &Html) -> Vec<(String, String)> {
    select(html, "#holds .alert")
        .into_iter()
        .map(|alert| {
            let href = select_in(alert, "a")
                .last()
                .and_then(|a| a.attr("href"))
                .unwrap_or_default()
                .to_string();
            (text(select_in(alert, ".phrase")[0]), href)
        })
        .collect()
}

/// How many icons `element` holds that a screen reader skips, so that its
/// text alone is its label.
fn icons(element: ElementRef) -> usize {
    select_in(element, r#"svg[aria-hidden="true"]"#).len()
}

#[tokio::test]
async fn every_page_has_the_same_sidebar_naming_the_repository_and_the_pages_with_their_icons() {
    let web = Web::start(with_ticket().await).await;

    let mut sidebars = vec![];
    for path in [
        "/",
        "/items/1",
        "/runs",
        "/automations",
        "/server",
        "/items/99",
    ] {
        let (_, body) = web.get(path).await;
        let html = Html::parse_document(&body);
        let sidebar = select(&html, ".drawer-side nav");
        assert_eq!(sidebar.len(), 1, "{path}");
        let repo = select(&html, ".drawer-side a.repo");
        assert_eq!(
            repo[0].attr("href"),
            Some("https://github.com/owner/widgets")
        );
        assert_eq!(text(repo[0]), "owner/widgets");
        assert_eq!(icons(repo[0]), 1, "{path}");
        let links: Vec<(String, String, usize)> = select(&html, ".drawer-side .menu a")
            .into_iter()
            .map(|a| (text(a), a.attr("href").unwrap().to_string(), icons(a)))
            .collect();
        assert_eq!(
            links,
            [
                ("Board".to_string(), "/".to_string(), 1),
                ("Runs".to_string(), "/runs".to_string(), 1),
                ("Automations".to_string(), "/automations".to_string(), 1),
                ("Server".to_string(), "/server".to_string(), 1),
            ],
            "{path}"
        );
        assert_eq!(select(&html, "#status-line").len(), 0, "{path}");
        assert_eq!(holds(&html), Vec::<(String, String)>::new(), "{path}");
        sidebars.push(select(&html, ".drawer-side nav")[0].html());
    }
    assert!(
        sidebars.windows(2).all(|pair| pair[0] == pair[1]),
        "{sidebars:?}"
    );
    web.stop().await;
}

/// `element`'s classes in the rail, without their `lg:max-2xl:` prefix.
fn in_the_rail(element: ElementRef<'_>) -> Vec<&str> {
    element
        .value()
        .classes()
        .filter_map(|class| class.strip_prefix("lg:max-2xl:"))
        .collect()
}

#[tokio::test]
async fn in_the_rail_the_repository_is_its_owners_avatar_with_its_name_as_a_tooltip_and_hidden_text()
 {
    let web = Web::start(with_ticket().await).await;

    let html = web.page("/runs").await;

    let repo = select(&html, ".drawer-side a.repo")[0];
    let avatars = select_in(repo, "img");
    assert_eq!(avatars.len(), 1, "{}", repo.html());
    let avatar = avatars[0];
    assert!(
        avatar
            .attr("src")
            .is_some_and(|src| src.starts_with("https://github.com/owner.png")),
        "{}",
        avatar.html()
    );
    assert_eq!(avatar.attr("alt"), Some(""), "the avatar is decorative");
    assert!(
        avatar
            .value()
            .has_class("hidden", scraper::CaseSensitivity::CaseSensitive)
            && in_the_rail(avatar).contains(&"block"),
        "only the rail shows the avatar: {}",
        avatar.html()
    );
    let icon = select_in(repo, "svg")[0];
    assert!(in_the_rail(icon).contains(&"hidden"), "{}", icon.html());
    let names = select_in(repo, "span");
    assert_eq!(names.len(), 1, "{}", repo.html());
    assert_eq!(text(names[0]), "owner/widgets");
    assert!(
        in_the_rail(names[0]).contains(&"sr-only"),
        "{}",
        names[0].html()
    );
    let tooltip = select(&html, ".drawer-side :has(> a.repo)")[0];
    assert!(
        in_the_rail(tooltip).contains(&"tooltip"),
        "{}",
        tooltip.html()
    );
    assert_eq!(tooltip.attr("data-tip"), Some("owner/widgets"));
    web.stop().await;
}

#[tokio::test]
async fn a_usage_limit_in_force_is_a_hold_on_every_page_linking_to_the_server_page() {
    let world = with_issue_for_triage().await;
    world
        .harness
        .on(WorkKind::Triage, |_| refused_for_the_usage_limit());
    let web = Web::start(world).await;

    let html = web
        .eventually_page("/items/1", |html| !holds(html).is_empty())
        .await;

    let held = [(
        "usage limit: in force until 2026-10-13 07:50:00 UTC".to_string(),
        "/server".to_string(),
    )];
    assert_eq!(holds(&html), held);
    let alert = select(&html, "#holds .alert")[0];
    assert!(
        !alert
            .value()
            .classes()
            .any(|class| class.starts_with("alert-")),
        "a usage limit shows neutral: {}",
        alert.html()
    );
    for path in ["/", "/runs", "/server"] {
        assert_eq!(holds(&web.page(path).await), held, "{path}");
    }
    web.stop().await;
}

#[tokio::test]
async fn a_drain_under_way_is_a_hold_linking_to_the_server_page() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    web.eventually_page("/items/1", |html| {
        has_run(&runs_shown(html), "in flight", "1/triage-1")
    })
    .await;

    web.serving.drain();

    let html = web
        .eventually_page("/", |html| !holds(html).is_empty())
        .await;
    let holds = holds(&html);
    assert!(
        holds.len() == 1
            && holds[0].0.starts_with("drain: under way since 2026-10-01 ")
            && holds[0].1 == "/server",
        "{holds:?}"
    );
    assert!(
        select(&html, "#holds .alert")[0]
            .value()
            .has_class("alert-warning", scraper::CaseSensitivity::CaseSensitive),
        "{}",
        html.html()
    );
    drop(agent);
    web.stop().await;
}

#[tokio::test]
async fn a_refused_delivery_is_a_hold_until_a_delivery_is_accepted() {
    let web = Web::start(with_ticket().await).await;
    let unsigned = Request {
        signature: None,
        ..Request::signed_json("{}")
    };

    unsigned.send(&web.listener).await;

    let html = web.page("/runs").await;
    let refused = holds(&html);
    assert!(
        refused.len() == 1
            && refused[0]
                .0
                .strip_prefix("webhook delivery refused: 2026-10-01 12:")
                .is_some_and(|rest| rest.ends_with(" UTC, none accepted since"))
            && refused[0].1 == "/server",
        "{refused:?}"
    );

    signed(&web.listener, "ping", "{}").await;

    assert_eq!(
        holds(&web.page("/runs").await),
        Vec::<(String, String)>::new()
    );

    unsigned.send(&web.listener).await;

    assert_eq!(holds(&web.page("/runs").await).len(), 1);
    web.stop().await;
}

#[tokio::test]
async fn a_hold_comes_and_goes_through_the_stream() {
    let web = Web::start(with_ticket().await).await;
    let mut events = stream(&web, "/events/runs").await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids[..1], ["holds"]);

    Request {
        signature: None,
        ..Request::signed_json("{}")
    }
    .send(&web.listener)
    .await;

    let changed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let changed = regions(&changed);
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].0, "holds");
    assert_eq!(select(&changed[0].1, ".alert").len(), 1);
    web.stop().await;
}

/// The server page's facts, as a reader reads them.
fn server_facts(html: &Html) -> Vec<String> {
    select(html, "#server li").into_iter().map(text).collect()
}

#[tokio::test]
async fn the_server_page_shows_the_facts_about_linewright_as_status_does() {
    let world = with_ticket().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/automations/weekly.md",
                "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    let web = Web::start(world).await;
    signed(&web.listener, "ping", "{}").await;

    let html = web.page("/server").await;

    let facts = server_facts(&html);
    let labels: Vec<&str> = facts
        .iter()
        .map(|fact| {
            fact.split_once(": ")
                .map_or(fact.as_str(), |(label, _)| label)
        })
        .collect();
    assert_eq!(
        labels,
        [
            "serving since",
            "last poll",
            "next poll",
            "runs in flight",
            "last webhook delivery accepted",
            "webhook deliveries refused",
        ],
        "{facts:?}"
    );
    assert_eq!(facts[3], "runs in flight: 0 of 4");
    assert_eq!(facts[5], "webhook deliveries refused: 0");
    assert!(
        !html.html().contains("automation weekly"),
        "the automations' facts are not shown: {}",
        html.html()
    );
    assert_eq!(events_path(&html), Some("/events/server"));
    web.stop().await;
}

#[tokio::test]
async fn the_server_pages_stream_sends_the_frame_and_the_facts_then_what_changed() {
    let web = Web::start(with_ticket().await).await;
    let mut events = stream(&web, "/events/server").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["holds", "server"]);

    Request {
        signature: None,
        ..Request::signed_json("{}")
    }
    .send(&web.listener)
    .await;

    let changed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let changed = regions(&changed);
    let server = changed
        .iter()
        .find(|(id, _)| id == "server")
        .map(|(_, region)| server_facts(region));
    assert!(
        server
            .as_ref()
            .is_some_and(|facts| facts.contains(&"webhook deliveries refused: 1".to_string())),
        "{server:?}"
    );
    web.stop().await;
}
