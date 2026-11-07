//! The environment on the server page, and the hold while there is none or
//! its latest build failed (`docs/specs/design_system.md`, The server page,
//! and Holds and problems).

use super::frame::{holds, server_facts};
use super::*;

use crate::environment::{Marks, set_up_with, setup_config};

#[test_log::test(tokio::test)]
async fn the_server_page_follows_a_build_shows_those_that_ended_and_asks_for_attention_until_one_succeeds()
 {
    let marks = Marks::new();
    let (go, fail) = (marks.path("go"), marks.path("fail"));
    std::fs::write(&fail, "").unwrap();
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    set_up_with(
        &world,
        &format!(
            "echo step; while [ ! -e '{}' ]; do sleep 0.05; done; test ! -e '{}'",
            go.display(),
            fail.display()
        ),
    )
    .await;
    let web = Web::polling(world, Duration::from_millis(200)).await;

    let building = web
        .eventually_page("/server", |html| {
            !select(html, "#server .build pre").is_empty()
        })
        .await;
    let mut events = stream(&web, "/events/server").await;
    std::fs::write(&go, "").unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let message = events.recv().await.unwrap();
            if message.contains(r#"<hx-partial id="server""#) && message.contains("exit 1") {
                return message;
            }
        }
    })
    .await
    .expect("the build's end streamed");
    let failed = web
        .eventually_page("/server", |html| {
            !select(html, "#server .build details").is_empty()
        })
        .await;
    web.store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/config.toml", &setup_config("echo ready"))],
            "Change",
        )
        .await
        .unwrap();
    let built = web
        .eventually_page("/server", |html| {
            select(html, "#server .build details").len() == 2
        })
        .await;

    assert!(ended.contains("build-1"), "{ended}");
    let held = holds(&building);
    assert!(
        held.len() == 1
            && held[0]
                .0
                .starts_with("environment: none yet, building since ")
            && held[0].1 == "/server",
        "{held:?}"
    );
    let running = text(select(&building, "#server .build")[0]);
    assert!(
        running.contains("build 1: started ")
            && running.contains("→ running")
            && running.contains("step"),
        "{running}"
    );
    let failed_held = holds(&failed);
    assert!(
        failed_held.len() == 1
            && failed_held[0]
                .0
                .starts_with("environment: the latest build failed ")
            && failed_held[0].1 == "/server",
        "{failed_held:?}"
    );
    let environment = server_facts(&failed)
        .into_iter()
        .find(|fact| fact.label == "Environment")
        .unwrap_or_else(|| panic!("{}", failed.html()));
    assert!(
        environment.value.starts_with("none yet")
            && environment.value.contains("the latest build failed ")
            && environment.value.contains("build 1: started ")
            && environment.tone.as_deref() == Some("warning"),
        "{environment:?}"
    );
    let failed_fold = select(&failed, "#server .build details")[0];
    assert!(failed_fold.attr("open").is_some(), "{}", failed_fold.html());
    assert!(
        text(failed_fold).contains("exit 1"),
        "{}",
        text(failed_fold)
    );

    assert_eq!(holds(&built), Vec::<(String, String)>::new());
    let folds = select(&built, "#server .build details");
    assert_eq!(
        folds
            .iter()
            .map(|fold| (
                fold.attr("open").is_some(),
                text(*fold).contains("echo ready")
            ))
            .collect::<Vec<_>>(),
        [(false, true), (true, false)],
        "{}",
        built.html()
    );
    let lines: Vec<(String, bool, bool)> = select(&built, "#server .build .line")
        .into_iter()
        .map(|line| {
            let chevron = line
                .children()
                .find_map(ElementRef::wrap)
                .is_some_and(|first| first.value().classes().any(|class| class == "chevron"));
            let settled = !select_in(line, "time.duration[datetime^=PT]").is_empty();
            (text(select_in(line, ".text")[0]), chevron, settled)
        })
        .collect();
    assert_eq!(
        lines[0],
        ("setup: echo ready → exit 0".to_string(), true, true)
    );
    assert!(
        lines[1].0.ends_with("→ exit 1") && lines[1].1 && lines[1].2,
        "{lines:?}"
    );
    assert_eq!(
        select(&building, "#server .build .line time.duration[data-since]").len(),
        1,
        "{}",
        building.html()
    );
    web.stop().await;
}
