use super::*;

fn frame_facts(html: &Html) -> Vec<String> {
    select(html, "#bar .facts li")
        .into_iter()
        .map(text)
        .collect()
}

#[tokio::test]
async fn the_frame_names_the_repository_and_shows_the_facts_as_status_does() {
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

    let html = web.page("/items/1").await;

    let facts = frame_facts(&html);
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
            "automation weekly last run",
            "automation weekly next run",
        ],
        "{facts:?}"
    );
    assert_eq!(facts[3], "runs in flight: 0 of 4");
    assert!(
        facts[1]
            .strip_prefix("last poll: 2026-10-01 12:00:")
            .is_some_and(|rest| rest.ends_with(" UTC")),
        "{facts:?}"
    );
    let repo = select(&html, "#bar a.repo");
    assert_eq!(
        repo[0].attr("href"),
        Some("https://github.com/owner/widgets")
    );
    assert_eq!(text(repo[0]), "owner/widgets");
    let links: Vec<_> = select(&html, "#bar nav a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert!(
        links.contains(&"/") && links.contains(&"/runs"),
        "{links:?}"
    );
    assert_eq!(banners(&html), Vec::<String>::new());
    web.stop().await;
}

#[tokio::test]
async fn the_frame_shows_a_usage_limit_in_force_as_a_banner() {
    let world = with_issue_for_triage().await;
    world
        .harness
        .on(WorkKind::Triage, |_| refused_for_the_usage_limit());
    let web = Web::start(world).await;

    let html = eventually_shown_frame(&web, |banners| !banners.is_empty()).await;

    assert_eq!(
        banners(&html),
        ["usage limit: in force until 2026-10-13 07:50:00 UTC"]
    );
    let facts = frame_facts(&html);
    assert!(
        !facts.iter().any(|fact| fact.starts_with("usage limit")),
        "{facts:?}"
    );
    web.stop().await;
}

#[tokio::test]
async fn the_frame_shows_a_drain_under_way_as_a_banner() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    eventually_shown(&web, "/items/1", |lines| {
        lines.iter().any(|line| line.starts_with("run in flight: "))
    })
    .await;

    web.serving.drain();

    let html = eventually_shown_frame(&web, |banners| !banners.is_empty()).await;
    let banners = banners(&html);
    assert!(
        banners.len() == 1 && banners[0].starts_with("drain: under way since 2026-10-01 "),
        "{banners:?}"
    );
    drop(agent);
    web.stop().await;
}

fn banners(html: &Html) -> Vec<String> {
    select(html, "#bar .banner").into_iter().map(text).collect()
}

async fn eventually_shown_frame(web: &Web, shown: impl Fn(&[String]) -> bool) -> Html {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let html = web.page("/items/1").await;
            if shown(&banners(&html)) {
                return html;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    waited.expect("shown in time")
}
