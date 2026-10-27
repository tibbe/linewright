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
        assert_eq!(icons(repo[0]), 0, "{path}");
        assert_eq!(select_in(repo[0], "img").len(), 1, "{path}");
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

#[tokio::test]
async fn every_page_has_one_tooltip_outside_every_region_for_its_script_to_show() {
    let web = Web::start(with_ticket().await).await;

    for path in [
        "/",
        "/items/1",
        "/runs",
        "/runs/1/triage-1",
        "/automations",
        "/server",
        "/items/99",
    ] {
        let (_, body) = web.get(path).await;
        let html = Html::parse_document(&body);
        let tips = select(&html, "[popover], .tooltip");
        assert_eq!(tips.len(), 1, "{path}: {tips:?}");
        let tip = tips[0];
        assert_eq!(tip.attr("id"), Some("tip"), "{path}");
        assert_eq!(tip.attr("popover"), Some("manual"), "{path}");
        assert_eq!(tip.attr("role"), Some("tooltip"), "{path}");
        assert_eq!(tip.attr("data-tip"), Some(""), "{path}");
        assert!(
            has_classes(tip, &["tooltip", "tooltip-open"]),
            "{path}: {}",
            tip.html()
        );
        // No region holds it, so no update morphs it away.
        let parent = ElementRef::wrap(tip.parent().unwrap()).unwrap();
        assert_eq!(parent.value().name(), "body", "{path}");
    }
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

/// Whether `element`, or one of its ancestors below `within`, is hidden at
/// some width.
fn hidden_at_some_width(element: ElementRef<'_>, within: ElementRef<'_>) -> bool {
    std::iter::successors(Some(element), |element| {
        element.parent().and_then(ElementRef::wrap)
    })
    .take_while(|element| element.id() != within.id())
    .any(|element| {
        element
            .value()
            .classes()
            .any(|class| class == "hidden" || class.ends_with(":hidden"))
    })
}

#[tokio::test]
async fn the_repository_is_its_owners_avatar_beside_its_name_and_in_the_rail_the_avatar_with_its_name_as_a_tip_while_cut_and_hidden_text()
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
        !hidden_at_some_width(avatar, repo),
        "every width shows the avatar: {}",
        repo.html()
    );
    assert_eq!(select_in(repo, "svg").len(), 0, "no icon: {}", repo.html());
    let names = select_in(repo, "span");
    assert_eq!(names.len(), 1, "{}", repo.html());
    let name = names[0];
    assert_eq!(text(name), "owner/widgets");
    assert!(
        in_the_rail(name).contains(&"sr-only") && !has_classes(name, &["sr-only"]),
        "only the rail hides the name: {}",
        name.html()
    );
    // The page's script shows the tip while the name is cut, which the
    // rail's `sr-only` does and the sidebar does not.
    assert_eq!(repo.attr("data-tip"), Some("owner/widgets"));
    assert_eq!(repo.attr("data-tip-cut"), Some(""));
    assert_eq!(
        select(&html, ".drawer-side [class*='tooltip']").len(),
        0,
        "the page's one tooltip is the repository's: {}",
        repo.html()
    );
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
    let server = web.page("/server").await;
    let facts = server_facts(&server);
    assert_eq!(
        facts.get(2),
        Some(&ServerFact {
            label: "Usage limit".into(),
            value: "in force until 2026-10-13 07:50:00 UTC".into(),
            tone: Some("neutral".into()),
        }),
        "{facts:?}"
    );
    // `docs/specs/design_system.md`, Facts and Type: the facts are labels
    // in Muted text beside their values, drawn as a card is, and a hold's
    // status is its icon and words in its colour, with no badge behind it.
    assert_eq!(
        select(&server, "#server dl.card").len(),
        1,
        "{}",
        server.html()
    );
    assert_in_muted_text(&server, "#server dt");
    let usage_limit = select(&server, "#server dd")[2];
    assert_eq!(
        select_in(usage_limit, ".status-text svg.icon").len(),
        1,
        "{}",
        usage_limit.html()
    );
    assert!(
        select(&server, "#server dl.card .badge").is_empty(),
        "{}",
        server.html()
    );
    assert_no_status_by_colour_alone(&server);
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
        has_classes(select(&html, "#holds .alert")[0], &["alert-info"]),
        "{}",
        html.html()
    );
    let facts = server_facts(&web.page("/server").await);
    let drain = &facts[2];
    assert!(
        drain.label == "Drain"
            && drain.value.starts_with("started 2026-10-01 ")
            && !drain.value.contains("deadline")
            && drain.tone.as_deref() == Some("info"),
        "{facts:?}"
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
    assert!(
        has_classes(select(&html, "#holds .alert")[0], &["alert-warning"]),
        "{}",
        html.html()
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

/// The holds region of the next event `events` sends that holds it.
async fn next_holds(events: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Html {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("an event in time")
            .unwrap();
        if let Some((_, holds)) = regions(&event).into_iter().find(|(id, _)| id == "holds") {
            return holds;
        }
    }
}

#[tokio::test]
async fn a_failed_poll_is_a_problem_on_every_page_until_a_poll_succeeds() {
    let world = with_ticket().await;
    world.store.fail_reads("GitHub is down");
    let web = Web::polling(world, Duration::from_millis(200)).await;
    let mut events = stream(&web, "/events/").await;

    let mut shown = next_holds(&mut events).await;
    while select(&shown, ".alert").is_empty() {
        shown = next_holds(&mut events).await;
    }

    let alert = select(&shown, ".alert")[0];
    assert!(has_classes(alert, &["alert-error"]), "{}", alert.html());
    // Pages that need a poll that succeeded say so, under the same frame.
    for path in ["/", "/runs", "/automations", "/server", "/items/1"] {
        let (_, body) = web.get(path).await;
        let held = holds(&Html::parse_document(&body));
        assert!(
            held.len() == 1 && held[0].0.starts_with("polls failed: ") && held[0].1 == "/server",
            "{path}: {held:?}"
        );
    }

    let facts = server_facts(&web.page("/server").await);
    let failed = facts
        .iter()
        .find(|fact| fact.label == "Failed polls")
        .unwrap_or_else(|| panic!("{facts:?}"));
    assert_eq!(failed.tone.as_deref(), Some("error"), "{facts:?}");
    // The count, when the first and the last failed, and the last's error.
    let shape = |value: &str| match value.split_once(", the first 2026-10-01 ") {
        Some((count, rest)) => {
            count.parse::<u64>().is_ok_and(|count| count > 1) && rest.contains(", the last ")
        }
        None => value.starts_with("1, 2026-10-01 "),
    };
    assert!(
        shape(&failed.value)
            && failed
                .value
                .ends_with(" UTC: taking a snapshot: GitHub is down"),
        "{failed:?}"
    );
    // Before any poll has succeeded, the polls stand as none yet.
    assert_eq!(facts[1].label, "Polls", "{facts:?}");
    assert!(facts[1].value.starts_with("none yet, next "), "{facts:?}");

    web.store.restore_reads();

    while !select(&shown, ".alert").is_empty() {
        shown = next_holds(&mut events).await;
    }
    assert_eq!(holds(&web.page("/").await), Vec::<(String, String)>::new());
    web.stop().await;
}

#[tokio::test]
async fn a_file_of_the_repositorys_linewright_cannot_use_is_a_problem_linking_to_the_server_page_until_the_default_branch_fixes_it()
 {
    let world = with_ticket().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/config.toml", "max-runs = 9\n")],
            "Configure Linewright",
        )
        .await
        .unwrap();
    let web = Web::polling(world, Duration::from_millis(200)).await;
    let error = "the setting `max-runs` is the operator's: the repository's file holds \
                 `model`, `effort`, `session-timeout`";

    let html = web
        .eventually_page("/", |html| !holds(html).is_empty())
        .await;

    assert_eq!(
        holds(&html),
        [(
            format!("repository file Linewright cannot use: `.linewright/config.toml`: {error}"),
            "/server".to_string(),
        )]
    );
    assert!(
        has_classes(select(&html, "#holds .alert")[0], &["alert-error"]),
        "{}",
        html.html()
    );
    let facts = server_facts(&web.page("/server").await);
    assert_eq!(
        facts.get(2),
        Some(&ServerFact {
            label: "Repository file".into(),
            value: format!("`.linewright/config.toml`: {error}"),
            tone: Some("error".into()),
        }),
        "{facts:?}"
    );

    web.store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/config.toml", "model = \"opus\"\n")],
            "Fix Linewright's config",
        )
        .await
        .unwrap();

    let fixed = web
        .eventually_page("/server", |page| holds(page).is_empty())
        .await;
    let fixed_facts = server_facts(&fixed);
    assert!(
        !fixed_facts
            .iter()
            .any(|fact| fact.label == "Repository file"),
        "{fixed_facts:?}"
    );
    web.stop().await;
}

