use std::time::Duration;

use crate::common;

use chrono::{DateTime, Utc};
use linewright::automation::{self, Automation, AutomationName, Unparsed, parse};
use linewright::work_settings::{Effort, Partial};

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn name(name: &str) -> AutomationName {
    name.parse().unwrap()
}

fn error(text: &str) -> String {
    format!("{:#}", parse(&name("weekly.md"), text).unwrap_err())
}

const SPEC_EXAMPLE: &str = "---
on:
  schedule: \"0 6 * * 1\"
session-timeout: 60m
max-issues: 3
---
Look at the bugs fixed in the past week ...
";

#[test_log::test]
fn the_spec_example_parses_into_its_schedule_timeout_issue_cap_and_instructions() {
    let automation: Automation = parse(&name("weekly.md"), SPEC_EXAMPLE).unwrap();

    assert_eq!(automation.name.to_string(), "weekly.md");
    assert_eq!(
        automation.schedule.next_after(utc("2026-10-01T12:00:00Z")),
        Some(utc("2026-10-05T06:00:00Z"))
    );
    assert_eq!(
        automation.session.session_timeout,
        Some(Duration::from_hours(1))
    );
    assert_eq!(automation.max_issues, 3);
    assert_eq!(
        automation.instructions,
        "Look at the bugs fixed in the past week ...\n"
    );
}

#[test_log::test]
fn without_session_settings_or_an_issue_cap_an_automation_takes_the_defaults() {
    let automation = parse(
        &name("nightly.md"),
        "---\non:\n  schedule: \"30 2 * * *\"\n---\nTidy up.\n",
    )
    .unwrap();

    assert_eq!(
        automation.session,
        Partial::default(),
        "the whole pipeline's"
    );
    assert_eq!(automation.max_issues, 3);
}

#[test_log::test]
fn the_schedule_is_in_utc() {
    let automation = parse(
        &name("nightly.md"),
        "---\non:\n  schedule: \"30 2 * * *\"\nmax-issues: 1\n---\nTidy up.\n",
    )
    .unwrap();

    assert_eq!(
        automation
            .schedule
            .next_after(utc("2026-10-01T03:00:00+02:00")),
        Some(utc("2026-10-01T02:30:00Z"))
    );
    assert_eq!(automation.max_issues, 1);
}

#[test_log::test]
fn a_file_without_a_schedule_does_not_parse() {
    let error = error("---\nsession-timeout: 60m\n---\nLook.\n");

    assert!(error.contains("missing field `on`"), "{error}");
}

#[test_log::test]
fn a_trigger_without_a_schedule_does_not_parse() {
    let error = error("---\non: {}\n---\nLook.\n");

    assert!(error.contains("missing field `schedule`"), "{error}");
}

#[test_log::test]
fn an_invalid_cron_expression_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 25 * * *\"\n---\nLook.\n");

    assert!(error.contains("0 25 * * *"), "{error}");
}

#[test_log::test]
fn a_cron_expression_with_seconds_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 0 6 * * 1\"\n---\nLook.\n");

    assert!(error.contains("0 0 6 * * 1"), "{error}");
}

#[test_log::test]
fn bad_yaml_does_not_parse() {
    let error = error("---\non: [schedule\n---\nLook.\n");

    assert!(
        error.contains("the front matter is not an automation's"),
        "{error}"
    );
}

#[test_log::test]
fn a_file_without_front_matter_does_not_parse() {
    let error = error("Look at the bugs fixed in the past week ...\n");

    assert!(
        error.contains("does not open with YAML front matter"),
        "{error}"
    );
}

#[test_log::test]
fn an_unknown_key_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 6 * * 1\"\nmax_issues: 3\n---\nLook.\n");

    assert!(error.contains("max_issues"), "{error}");
}

#[test_log::test]
fn a_session_timeout_that_is_not_a_duration_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 6 * * 1\"\nsession-timeout: soon\n---\nLook.\n");

    assert!(error.contains("`session-timeout` is \"soon\""), "{error}");
}

#[test_log::test]
fn a_leftover_timeout_does_not_parse_naming_the_key() {
    let error = error("---\non:\n  schedule: \"0 6 * * 1\"\ntimeout: 15m\n---\nLook.\n");

    assert!(error.contains("`timeout`"), "{error}");
    assert!(error.contains("`session-timeout`"), "{error}");
}

#[test_log::test]
fn the_front_matter_takes_model_effort_and_session_timeout() {
    let automation = parse(
        &name("weekly.md"),
        "---
on:
  schedule: \"0 6 * * 1\"
model: opus
effort: high
session-timeout: 15m
---
Look.
",
    )
    .unwrap();

    assert_eq!(
        automation.session,
        Partial {
            model: Some("opus".to_string()),
            effort: Some(Effort::High),
            session_timeout: Some(Duration::from_mins(15)),
        }
    );
}

