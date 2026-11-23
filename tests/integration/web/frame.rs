use super::*;

use linewright::prompts::Readers;
use linewright::repo_files::{FileError, Unusable};
use linewright::status::View;

/// Each hold's words, and where it links.
pub(super) fn holds(html: &Html) -> Vec<(String, String)> {
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

#[test_log::test(tokio::test)]
async fn every_page_has_the_same_sidebar_naming_the_repository_and_the_pages_with_their_icons() {
    let pages = Pages::of(a_ticket()).await;

    let mut sidebars = vec![];
    for path in [
        "/",
        "/items/1",
        "/runs",
        "/automations",
        "/benchmarks",
        "/server",
        "/items/99",
    ] {
        let (_, body) = pages.get(path).await;
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
                ("Benchmarks".to_string(), "/benchmarks".to_string(), 1),
                ("Server".to_string(), "/server".to_string(), 1),
                ("People".to_string(), "/people".to_string(), 1),
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
}

#[test_log::test(tokio::test)]
async fn every_page_has_one_tooltip_outside_every_region_for_its_script_to_show() {
    let pages = Pages::of(a_ticket()).await;

    for path in [
        "/",
        "/items/1",
        "/runs",
        "/runs/1/triage-1",
        "/automations",
        "/server",
        "/items/99",
    ] {
        let (_, body) = pages.get(path).await;
        let html = Html::parse_document(&body);
        let tips = select(&html, "[popover], .tooltip[data-tip]");
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
        let parent = ElementRef::wrap(tip.parent().unwrap()).unwrap();
        assert_eq!(
            parent.value().name(),
            "body",
            "{path}: no region holds the tip, so no update morphs it away"
        );
    }
}

/// `element`'s classes in the rail, without their `lg:rail:` prefix.
fn in_the_rail(element: ElementRef<'_>) -> Vec<&str> {
    element
        .value()
        .classes()
        .filter_map(|class| class.strip_prefix("lg:rail:"))
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

/// The name `element` shows in a tooltip to its right on the rail alone
/// (`docs/specs/web/shell.md`, The rail), which a screen reader skips for
/// the name `element` gives itself.
#[track_caller]
fn rail_tip(element: ElementRef<'_>) -> String {
    assert_eq!(
        in_the_rail(element),
        ["tooltip", "tooltip-right"],
        "{}",
        element.html()
    );
    let tips = select_in(element, ":scope > .tooltip-content");
    assert_eq!(tips.len(), 1, "{}", element.html());
    let tip = tips[0];
    assert!(
        has_classes(tip, &["hidden", "lg:rail:block"]),
        "only the rail shows it: {}",
        tip.html()
    );
    assert_eq!(tip.attr("aria-hidden"), Some("true"), "{}", tip.html());
    text(tip)
}

/// `docs/specs/web/shell.md`, The sidebar, The rail: the repository is its
/// owner's avatar beside its name, a tip while the name is cut; the rail
/// shows the avatar alone, with the name in a tooltip to its right.
#[test_log::test(tokio::test)]
async fn the_repository_is_its_owners_avatar_beside_its_name_and_in_the_rail_the_avatar_with_its_name_in_a_tooltip()
 {
    let pages = Pages::of(a_ticket()).await;

    let html = pages.page("/runs").await;

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
    assert_eq!(
        (in_the_rail(name), has_classes(name, &["hidden"])),
        (vec!["hidden"], false),
        "only the rail hides the name: {}",
        name.html()
    );
    assert_eq!(repo.attr("aria-label"), Some("owner/widgets"));
    assert_eq!(repo.attr("data-tip"), Some("owner/widgets"));
    assert_eq!(repo.attr("data-tip-cut"), Some(""));
    let holder = ElementRef::wrap(repo.parent().unwrap()).unwrap();
    assert_eq!(rail_tip(holder), "owner/widgets");
}

/// `docs/specs/web/shell.md`, The rail: `w-14`, each link its icon alone,
/// its name in a daisyUI `tooltip` to its right, which the drawer lets out.
#[test_log::test(tokio::test)]
async fn in_the_rail_each_link_is_its_icon_with_its_name_in_a_tooltip_to_its_right() {
    let pages = Pages::of(a_ticket()).await;

    let html = pages.page("/runs").await;

    let side = select(&html, ".drawer-side")[0];
    assert_eq!(in_the_rail(side), ["overflow-visible"], "{}", side.html());
    let nav = select_in(side, "nav")[0];
    assert!(in_the_rail(nav).contains(&"w-14"), "{}", nav.html());
    let items = select_in(nav, ".menu:not(.dropdown-content) > li");
    assert_eq!(items.len(), 6, "{}", nav.html());
    for item in items {
        let link = select_in(item, "a")[0];
        let names = select_in(link, "span");
        assert_eq!(names.len(), 1, "{}", link.html());
        assert_eq!(in_the_rail(names[0]), ["sr-only"], "{}", link.html());
        assert_eq!(rail_tip(item), text(link), "{}", item.html());
    }
}

/// The foot button of `html`, from `lg` alone: its icon, mirrored on the
/// sidebar alone, and the name in its `tooltip-right` that labels it.
#[track_caller]
fn foot_button(html: &Html) -> (String, String) {
    let buttons = select(html, ".drawer-side nav button[data-navigation-toggle]");
    assert_eq!(buttons.len(), 1, "{}", html.html());
    let button = buttons[0];
    assert!(
        has_classes(button, &["btn", "btn-ghost", "btn-circle", "btn-sm"]),
        "{}",
        button.html()
    );
    assert_eq!(button.attr("type"), Some("button"));
    assert_eq!(button.attr("data-navigation-toggle"), Some("/navigation"));
    let tooltip = ElementRef::wrap(button.parent().unwrap()).unwrap();
    assert!(
        has_classes(tooltip, &["tooltip", "tooltip-right", "hidden", "lg:block"]),
        "only from `lg`: {}",
        tooltip.html()
    );
    let name = select_in(tooltip, ".tooltip-content")[0];
    assert_eq!(button.attr("aria-labelledby"), name.attr("id"));
    let icon = select_in(button, "svg")[0];
    assert_eq!(
        (has_classes(icon, &["-scale-x-100"]), in_the_rail(icon)),
        (true, vec!["scale-x-100"]),
        "mirrored on the sidebar alone: {}",
        icon.html()
    );
    (icon.attr("data-icon").unwrap().to_string(), text(name))
}

/// `docs/specs/web/shell.md`, Expanding and collapsing: the foot button is
/// a small (`docs/specs/web/design_system.md`, Buttons) `btn-ghost`
/// `btn-circle`, from `lg` alone, named in a `tooltip-right`
/// for what it does on the navigation the page is drawn with.
#[test_log::test(tokio::test)]
async fn the_foot_button_collapses_the_sidebar_and_expands_the_rail() {
    let pages = Pages::of(a_ticket()).await;

    for (cookie, name) in [
        ("", "Collapse navigation"),
        ("navigation=rail", "Expand navigation"),
    ] {
        let html = page_with_cookie(&pages, "/runs", cookie).await;

        assert_eq!(
            foot_button(&html),
            ("chevron-double-right".to_string(), name.to_string()),
            "{cookie}"
        );
    }
}

/// Asserts that the one notice in `html` is an alert of the classes
/// `classes`, its status's icon `icon` first and its link a daisyUI `link` at
/// its end: `docs/specs/web/shell.md`, A notice.
#[track_caller]
fn assert_notice(html: &Html, classes: &[&str], icon: &str) {
    let notices = select(html, "#holds > *");
    assert_eq!(notices.len(), 1, "{}", html.html());
    let notice = notices[0];
    assert_eq!(
        notice.value().classes().collect::<Vec<_>>(),
        classes,
        "{}",
        notice.html()
    );
    let parts: Vec<ElementRef> = notice.children().filter_map(ElementRef::wrap).collect();
    assert_eq!(
        parts.first().and_then(|first| first.attr("data-icon")),
        Some(icon),
        "{}",
        notice.html()
    );
    let last = parts.last().expect("a link");
    assert!(
        last.value().name() == "a" && has_classes(*last, &["link"]),
        "{}",
        notice.html()
    );
}

#[test_log::test(tokio::test)]
async fn a_usage_limit_in_force_is_a_hold_on_every_page_linking_to_the_server_page() {
    let pages = Pages::of(View {
        memory: limited_until("2026-10-13T07:50:00Z"),
        ..a_ticket()
    })
    .await;

    let html = pages.page("/items/1").await;

    let held = [(
        "usage limit: in force until 2026-10-13 07:50:00 UTC".to_string(),
        "/server".to_string(),
    )];
    assert_eq!(holds(&html), held);
    assert_notice(&html, &["alert"], "pause-circle");
    for path in ["/", "/runs", "/server"] {
        assert_eq!(holds(&pages.page(path).await), held, "{path}");
    }
    let server = pages.page("/server").await;
    let facts = server_facts(&server);
    assert_eq!(
        facts.get(2),
        Some(&ServerFact {
            label: "Usage limit".into(),
            value: "in force until 2026-10-13 07:50:00 UTC".into(),
            tone: Some("base-content/60".into()),
        }),
        "{facts:?}"
    );
    assert_eq!(
        select(&server, "#server dl.card").len(),
        1,
        "{}",
        server.html()
    );
    assert_in_secondary_text(&server, "#server dt");
    let usage_limit = select(&server, "#server dd")[2];
    assert_eq!(
        select_in(usage_limit, ".status-text > .status[aria-hidden=true]").len(),
        1,
        "{}",
        usage_limit.html()
    );
    assert_eq!(
        select_in(usage_limit, ".status-text > svg.icon")
            .into_iter()
            .map(|icon| icon.attr("aria-label"))
            .collect::<Vec<_>>(),
        [Some("paused")],
        "{}",
        usage_limit.html()
    );
    assert_badges_hold_no_words(&server, "#server dl.card");
    assert_no_status_by_colour_alone(&server);
}

#[test_log::test(tokio::test)]
async fn a_drain_under_way_is_a_hold_linking_to_the_server_page() {
    let pages = Pages::of(View {
        memory: draining_since("2026-10-01T12:00:20Z"),
        ..a_ticket()
    })
    .await;

    let html = pages.page("/").await;

    assert_eq!(
        holds(&html),
        [(
            "drain: under way since 2026-10-01 12:00:20 UTC".to_string(),
            "/server".to_string()
        )]
    );
    assert_notice(&html, &["alert"], "pause-circle");
    let facts = server_facts(&pages.page("/server").await);
    assert_eq!(
        facts.get(2),
        Some(&ServerFact {
            label: "Drain".into(),
            value: "started 2026-10-01 12:00:20 UTC".into(),
            tone: Some("base-content/60".into()),
        }),
        "{facts:?}"
    );
}

#[test_log::test(tokio::test)]
async fn a_refused_delivery_is_a_hold_linking_to_the_server_page() {
    let pages = Pages::with_deliveries(a_ticket()).await;
    let unsigned = Request {
        signature: None,
        ..Request::signed_json("{}")
    };

    unsigned.send(pages.deliveries()).await;

    let html = pages.page("/runs").await;
    assert_eq!(
        holds(&html),
        [(
            "webhook delivery refused: 2026-10-01 12:00:02 UTC, none accepted since".to_string(),
            "/server".to_string()
        )]
    );
    assert_notice(&html, &["alert", "alert-error"], "x-circle");
}

#[test_log::test(tokio::test)]
async fn a_hold_comes_and_goes_through_the_stream() {
    let pages = Pages::with_deliveries(a_ticket()).await;
    let mut events = stream(&pages, "/events/runs").await;
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
    .send(pages.deliveries())
    .await;

    let changed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let changed = regions(&changed);
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].0, "holds");
    assert_eq!(select(&changed[0].1, ".alert").len(), 1);
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

#[test_log::test(tokio::test)]
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
    assert!(
        failed
            .value
            .ends_with(" UTC: taking a snapshot: GitHub is down"),
        "{failed:?}"
    );

    web.store.restore_reads();

    while !select(&shown, ".alert").is_empty() {
        shown = next_holds(&mut events).await;
    }
    assert_eq!(holds(&web.page("/").await), Vec::<(String, String)>::new());
    web.stop().await;
}

#[test_log::test(tokio::test)]
async fn a_file_of_the_repositorys_linewright_cannot_use_is_a_problem_linking_to_the_server_page() {
    let error = "the setting `max-runs` is the operator's";
    let pages = Pages::of(View {
        unusable: vec![Unusable {
            file: FileError {
                path: ".linewright/config.toml".into(),
                error: error.into(),
            },
            holds: Readers::Every,
        }],
        ..a_ticket()
    })
    .await;

    let html = pages.page("/").await;

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
    let facts = server_facts(&pages.page("/server").await);
    assert_eq!(
        facts.get(2),
        Some(&ServerFact {
            label: "Repository file".into(),
            value: format!("`.linewright/config.toml`: {error}"),
            tone: Some("error".into()),
        }),
        "{facts:?}"
    );
}

/// A fact of the server page, as a reader reads it, with the colour of its
/// value's status, such as `error`, if it shows one.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ServerFact {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) tone: Option<String>,
}