/// A fact of the server page, as a reader reads it, with the colour of its
/// value's status, such as `error`, if it shows one.
#[derive(Debug, PartialEq, Eq)]
struct ServerFact {
    label: String,
    value: String,
    tone: Option<String>,
}

/// The server page's facts, in order.
fn server_facts(html: &Html) -> Vec<ServerFact> {
    let labels = select(html, "#server dl > dt");
    let values = select(html, "#server dl > dd");
    assert_eq!(labels.len(), values.len(), "{}", html.html());
    labels
        .into_iter()
        .zip(values)
        .map(|(label, value)| ServerFact {
            label: text(label),
            value: text(value),
            tone: select_in(value, ".status-text").first().and_then(|status| {
                status
                    .value()
                    .classes()
                    .find_map(|class| {
                        class
                            .strip_prefix("[--tone:var(--color-")
                            .and_then(|colour| colour.strip_suffix(")]"))
                    })
                    .map(str::to_string)
            }),
        })
        .collect()
}

/// The server page's facts, as label and value.
fn labelled(facts: &[ServerFact]) -> Vec<(&str, &str)> {
    facts
        .iter()
        .map(|fact| (fact.label.as_str(), fact.value.as_str()))
        .collect()
}

#[tokio::test]
async fn the_server_page_shows_the_facts_about_linewright_each_under_its_label() {
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
    let labels: Vec<&str> = facts.iter().map(|fact| fact.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Started",
            "Polls",
            "Runs in flight",
            "Last webhook delivery accepted",
            "Webhook deliveries refused",
        ],
        "{facts:?}"
    );
    assert!(facts[0].value.starts_with("2026-10-01 "), "{:?}", facts[0]);
    let polls = facts[1]
        .value
        .strip_prefix("last 2026-10-01 ")
        .and_then(|rest| rest.split_once(" UTC, next 2026-10-01 "));
    assert!(polls.is_some(), "{:?}", facts[1]);
    assert_eq!(labelled(&facts[2..3]), [("Runs in flight", "0 of 4")]);
    assert!(facts[3].value.starts_with("2026-10-01 "), "{:?}", facts[3]);
    assert_eq!(labelled(&facts[4..]), [("Webhook deliveries refused", "0")]);
    assert!(facts.iter().all(|fact| fact.tone.is_none()), "{facts:?}");
    // `docs/specs/web/design_system.md`, Facts: drawn as a card is.
    let cards = classes(&html, "#server dl.card");
    assert!(
        cards.len() == 1
            && [
                "card-sm",
                "border",
                "border-base-300",
                "bg-card",
                "shadow-sm"
            ]
            .iter()
            .all(|class| cards[0].contains(class)),
        "{cards:?}"
    );
    assert!(select(&html, "#server table").is_empty(), "{}", html.html());
    // The times are `<time>` elements, which the page's script shows past
    // ones relative and future ones absolute.
    assert_eq!(select(&html, "#server time[datetime]").len(), 4);
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
            .is_some_and(|facts| labelled(facts).contains(&("Webhook deliveries refused", "1"))),
        "{server:?}"
    );
    web.stop().await;
}

#[tokio::test]
async fn every_pages_heading_takes_its_look_from_the_stylesheet_alone() {
    let (web, _) = super::run::run_page_of_implemented().await;

    for path in [
        "/items/1",
        "/runs",
        "/runs/1/implement-1",
        "/automations",
        "/server",
    ] {
        let html = web.page(path).await;
        let headings = select(&html, "h1");
        assert_eq!(headings.len(), 1, "{path}");
        assert_eq!(headings[0].attr("class"), None, "{path}");
    }
    web.stop().await;
}