#[test_log::test]
fn the_front_matter_takes_no_operator_only_session_setting() {
    let error =
        error("---\non:\n  schedule: \"0 6 * * 1\"\nsession-pass-env: [HOME]\n---\nLook.\n");

    assert!(error.contains("`session-pass-env`"), "{error}");
}

#[test_log::test]
fn an_automation_names_its_model_and_effort_or_takes_the_whole_pipelines() {
    let chosen = parse(
        &name("weekly.md"),
        "---\non:\n  schedule: \"0 6 * * 1\"\nmodel: opus\neffort: high\n---\nLook.\n",
    )
    .unwrap();
    let unset = parse(
        &name("weekly.md"),
        "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
    )
    .unwrap();

    assert_eq!(chosen.session.model.as_deref(), Some("opus"));
    assert_eq!(chosen.session.effort, Some(Effort::High));
    assert_eq!((unset.session.model, unset.session.effort), (None, None));
}

#[test_log::test]
fn an_effort_outside_its_levels_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 6 * * 1\"\neffort: huge\n---\nLook.\n");

    assert!(error.contains("`effort` is \"huge\""), "{error}");
    assert!(
        error.contains("`low`, `medium`, `high`, `xhigh` or `max`"),
        "{error}"
    );
}

#[test_log::test(tokio::test)]
async fn every_file_under_the_automations_but_a_hidden_one_is_an_automation_or_does_not_parse() {
    let dir = tempfile::tempdir().unwrap();
    let store = common::store_holding(
        dir.path(),
        &[
            (".linewright/automations/hourly.md", common::HOURLY),
            (
                ".linewright/automations/broken.md",
                "---\non:\n  schedule: \"every hour\"\n---\nLook.\n",
            ),
            (".linewright/automations/notes.txt", "Notes.\n"),
            (".linewright/automations/team/weekly.md", common::HOURLY),
            (".linewright/automations/.gitkeep", ""),
            (".linewright/automations/.drafts/x.md", common::HOURLY),
        ],
    )
    .await;

    let files = automation::read(&store, "main").await.unwrap();

    assert_eq!(
        files.parsed,
        [
            parse(&name("hourly.md"), common::HOURLY).unwrap(),
            parse(&name("team/weekly.md"), common::HOURLY).unwrap(),
        ]
    );
    assert_eq!(
        files.unparsed,
        [
            Unparsed {
                path: ".linewright/automations/notes.txt".into(),
                reason: "an automation is a `.md` file".into(),
            },
            Unparsed {
                path: ".linewright/automations/broken.md".into(),
                reason: "`on.schedule` is \"every hour\", not a cron expression: Invalid \
                         pattern: Pattern must have 5 fields when seconds are disallowed."
                    .into(),
            },
        ]
    );
}

#[test_log::test]
fn an_automation_is_named_by_its_path_under_the_automations() {
    let named = name("team/weekly.md");

    assert_eq!(named.to_string(), "team/weekly.md");
    assert_eq!(named.path(), ".linewright/automations/team/weekly.md");
}

#[test_log::test]
fn a_name_is_a_path_of_visible_segments_ending_in_md() {
    for bad in [
        "",
        "weekly",
        "weekly.txt",
        "/weekly.md",
        "team//weekly.md",
        "team/.weekly.md",
        ".team/weekly.md",
        "team/",
        ".md/",
    ] {
        assert_eq!(
            bad.parse::<AutomationName>(),
            Err(format!(
                "{bad:?} is not an automation's name: the path of a `.md` file under \
                 `.linewright/automations/`, such as `team/weekly.md`"
            ))
        );
    }
}

#[test_log::test]
fn a_backslash_is_a_character_of_a_segment() {
    assert_eq!(
        name("team/a\\b.md").path(),
        ".linewright/automations/team/a\\b.md"
    );
}

#[test_log::test]
fn names_that_differ_have_file_names_that_differ_and_hold_no_slash() {
    let names = ["a/b.md", "a-b.md", "a%2Fb.md", "a--b.md", "a_b.md"];
    let file_names: std::collections::BTreeSet<String> =
        names.iter().map(|n| name(n).file_name()).collect();

    assert_eq!(file_names.len(), names.len(), "{file_names:?}");
    assert!(
        file_names
            .iter()
            .all(|f| !f.contains('/') && !f.starts_with('.')),
        "{file_names:?}"
    );
}
