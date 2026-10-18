use std::time::Duration;

use chrono::{DateTime, Utc};
use linewright::workflow::{Workflow, WorkflowName, parse};

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn name(name: &str) -> WorkflowName {
    name.parse().unwrap()
}

fn error(text: &str) -> String {
    format!("{:#}", parse(&name("weekly"), text).unwrap_err())
}

const SPEC_EXAMPLE: &str = "---
on:
  schedule: \"0 6 * * 1\"
timeout: 60m
max-issues: 3
---
Look at the bugs fixed in the past week ...
";

#[test]
fn the_spec_example_parses_into_its_schedule_timeout_issue_cap_and_instructions() {
    let workflow: Workflow = parse(&name("weekly"), SPEC_EXAMPLE).unwrap();

    assert_eq!(workflow.name.to_string(), "weekly");
    // 2026-10-01 is a Thursday: the next Monday 06:00 UTC is the 5th.
    assert_eq!(
        workflow.schedule.next_after(utc("2026-10-01T12:00:00Z")),
        Some(utc("2026-10-05T06:00:00Z"))
    );
    assert_eq!(workflow.timeout, Some(Duration::from_hours(1)));
    assert_eq!(workflow.max_issues, 3);
    assert_eq!(
        workflow.instructions,
        "Look at the bugs fixed in the past week ...\n"
    );
}

#[test]
fn without_a_timeout_or_an_issue_cap_a_workflow_takes_the_defaults() {
    let workflow = parse(
        &name("nightly"),
        "---\non:\n  schedule: \"30 2 * * *\"\n---\nTidy up.\n",
    )
    .unwrap();

    assert_eq!(workflow.timeout, None, "the `session-timeout` setting");
    assert_eq!(workflow.max_issues, 3);
}

#[test]
fn the_schedule_is_in_utc() {
    let workflow = parse(
        &name("nightly"),
        "---\non:\n  schedule: \"30 2 * * *\"\nmax-issues: 1\n---\nTidy up.\n",
    )
    .unwrap();

    assert_eq!(
        workflow
            .schedule
            .next_after(utc("2026-10-01T03:00:00+02:00")),
        // 01:00 UTC: 02:30 UTC is the same day.
        Some(utc("2026-10-01T02:30:00Z"))
    );
    assert_eq!(workflow.max_issues, 1);
}

#[test]
fn a_file_without_a_schedule_does_not_parse() {
    let error = error("---\ntimeout: 60m\n---\nLook.\n");

    assert!(error.contains("missing field `on`"), "{error}");
}

#[test]
fn a_trigger_without_a_schedule_does_not_parse() {
    let error = error("---\non: {}\n---\nLook.\n");

    assert!(error.contains("missing field `schedule`"), "{error}");
}

#[test]
fn an_invalid_cron_expression_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 25 * * *\"\n---\nLook.\n");

    assert!(error.contains("0 25 * * *"), "{error}");
}

#[test]
fn a_cron_expression_with_seconds_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 0 6 * * 1\"\n---\nLook.\n");

    assert!(error.contains("0 0 6 * * 1"), "{error}");
}

#[test]
fn bad_yaml_does_not_parse() {
    let error = error("---\non: [schedule\n---\nLook.\n");

    assert!(
        error.contains("the front matter is not a workflow's"),
        "{error}"
    );
}

#[test]
fn a_file_without_front_matter_does_not_parse() {
    let error = error("Look at the bugs fixed in the past week ...\n");

    assert!(
        error.contains("does not open with YAML front matter"),
        "{error}"
    );
}

#[test]
fn an_unknown_key_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 6 * * 1\"\nmax_issues: 3\n---\nLook.\n");

    assert!(error.contains("max_issues"), "{error}");
}

#[test]
fn a_timeout_that_is_not_a_duration_does_not_parse() {
    let error = error("---\non:\n  schedule: \"0 6 * * 1\"\ntimeout: soon\n---\nLook.\n");

    assert!(error.contains("`timeout` is \"soon\""), "{error}");
}