/// The server page's facts, in order.
pub(super) fn server_facts(html: &Html) -> Vec<ServerFact> {
    let labels = select(html, "#server dl > div > dt");
    let values = select(html, "#server dl > div > dd");
    assert_eq!(labels.len(), values.len(), "{}", html.html());
    labels
        .into_iter()
        .zip(values)
        .map(|(label, value)| ServerFact {
            label: text(label),
            value: text(value),
            tone: select_in(value, ".status-text > .status")
                .first()
                .and_then(|dot| {
                    dot.value()
                        .classes()
                        .find_map(|class| match class {
                            "bg-base-content/60" => Some("base-content/60"),
                            _ => class.strip_prefix("status-"),
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

#[test_log::test(tokio::test)]
async fn the_server_page_shows_the_facts_about_linewright_each_under_its_label() {
    let pages = Pages::with_deliveries(View {
        snapshot: a_ticket().snapshot,
        ..super::automations::with(&[("weekly.md", super::automations::WEEKLY)], false)
    })
    .await;
    signed(pages.deliveries(), "ping", "{}").await;

    let html = pages.page("/server").await;

    let facts = server_facts(&html);
    assert_eq!(
        labelled(&facts),
        [
            ("Started", "2026-10-01 12:00:01 UTC"),
            (
                "Polls",
                "last 2026-10-01 12:00:00 UTC, next 2026-10-01 12:01:00 UTC"
            ),
            ("Runs in flight", "0 of 4"),
            ("Last webhook delivery accepted", "2026-10-01 12:00:02 UTC"),
            ("Webhook deliveries refused", "0"),
        ],
        "{facts:?}"
    );
    assert!(facts.iter().all(|fact| fact.tone.is_none()), "{facts:?}");
    assert_eq!(select(&html, "#server dl.card").len(), 1, "{}", html.html());
    assert!(select(&html, "#server table").is_empty(), "{}", html.html());
    assert_eq!(select(&html, "#server time[datetime]").len(), 4);
    assert!(
        !html.html().contains("automation weekly.md"),
        "the automations' facts are not shown: {}",
        html.html()
    );
    assert_eq!(events_path(&html), Some("/events/server"));
}

#[test_log::test(tokio::test)]
async fn the_server_pages_stream_sends_the_frame_and_the_facts_then_what_changed() {
    let pages = Pages::with_deliveries(a_ticket()).await;
    let mut events = stream(&pages, "/events/server").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ids,
        ["holds", "title", "crumbs", "heading", "buttons", "server"]
    );

    Request {
        signature: None,
        ..Request::signed_json("{}")
    }
    .send(pages.deliveries())
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
}
