use linewright_core::automation::{self, Automation, AutomationFiles, Unparsed};
use linewright_core::by_hand::{HandRefusal, take};
use linewright_core::repo_files::FileError;

fn weekly() -> Automation {
    automation::parse(
        &"weekly.md".parse().unwrap(),
        "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
    )
    .unwrap()
}

/// The automation files `weekly.md`, which parses, and `broken.md`, which
/// does not.
fn listed() -> AutomationFiles {
    AutomationFiles {
        parsed: vec![weekly()],
        unparsed: vec![Unparsed {
            path: ".linewright/automations/broken.md".into(),
            reason: "it has no front matter".into(),
        }],
    }
}

#[test_log::test]
fn a_request_for_an_automation_that_parsed_is_taken_for_it() {
    let files = listed();

    assert_eq!(
        take(&"weekly.md".parse().unwrap(), false, Some(&files)),
        Ok(Some(&weekly()))
    );
}

#[test_log::test]
fn a_request_while_the_automation_files_are_not_listed_is_taken_for_the_next_poll() {
    assert_eq!(take(&"weekly.md".parse().unwrap(), false, None), Ok(None));
}

#[test_log::test]
fn a_request_during_a_drain_is_refused() {
    let files = listed();

    let refused = take(&"weekly.md".parse().unwrap(), true, Some(&files)).unwrap_err();

    assert_eq!(refused, HandRefusal::Draining);
    assert_eq!(
        refused.to_string(),
        "`serve` is draining: it takes no request to run an automation"
    );
}

#[test_log::test]
fn a_request_for_no_automation_is_refused_naming_its_file() {
    let files = listed();

    let refused = take(&"daily.md".parse().unwrap(), false, Some(&files)).unwrap_err();

    assert_eq!(
        refused.to_string(),
        "there is no automation `daily.md`: the default branch has no \
         .linewright/automations/daily.md"
    );
}

#[test_log::test]
fn a_request_for_a_file_that_does_not_parse_is_refused_with_its_error() {
    let files = listed();

    let refused = take(&"broken.md".parse().unwrap(), false, Some(&files)).unwrap_err();

    assert_eq!(
        refused,
        HandRefusal::Unparsed(FileError {
            path: ".linewright/automations/broken.md".into(),
            error: "it has no front matter".into(),
        })
    );
    assert_eq!(
        refused.to_string(),
        ".linewright/automations/broken.md does not parse: it has no front matter"
    );
}
